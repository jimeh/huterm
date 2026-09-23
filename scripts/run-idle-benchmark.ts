/** macOS idle-process counters after a startup-only production fixture. */
import { chmod, mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { cpus, loadavg, release } from "node:os";
import { dirname, join, resolve } from "node:path";
import { parseArgs } from "node:util";

type Arm = { label: string; executable: string; revision: string; env?: Record<string, string> };
export function validateArms(value: unknown): Arm[] {
  if (!Array.isArray(value) || value.length === 0) throw new Error("arms must be a nonempty array");
  const labels = new Set<string>();
  for (const arm of value) {
    if (!arm || typeof arm !== "object" || ![arm.label, arm.executable, arm.revision].every(v => typeof v === "string" && v.length > 0)) {
      throw new Error("each arm requires label, executable and revision strings");
    }
    if (labels.has(arm.label)) throw new Error(`duplicate arm label: ${arm.label}`);
    labels.add(arm.label);
    if (arm.env !== undefined && (typeof arm.env !== "object" || arm.env === null || Array.isArray(arm.env) || Object.values(arm.env).some(v => typeof v !== "string"))) {
      throw new Error("arm env must contain string values");
    }
  }
  return value as Arm[];
}

export function armOrder<T>(arms: T[], round: number): T[] {
  return round % 2 === 0 ? [...arms] : [...arms].reverse();
}

function run(args: string[]): string {
  const child = Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe" });
  if (child.exitCode !== 0) throw new Error(`${args.join(" ")}: ${child.stderr.toString()}`);
  return child.stdout.toString().trim();
}

async function command(args: string[]): Promise<void> {
  const child = Bun.spawn(args, { stdout: "inherit", stderr: "inherit" });
  if (await child.exited !== 0) throw new Error(`${args.join(" ")} failed`);
}

async function main(): Promise<void> {
  const { values } = parseArgs({ args: Bun.argv.slice(2), options: {
    arms: { type: "string" }, output: { type: "string", default: "target/bench/idle/report.json" },
    duration: { type: "string", default: "5" }, repeats: { type: "string", default: "3" },
    settle: { type: "string", default: "2" }, tabs: { type: "string", default: "1,50" },
    windows: { type: "string", default: "1,2" },
  }, strict: true });
  if (process.platform !== "darwin") throw new Error("bench:idle currently requires macOS");
  const duration = Number(values.duration), repeats = Number(values.repeats), settle = Number(values.settle);
  const tabs = values.tabs!.split(",").map(Number), windows = values.windows!.split(",").map(Number);
  if (!Number.isFinite(duration) || duration <= 0 || !Number.isInteger(repeats) || repeats < 1 || !Number.isFinite(settle) || settle < 0
    || tabs.some(n => !Number.isInteger(n) || n < 1 || n > 50) || windows.some(n => n !== 1 && n !== 2)) throw new Error("invalid benchmark dimensions or duration");
  const output = resolve(values.output!);
  await mkdir(dirname(output), { recursive: true });
  const artifacts = await mkdtemp(join(dirname(output), "idle-artifacts-"));
  const sampler = join(artifacts, "idle-sample");
  await command(["xcrun", "clang", "-fobjc-arc", "-Wall", "-Wextra", "-Werror", "scripts/macos/idle-sample.m", "-framework", "Cocoa", "-framework", "ApplicationServices", "-o", sampler]);
  let arms: Arm[];
  if (values.arms) {
    arms = validateArms(JSON.parse(await readFile(values.arms, "utf8"))).map(arm => ({ ...arm, executable: resolve(arm.executable) }));
  } else {
    await command(["bash", "scripts/build-exec.sh", "cargo", "build", "--release", "--locked", "-p", "huterm-gpui", "--example", "idle_bench"]);
    arms = [{ label: "current", executable: resolve(process.env.CARGO_TARGET_DIR ?? "target", "release/examples/idle_bench"), revision: run(["git", "rev-parse", "HEAD"]) }];
  }
  const context = JSON.parse(run([sampler, "context"]));
  if (!context.unlocked) throw new Error("an unlocked console GUI session is required");
  const report = {
    started_at: new Date().toISOString(), artifacts, duration, repeats, settle,
    host: { model: run(["sysctl", "-n", "hw.model"]), os: release(), cpus: cpus(), display: context,
      display_details: run(["system_profiler", "SPDisplaysDataType", "-json"]), load_start: loadavg() },
    runner_revision: run(["git", "rev-parse", "HEAD"]), runner_dirty: run(["git", "status", "--porcelain"]),
    configuration: 'isolated config; tabs.label="title"; no Quake; blocking fixture shell; tabs are per window',
    arms: await Promise.all(arms.map(async arm => ({ ...arm, sha256: new Bun.CryptoHasher("sha256").update(await Bun.file(arm.executable).arrayBuffer()).digest("hex") }))),
    samples: [] as Record<string, unknown>[],
  };
  const save = () => writeFile(output, `${JSON.stringify(report, null, 2)}\n`);
  await save();
  const caffeinate = Bun.spawn(["caffeinate", "-di", "-w", String(process.pid)], { stdout: "ignore", stderr: "ignore" });
  let sequence = 0;
  try {
    for (let repeat = 0; repeat < repeats; repeat++) for (const windowCount of windows) for (const tabCount of tabs) {
      for (const arm of armOrder(arms, repeat + windows.indexOf(windowCount) + tabs.indexOf(tabCount))) {
        const directory = await mkdtemp(join(artifacts, "run-"));
        const shell = join(directory, "shell");
        await writeFile(shell, "#!/bin/sh\nprintf 'HUTERM_IDLE_READY\\n'\nwhile IFS= read -r line; do :; done\n");
        await chmod(shell, 0o755);
        const config = join(directory, "config.toml");
        await writeFile(config, '[tabs]\nlabel = "title"\n');
        const child = Bun.spawn([arm.executable], { env: { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("HUTERM_"))), ...arm.env,
          SHELL: shell, HUTERM_CONFIG_FILE: config, HUTERM_IDLE_TABS: String(tabCount), HUTERM_IDLE_WINDOWS: String(windowCount),
          XDG_STATE_HOME: join(directory, "state"),
        }, stdout: "pipe", stderr: Bun.file(join(directory, "stderr.log")) });
        let stdout = "";
        let resolveReady!: () => void;
        const ready = new Promise<void>(resolveReadyValue => { resolveReady = resolveReadyValue; });
        const pump = (async () => {
          for await (const chunk of child.stdout) {
            stdout += new TextDecoder().decode(chunk);
            if (stdout.includes(`huterm-idle ready windows=${windowCount} tabs_per_window=${tabCount} adapters=${windowCount}\n`)) resolveReady();
          }
        })();
        let timer: ReturnType<typeof setTimeout> | undefined;
        try {
          await Promise.race([ready, child.exited.then(code => { throw new Error(`startup exited ${code}: ${directory}`); }),
            new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error(`startup timeout: ${directory}`)), 100_000); })]);
          if (timer) clearTimeout(timer);
          for (const visibility of ["visible", "hidden"]) {
            const loadBefore = loadavg();
            const sample = Bun.spawn([sampler, String(child.pid), visibility, String(duration), String(settle)], { stdout: "pipe", stderr: "pipe" });
            const [text, error, code] = await Promise.all([new Response(sample.stdout).text(), new Response(sample.stderr).text(), sample.exited]);
            const counters = text.trim() ? JSON.parse(text) : { valid: false, error };
            report.samples.push({ sequence: sequence++, repeat, arm: arm.label, windows: windowCount, tabs_per_window: tabCount, visibility,
              pid: child.pid, startup: stdout, timestamp: new Date().toISOString(), load_before: loadBefore, load_after: loadavg(), directory, ...counters });
            await save();
            if (code !== 0 || !counters.valid) throw new Error(`discarded invalid sample: ${error || text}`);
            console.log(`${arm.label} ${windowCount}w/${tabCount}t ${visibility}: CPU ${counters.cpu_percent_one_core.toFixed(3)}%, wakeups ${counters.interrupt_wakeups_per_second.toFixed(2)}/s`);
          }
        } finally {
          if (timer) clearTimeout(timer);
          Bun.spawnSync([sampler, "terminate", String(child.pid)], { stdout: "ignore", stderr: "ignore" });
          let cleanupTimer: ReturnType<typeof setTimeout> | undefined;
          let forcedCleanup = false;
          await Promise.race([child.exited, new Promise<void>(resolveTimeout => { cleanupTimer = setTimeout(() => {
            forcedCleanup = true;
            child.kill("SIGKILL");
            resolveTimeout();
          }, 10_000); })]);
          if (cleanupTimer) clearTimeout(cleanupTimer);
          const exitCode = await child.exited;
          await pump;
          await writeFile(join(directory, "stdout.log"), stdout);
          await writeFile(join(directory, "cleanup.json"), JSON.stringify({ exit_code: exitCode, forced: forcedCleanup }));
          if (forcedCleanup || exitCode !== 0) throw new Error(`benchmark cleanup failed (exit ${exitCode}, forced ${forcedCleanup}): ${directory}`);
        }
      }
    }
  } finally {
    caffeinate.kill();
    await caffeinate.exited;
    await save();
  }
  console.log(`Idle report: ${output}`);
}

if (import.meta.main) await main();
