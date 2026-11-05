/** Quit with two windows, a split, custom names, and a `cd`, then relaunch
 * on the same state directory and check that the same structure returns
 * with fresh shells in the saved directories. */
import { chmod, mkdir, mkdtemp, readFile, realpath, rename, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

if (!process.argv[2]) throw new Error("usage: check-restore.ts <restore_smoke>");
// Absolute, since the app starts in the scratch directory.
const executable = resolve(process.argv[2]);

async function waitFor<T>(label: string, check: () => Promise<T | undefined>, timeout = 15_000): Promise<T> {
  const deadline = performance.now() + timeout;
  for (;;) {
    const value = await check();
    if (value !== undefined) return value;
    if (performance.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await Bun.sleep(20);
  }
}

/** `key=value` fields of a state file; quoted values are JSON strings. */
export function parseState(text: string): Record<string, string> {
  const fields: Record<string, string> = {};
  for (const match of text.matchAll(/([\w.]+)=("(?:[^"\\]|\\.)*"|\S+)/g)) {
    const [, key, raw] = match as unknown as [string, string, string];
    fields[key] = raw.startsWith('"') ? JSON.parse(raw) : raw;
  }
  return fields;
}

const directory = await realpath(await mkdtemp(join(tmpdir(), "huterm-restore-")));
const stateRoot = join(directory, "state-root");
const project = join(directory, "project");
const pids = join(directory, "pids");
await mkdir(project);
const shell = join(directory, "shell");
// A shell that records its PID, reports its directory through OSC 7 at
// start and after each `cd`, and waits for input.
await writeFile(shell, `#!/bin/sh
printf '%s\\n' "$$" >> '${pids}'
report() { printf '\\033]7;file://localhost%s\\033\\\\' "$(pwd -P)"; }
report
printf 'READY\\n'
while IFS= read -r line; do
  case "$line" in
    cd\\ *) cd "\${line#cd }" && report ;;
  esac
done
`);
await chmod(shell, 0o755);
const config = join(directory, "config.toml");
await writeFile(config, `[terminal]\nclose_on_exit = false\n\n[tabs]\nposition = "top"\n`);

async function shellPids(): Promise<number[]> {
  const text = await readFile(pids, "utf8").catch(() => "");
  return text.split("\n").filter(Boolean).map(Number);
}

async function launch(run: number) {
  const fixture = join(directory, `run-${run}`);
  await mkdir(fixture);
  const app = Bun.spawn([executable], {
    env: {
      ...process.env,
      WAYLAND_DISPLAY: undefined,
      HUTERM_RESTORE_SMOKE: fixture,
      HUTERM_CONFIG_FILE: config,
      HUTERM_STATE_DIR: stateRoot,
      HUTERM_STATE_TIMING: "1",
      SHELL: shell,
      HOME: directory,
    },
    // The default launch directory on Linux; HOME is macOS's.
    cwd: directory,
    stdout: "pipe",
    stderr: "pipe",
  });
  const output = Promise.all([new Response(app.stdout).text(), new Response(app.stderr).text()]);
  let sequence = 0;
  const read = async () => parseState(await readFile(join(fixture, "state"), "utf8").catch(() => ""));
  const state = (label: string, done: (fields: Record<string, string>) => boolean) =>
    waitFor(label, async () => {
      const fields = await read();
      return done(fields) ? fields : undefined;
    });
  async function command(...fields: string[]): Promise<string> {
    const index = sequence++;
    const pending = join(fixture, "command-next");
    await writeFile(pending, fields.join("\t"));
    await rename(pending, join(fixture, `command-${index}`));
    const result = await waitFor(`command ${fields.join(" ")}`, async () =>
      readFile(join(fixture, `result-${index}`), "utf8").catch(() => undefined));
    if (result.startsWith("error:")) throw new Error(`${fields.join(" ")}: ${result}`);
    return result;
  }
  async function exit(): Promise<string> {
    const code = await Promise.race([app.exited, Bun.sleep(15_000).then(() => undefined)]);
    if (code === undefined) {
      app.kill("SIGKILL");
      throw new Error("Huterm did not exit after Quit");
    }
    const [, stderr] = await output;
    if (code !== 0) throw new Error(`Huterm exited with ${code}:\n${stderr}`);
    return stderr;
  }
  return { app, read, state, command, exit, output };
}

const finalWrites: number[] = [];
let failed = false;
let first: Awaited<ReturnType<typeof launch>> | undefined;
let second: Awaited<ReturnType<typeof launch>> | undefined;
try {
  first = await launch(1);
  await first.state("the first window", (s) => s.windows === "1" && s["w0.t0.panes"]?.endsWith(`|${directory}`) === true);
  await first.command("invoke", "-", "new_window");
  await first.state("the second window", (s) => s.windows === "2" && s["w1.tabs"] === "1");
  await first.command("invoke", "0", "split_right");
  await first.state("the split", (s) => s["w0.t0.weights"] !== undefined && s["w0.t0.weights"] !== "-" && s["w0.t0.focus"] === "1");
  await first.command("invoke", "0", "resize_pane_left");
  const resized = await first.state("the resized split", (s) => s["w0.t0.weights"] !== "500000/500000");
  await first.command("invoke", "0", "rename_session", "name=restored-main");
  await first.command("invoke", "0", "rename_tab", "name=build");
  await first.command("invoke", "0", "rename_pane", "name=editor");
  await first.command("type", "0", `cd ${project}\\r`);
  const before = await first.state("names and the cd", (s) =>
    s["w0.session"] === "restored-main" && s["w0.t0.name"] === "build"
      && s["w0.t0.panes"] === `|${directory};editor|${project}`);
  const firstPids = await waitFor("three shells", async () => {
    const found = await shellPids();
    return found.length === 3 ? found : undefined;
  });
  if (before["w0.t0.weights"] !== resized["w0.t0.weights"]) throw new Error("weights moved before Quit");
  await first.command("invoke", "-", "quit");
  const stderr = await first.exit();
  first = undefined;
  const timing = /huterm-state final-write=Saved micros=(\d+)/.exec(stderr);
  if (!timing) throw new Error(`Quit wrote nothing:\n${stderr}`);
  finalWrites.push(Number(timing[1]));
  for (const name of ["runtime.json", "desktop.json"]) {
    if (!existsSync(join(stateRoot, "sockets/default", name))) throw new Error(`Quit left no ${name}`);
  }
  for (const pid of firstPids) {
    let alive = true;
    try { process.kill(pid, 0); } catch { alive = false; }
    if (alive) throw new Error(`shell ${pid} outlived Quit`);
  }

  second = await launch(2);
  const after = await second.state("the restored windows", (s) =>
    s.windows === "2" && s.restoring === "false" && s.persistence === "Running"
      && s["w0.t0.panes"] === `|${directory};editor|${project}` && s["w1.t0.panes"] === `|${directory}`);
  const secondPids = await waitFor("three fresh shells", async () => {
    const found = await shellPids();
    return found.length === 6 ? found.slice(3) : undefined;
  });
  if (secondPids.some((pid) => firstPids.includes(pid))) throw new Error("a restored pane reused a PID");
  const expect = (key: string, value: string | undefined) => {
    if (after[key] !== value) throw new Error(`${key}: ${after[key]} != ${value}`);
  };
  expect("w0.session", "restored-main");
  expect("w0.t0.name", "build");
  expect("w0.t0.weights", before["w0.t0.weights"]);
  expect("w0.t0.focus", "1");
  expect("w0.tabs", "1");
  expect("w1.tabs", "1");
  for (const window of ["w0", "w1"]) {
    expect(`${window}.mode`, before[`${window}.mode`]);
    expect(`${window}.fullscreen`, "false");
    // Without a window manager, X11 may place windows anywhere; their size
    // still comes back.
    const bounds = (state: Record<string, string>) => state[`${window}.bounds`]?.split(",").map(Number) ?? [];
    const [was, now] = [bounds(before), bounds(after)];
    if (![was, now].every((values) => values.length === 4 && values.every(Number.isFinite))) {
      throw new Error(`${window} bounds are not four numbers: ${was} and ${now}`);
    }
    const compared = process.platform === "linux" ? [2, 3] : [0, 1, 2, 3];
    if (compared.some((index) => Math.abs(was[index]! - now[index]!) > 1)) {
      throw new Error(`${window} bounds ${was} came back as ${now}`);
    }
  }
  await second.command("invoke", "-", "quit");
  const secondStderr = await second.exit();
  second = undefined;
  const secondTiming = /huterm-state final-write=\w+ micros=(\d+)/.exec(secondStderr);
  if (secondTiming) finalWrites.push(Number(secondTiming[1]));
  console.log(`Restore smoke passed; final writes took ${finalWrites.map((micros) => `${(micros / 1000).toFixed(2)} ms`).join(", ")}`);
} catch (error) {
  failed = true;
  for (const run of [first, second]) {
    if (!run) continue;
    run.app.kill("SIGKILL");
    const [, stderr] = await run.output;
    console.error(stderr);
    console.error(JSON.stringify(await run.read(), null, 2));
  }
  throw error;
} finally {
  if (!failed) await rm(directory, { recursive: true, force: true });
}
