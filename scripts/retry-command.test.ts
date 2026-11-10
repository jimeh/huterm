import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync } from "node:fs";
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
  for (const pid of children) expect(alive(pid)).toBe(false);
  expect(messages[0]).toContain("attempt 1 of 2 of `sh -c");
  expect(messages[0]).toContain("timed out after 0.3 s; retrying");
  expect(messages.at(-1)).toContain("has timed out after 2 attempts of 0.3 s");
});
