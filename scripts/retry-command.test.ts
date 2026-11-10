import { expect, test } from "bun:test";
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { retryCommand } from "./retry-command.ts";

const scratch = () => mkdtempSync(join(tmpdir(), "retry-command-"));

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

/** Polls until `condition` holds; a killed orphan stays visible until init reaps it. */
async function eventually(condition: () => boolean, describe: () => string, timeoutMs = 3_000): Promise<void> {
  const deadline = performance.now() + timeoutMs;
  while (!condition()) {
    if (performance.now() > deadline) throw new Error(`timed out waiting: ${describe()}`);
    await Bun.sleep(20);
  }
}

test("retries a failing command until an attempt succeeds", async () => {
  const counter = join(scratch(), "count");
  // Fails on the first two attempts and succeeds on the third.
  const script = `n=$(cat "$1" 2>/dev/null || echo 0); n=$((n + 1)); echo $n > "$1"; [ "$n" -ge 3 ]`;
  const messages: string[] = [];
  const code = await retryCommand(["sh", "-c", script, "sh", counter], { attempts: 3, timeoutMs: 10_000, log: (message) => messages.push(message) });
  expect(code).toBe(0);
  expect(readFileSync(counter, "utf8").trim()).toBe("3");
  expect(messages).toEqual([
    expect.stringContaining("attempt 1 of 3 of `sh -c"),
    expect.stringContaining("attempt 2 of 3 of `sh -c"),
  ]);
});

test("stops after the last attempt and returns its exit status", async () => {
  const counter = join(scratch(), "count");
  const messages: string[] = [];
  const code = await retryCommand(["sh", "-c", `echo x >> "$1"; exit 3`, "sh", counter], { attempts: 2, timeoutMs: 10_000, log: (message) => messages.push(message) });
  expect(code).toBe(3);
  expect(readFileSync(counter, "utf8")).toBe("x\nx\n");
  expect(messages.at(-1)).toMatch(/`sh -c .*` failed after 2 attempts$/);
});

test("ends a timed-out attempt's whole process group and reports the timeout", async () => {
  const directory = scratch();
  const pids = join(directory, "pids");
  // The background sleep stands in for a process the command starts; it must
  // not outlive its attempt.
  const script = `sleep 30 & echo $! >> "$1"; wait`;
  const messages: string[] = [];
  const started = performance.now();
  const code = await retryCommand(["sh", "-c", script, "sh", pids], { attempts: 2, timeoutMs: 300, graceMs: 1_000, log: (message) => messages.push(message) });
  expect(performance.now() - started).toBeLessThan(5_000);
  expect(code).toBe(124);
  const children = readFileSync(pids, "utf8").trim().split("\n").map(Number);
  expect(children).toHaveLength(2);
  for (const pid of children) await eventually(() => !alive(pid), () => `pid ${pid} still alive`);
  expect(messages[0]).toContain("attempt 1 of 2 of `sh -c");
  expect(messages[0]).toContain("timed out after 0.3 s; retrying");
  expect(messages.at(-1)).toContain("has timed out after 2 attempts of 0.3 s");
});

/** Runs the helper's CLI around a shell script that receives a scratch file path as $1. */
function runHelper(timeoutSeconds: string, script: string, file: string) {
  const helper = spawn(process.execPath, [join(import.meta.dir, "retry-command.ts"), "--attempts", "1", "--timeout", timeoutSeconds, "--",
    "sh", "-c", script, "sh", file], { stdio: ["ignore", "ignore", "inherit"] });
  const exited = new Promise<number | null>((resolve) => helper.once("exit", (code) => resolve(code)));
  return { helper, exited };
}

/** A survivor would keep the test runner's stderr open and hang the run. */
function killGroup(pid: number) {
  try {
    process.kill(-pid, "SIGKILL");
  } catch {
    // Already gone.
  }
}

const lines = (file: string) => existsSync(file) ? readFileSync(file, "utf8").split("\n").filter(Boolean) : [];

test("forwards a signal it receives to the attempt's process group", async () => {
  const file = join(scratch(), "events");
  const { helper, exited } = runHelper("60", `echo $$ > "$1"; exec sleep 30`, file);
  await eventually(() => lines(file).length > 0 && readFileSync(file, "utf8").endsWith("\n"), () => "the command never started");
  const pid = Number(lines(file)[0]);
  try {
    helper.kill("SIGTERM");
    expect(await exited).toBe(143);
    await eventually(() => !alive(pid), () => `pid ${pid} outlived the helper`);
  } finally {
    killGroup(pid);
  }
});

test("kills a timed-out attempt that ignores SIGTERM when the helper is signalled in its grace period", async () => {
  const file = join(scratch(), "events");
  const { helper, exited } = runHelper("0.2", `trap 'echo term >> "$1"' TERM; echo $$ >> "$1"; while :; do sleep 0.05; done`, file);
  // The trap's line shows the timeout's SIGTERM arrived and was ignored; the
  // default grace period is far longer than this test.
  await eventually(() => lines(file).includes("term"), () => `no timeout SIGTERM yet: ${JSON.stringify(lines(file))}`);
  const pid = Number(lines(file)[0]);
  try {
    helper.kill("SIGTERM");
    expect(await exited).toBe(143);
    await eventually(() => !alive(pid), () => `pid ${pid} outlived the helper`);
  } finally {
    killGroup(pid);
  }
});
