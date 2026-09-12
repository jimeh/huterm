import { expect, test } from "bun:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkSmokeProcess, runSmokeProcess } from "./smoke-process.ts";

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
