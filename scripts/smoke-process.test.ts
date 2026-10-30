import { expect, test } from "bun:test";
import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkSmokeProcess, processGroupThreads, runSmokeProcess, type ThreadState } from "./smoke-process.ts";

const run = (source: string, timeoutMs = 5_000) => runSmokeProcess(
  [process.execPath, "-e", source], { timeoutMs, graceMs: 50, stream: false },
);

test("smoke process drains both pipes and records the real exit", async () => {
  const result = await run('console.log("complete"); console.error("diagnostic"); process.exitCode = 7');
  expect(result.stdout).toBe("complete\n");
  expect(result.stderr).toBe("diagnostic\n");
  expect(result.exitCode).toBe(7);
  expect(result.timedOut).toBe(false);
  expect(() => checkSmokeProcess(result, "fixture")).toThrow("exited 7");
});

test("success output does not forgive a stuck exit or ignored TERM", async () => {
  const result = await run('process.on("SIGTERM", () => {}); console.log("passed"); setInterval(() => {}, 10)', 1_500);
  expect(result.stdout).toBe("passed\n");
  expect(result.timedOut).toBe(true);
  expect(result.signalCode).toBe("SIGKILL");
  expect(() => checkSmokeProcess(result, "fixture")).toThrow("timed out");
});

test("a stop condition ends the process before its deadline", async () => {
  const result = await runSmokeProcess(
    [process.execPath, "-e", 'console.error("working"); setTimeout(() => console.error("finished"), 50); setInterval(() => {}, 10)'],
    { timeoutMs: 10_000, graceMs: 50, stream: false, stopWhen: stderr => stderr.includes("finished") },
  );
  expect(result.stopped).toBe(true);
  expect(result.timedOut).toBe(false);
  expect(result.stderr).toContain("finished");
  expect(result.elapsedMs).toBeLessThan(5_000);
});

test("smoke process distinguishes a signal from its deadline", async () => {
  const result = await run('process.kill(process.pid, "SIGTERM")');
  expect(result.signalCode).toBe("SIGTERM");
  expect(result.timedOut).toBe(false);
  expect(() => checkSmokeProcess(result, "fixture")).toThrow("terminated by SIGTERM");
});

test("smoke deadline closes pipes retained by a descendant after parent exit", async () => {
  const result = await run('require("node:child_process").spawn("sh", ["-c", "sleep 30"], { stdio: "inherit" }); process.exit(0)', 1_500);
  expect(result.exitCode).toBe(0);
  expect(result.timedOut).toBe(true);
  expect(result.elapsedMs).toBeLessThan(5_000);
});

test("smoke evidence records output timing before process completion", async () => {
  const directory = await mkdtemp(join(tmpdir(), "huterm-process-test-"));
  try {
    const release = join(directory, "release");
    const pending = runSmokeProcess([process.execPath, "-e", `
      const { existsSync } = require("node:fs");
      console.log("painted");
      const timer = setInterval(() => {
        if (existsSync(${JSON.stringify(release)})) { clearInterval(timer); console.log("passed"); }
      }, 10);
    `], { timeoutMs: 5_000, evidenceDir: directory, stream: false });
    try {
      const deadline = performance.now() + 4_000;
      while (!(await readFile(join(directory, "stdout.log"), "utf8")).includes("painted")) {
        if (performance.now() >= deadline) throw new Error("fixture never produced live output");
        await Bun.sleep(10);
      }
      expect(await Bun.file(join(directory, "outcome.json")).exists()).toBe(false);
    } finally {
      await writeFile(release, "release");
    }
    const result = await pending;
    checkSmokeProcess(result, "fixture");
    const events = (await readFile(join(directory, "events.jsonl"), "utf8")).trim().split("\n").map(line => JSON.parse(line));
    expect(events.filter(event => event.kind === "stdout").map(event => event.text).join("")).toBe("painted\npassed\n");
    expect(events.findIndex(event => event.kind === "stdout")).toBeLessThan(events.findIndex(event => event.kind === "closed"));
    expect(await readFile(join(directory, "stdout.log"), "utf8")).toBe(result.stdout);
    expect(JSON.parse(await readFile(join(directory, "outcome.json"), "utf8")).exitCode).toBe(0);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test.skipIf(process.platform !== "linux")("a smoke deadline records the smoke's thread states", async () => {
  const directory = await mkdtemp(join(tmpdir(), "huterm-process-test-"));
  try {
    // Spawning returns once `sleep` has exec'd, so it is named by the deadline.
    const result = await runSmokeProcess(["sleep", "30"], { timeoutMs: 300, graceMs: 50, evidenceDir: directory, stream: false });
    expect(result.timedOut).toBe(true);
    const events = (await readFile(join(directory, "events.jsonl"), "utf8")).trim().split("\n").map(line => JSON.parse(line));
    const pid = events.find(event => event.kind === "started")?.pid;
    const threads: ThreadState[] = events.find(event => event.kind === "threads")?.threads ?? [];
    expect(threads.map(thread => [thread.pid, thread.tid, thread.comm])).toEqual([[pid, pid, "sleep"]]);
    expect(typeof threads[0]?.state).toBe("string");
    expect(typeof threads[0]?.wchan).toBe("string");
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test.skipIf(process.platform !== "linux")("process group threads include every member and nothing else", async () => {
  // The shell reports ready only once its background child has exec'd.
  const shell = spawn("sh", ["-c", 'sleep 30 & until [ "$(cat /proc/$!/comm)" = sleep ]; do :; done; echo ready; wait'], { detached: true, stdio: ["ignore", "pipe", "ignore"] });
  try {
    await new Promise<void>((resolve, reject) => {
      shell.once("error", reject);
      shell.stdout.on("data", (chunk: Buffer) => { if (chunk.toString().includes("ready")) resolve(); });
    });
    const threads = processGroupThreads(shell.pid!);
    expect(threads.map(thread => thread.comm).sort()).toEqual(["sh", "sleep"]);
    expect(threads.find(thread => thread.comm === "sh")?.pid).toBe(shell.pid);
  } finally {
    process.kill(-shell.pid!, "SIGKILL");
  }
});

test("missing smoke executable rejects promptly", async () => {
  await expect(runSmokeProcess(["/nonexistent/huterm-smoke"], { timeoutMs: 100, stream: false })).rejects.toThrow();
});

test("cancelling the supervisor terminates its child and cannot report success", async () => {
  const module = join(import.meta.dir, "smoke-process.ts");
  const source = `
    import { runSmokeProcess } from ${JSON.stringify(module)};
    const outcome = await runSmokeProcess([process.execPath, "-e", 'process.kill(process.ppid, "SIGTERM"); setTimeout(() => process.exit(3), 1500)'], { timeoutMs: 2000, stream: false });
    console.log(JSON.stringify(outcome));
  `;
  const result = await run(source, 3_000);
  expect(result.exitCode).toBe(0);
  const inner = JSON.parse(result.stdout);
  expect(inner.signalCode).toBe("SIGTERM");
  expect(inner.timedOut).toBe(false);
  expect(() => checkSmokeProcess(inner, "fixture")).toThrow("terminated by SIGTERM");
});

test.skipIf(process.platform === "win32")("a hung-up supervisor stops its smoke's process group and records the outcome", async () => {
  const directory = await mkdtemp(join(tmpdir(), "huterm-hangup-"));
  let group: number | undefined;
  try {
    const started = join(directory, "started");
    const finished = join(directory, "finished");
    const module = join(import.meta.dir, "smoke-process.ts");
    // The smoke keeps printing, so the supervisor keeps streaming to the
    // terminal after it hangs up, and ignores SIGTERM, so only the
    // supervisor's SIGKILL after its grace period stops it.
    const native = `process.on("SIGTERM", () => {}); require("node:fs").writeFileSync(${JSON.stringify(started)}, String(process.pid)); setInterval(() => console.log("frame"), 5);`;
    const supervisor = `
      import { writeFileSync } from "node:fs";
      import { runSmokeProcess } from ${JSON.stringify(module)};
      const outcome = await runSmokeProcess([process.execPath, "-e", ${JSON.stringify(native)}], { timeoutMs: 20_000, graceMs: 500 });
      writeFileSync(${JSON.stringify(finished)}, JSON.stringify({ signalCode: outcome.signalCode, timedOut: outcome.timedOut }));
    `;
    // The terminal makes the supervisor a session leader, so closing it
    // hangs up the supervisor as a closed terminal window or dropped SSH
    // session would.
    const child = Bun.spawn([process.execPath, "-e", supervisor], { terminal: { data() {} } });
    const deadline = Date.now() + 10_000;
    while (!((group = Number(await readFile(started, "utf8").catch(() => ""))) > 0)) {
      if (Date.now() > deadline) throw new Error("the smoke never started");
      await Bun.sleep(10);
    }
    child.terminal!.close();
    // Not its status: Bun can report a write already in flight when the
    // terminal closed as an unhandled rejection, which fails the exit.
    await child.exited;
    const alive = () => { try { process.kill(-group!, 0); return true; } catch { return false; } };
    const stopped = Date.now() + 5_000;
    while (alive() && Date.now() < stopped) await Bun.sleep(10);
    expect(alive()).toBe(false);
    expect(JSON.parse(await readFile(finished, "utf8").catch(() => "null"))).toEqual({ signalCode: "SIGHUP", timedOut: false });
  } finally {
    if (group) { try { process.kill(-group, "SIGKILL"); } catch {} }
    await rm(directory, { recursive: true, force: true });
  }
}, 20_000);


test("nested cancellation allows the inner supervisor to force native cleanup", async () => {
  const directory = await mkdtemp(join(tmpdir(), "huterm-nested-cancel-"));
  try {
    const ready = join(directory, "ready");
    const module = join(import.meta.dir, "smoke-process.ts");
    const native = `
      process.on("SIGTERM", () => {});
      require("node:fs").writeFileSync(${JSON.stringify(ready)}, "ready");
      setTimeout(() => process.exit(3), 4000);
    `;
    const inner = `
      import { runSmokeProcess } from ${JSON.stringify(module)};
      const result = await runSmokeProcess([process.execPath, "-e", ${JSON.stringify(native)}], { timeoutMs: 3000, graceMs: 50, stream: false });
      console.log(JSON.stringify(result));
    `;
    const outer = `
      import { runSmokeProcess } from ${JSON.stringify(module)};
      const timer = setInterval(() => {
        if (require("node:fs").existsSync(${JSON.stringify(ready)})) {
          clearInterval(timer);
          process.kill(process.pid, "SIGTERM");
        }
      }, 10);
      const result = await runSmokeProcess([process.execPath, "-e", ${JSON.stringify(inner)}], { timeoutMs: 3500, graceMs: 500, stream: false });
      clearInterval(timer);
      console.log(JSON.stringify(result));
    `;
    const result = await run(outer);
    checkSmokeProcess(result, "test driver");
    const outerResult = JSON.parse(result.stdout);
    expect(outerResult.signalCode).toBe("SIGTERM");
    expect(outerResult.timedOut).toBe(false);
    const innerResult = JSON.parse(outerResult.stdout);
    expect(innerResult.signalCode).toBe("SIGTERM");
    expect(innerResult.exitCode).toBe(null);
    expect(innerResult.timedOut).toBe(false);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
