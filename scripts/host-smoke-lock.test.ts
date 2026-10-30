import { afterEach, describe, expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const script = join(import.meta.dir, "host-smoke-lock.ts");
const worktree = resolve(import.meta.dir, "..");
const darwin = process.platform === "darwin";

const directories: string[] = [];
const processes: ReturnType<typeof Bun.spawn>[] = [];
const pids: number[] = [];
afterEach(() => {
  for (const child of processes.splice(0)) child.kill("SIGKILL");
  for (const pid of pids.splice(0)) { try { process.kill(pid, "SIGKILL"); } catch {} }
  for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true });
});

function fixture() {
  const directory = mkdtempSync(join(tmpdir(), "huterm-host-smoke-lock-test-"));
  directories.push(directory);
  return { state: join(directory, "state"), files: directory };
}

type Run = { child: ReturnType<typeof Bun.spawn>; stderr: () => string; exit: Promise<number> };

/** Starts the wrapper with a private lock and no inherited hold. */
function start(state: string, command: string[], env: Record<string, string> = {}, task = "test-task"): Run {
  const base = Object.fromEntries(Object.entries(process.env).filter(([name]) => name !== "HUTERM_HOST_SMOKE_LOCK_HELD" && name !== "HUTERM_CI_SMOKE_STEP"));
  const child = Bun.spawn([process.execPath, script, "--", ...command], {
    env: { ...base, HUTERM_HOST_SMOKE_STATE: state, MISE_TASK_NAME: task, ...env },
    stdin: "ignore", stdout: "ignore", stderr: "pipe",
  });
  processes.push(child);
  let stderr = "";
  const decoder = new TextDecoder();
  const drained = (async () => { for await (const chunk of child.stderr) stderr += decoder.decode(chunk); })();
  return { child, stderr: () => stderr, exit: child.exited.then(async (code) => { await drained; return code; }) };
}

async function until(condition: () => boolean, description: string, describeState: () => string = () => "", timeoutMs = 3_000) {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${description}; last state: ${describeState()}`);
    await Bun.sleep(20);
  }
}

/** A command that marks itself started and runs until the test creates `release`. */
const holder = (files: string, name: string) =>
  ["sh", "-c", `touch "$1/${name}.started"; while [ ! -e "$1/${name}.release" ]; do sleep 0.05; done`, "sh", files];
const marker = (files: string, name: string) => ["sh", "-c", `touch "$1/${name}"`, "sh", files];

describe.skipIf(!darwin)("the host smoke lock", () => {
  test("a second run waits, naming the holder, until the first releases", async () => {
    const { state, files } = fixture();
    const first = start(state, holder(files, "a"), {}, "smoke:first");
    await until(() => existsSync(join(files, "a.started")), "the first command", first.stderr);
    const second = start(state, marker(files, "b.ran"), {}, "smoke:second");
    await until(() => second.stderr().includes("Waiting for the host smoke lock"), "the waiting message", second.stderr);
    const message = second.stderr();
    expect(message).toContain("smoke:first");
    expect(message).toContain(worktree);
    expect(message).toContain(`PID ${first.child.pid}`);
    expect(message.match(/Waiting for the host smoke lock/g)).toHaveLength(1);
    expect(existsSync(join(files, "b.ran"))).toBe(false);
    writeFileSync(join(files, "a.release"), "");
    expect(await first.exit).toBe(0);
    expect(await second.exit).toBe(0);
    expect(existsSync(join(files, "b.ran"))).toBe(true);
  });

  test("the kernel releases a killed holder's lock even while its descendants live", async () => {
    const { state, files } = fixture();
    // The descendants outlive the wrapper; they must not inherit the lock.
    const first = start(state, ["sh", "-c", `echo $$ > "$1/shell"; sleep 30 & echo $! > "$1/sleep"; touch "$1/a.started"; wait`, "sh", files]);
    await until(() => existsSync(join(files, "a.started")), "the first command", first.stderr);
    for (const name of ["shell", "sleep"]) pids.push(Number(readFileSync(join(files, name), "utf8")));
    first.child.kill("SIGKILL");
    // The orphans still hold the stderr pipe, so wait for the exit alone.
    await first.child.exited;
    const second = start(state, marker(files, "b.ran"), { HUTERM_HOST_SMOKE_LOCK_TIMEOUT: "2" });
    expect(await second.exit).toBe(0);
    expect(existsSync(join(files, "b.ran"))).toBe(true);
    for (const pid of pids) expect(() => process.kill(pid, 0)).not.toThrow();
  });

  test("a nested run inside a held lock runs at once, leaving the holder's record, while others still wait", async () => {
    const { state, files } = fixture();
    const outer = start(state, [process.execPath, script, ...holder(files, "inner")], {}, "smoke:outer");
    await until(() => existsSync(join(files, "inner.started")), "the nested command", outer.stderr);
    const owner = JSON.parse(readFileSync(join(state, "owner"), "utf8"));
    expect(owner.pid).toBe(outer.child.pid);
    expect(owner.task).toBe("smoke:outer");
    const other = start(state, marker(files, "other.ran"));
    await until(() => other.stderr().includes("Waiting for the host smoke lock"), "the other run's waiting message", other.stderr);
    expect(existsSync(join(files, "other.ran"))).toBe(false);
    writeFileSync(join(files, "inner.release"), "");
    expect(await outer.exit).toBe(0);
    expect(await other.exit).toBe(0);
    expect(existsSync(join(files, "other.ran"))).toBe(true);
  });

  test("a nested run that lost the held marker fails at once instead of waiting on its ancestor", async () => {
    const { state, files } = fixture();
    const started = Date.now();
    const outer = start(state, ["env", "-u", "HUTERM_HOST_SMOKE_LOCK_HELD", process.execPath, script, ...marker(files, "inner.ran")]);
    expect(await outer.exit).toBe(1);
    expect(outer.stderr()).toContain("held by an ancestor of this run");
    expect(existsSync(join(files, "inner.ran"))).toBe(false);
    expect(Date.now() - started).toBeLessThan(5_000);
  });

  test("a held marker for another lock still waits, then times out naming the holder", async () => {
    const { state, files } = fixture();
    const first = start(state, holder(files, "a"), {}, "smoke:first");
    await until(() => existsSync(join(files, "a.started")), "the first command", first.stderr);
    const second = start(state, marker(files, "b.ran"), { HUTERM_HOST_SMOKE_LOCK_HELD: join(files, "elsewhere"), HUTERM_HOST_SMOKE_LOCK_TIMEOUT: "0.5" });
    expect(await second.exit).toBe(75);
    expect(second.stderr()).toContain("timed out after 0.5 seconds waiting for smoke:first");
    expect(existsSync(join(files, "b.ran"))).toBe(false);
    writeFileSync(join(files, "a.release"), "");
    expect(await first.exit).toBe(0);
  });

  test("an interrupted wait exits without running its command", async () => {
    const { state, files } = fixture();
    const first = start(state, holder(files, "a"));
    await until(() => existsSync(join(files, "a.started")), "the first command", first.stderr);
    const second = start(state, marker(files, "b.ran"));
    await until(() => second.stderr().includes("Waiting for the host smoke lock"), "the waiting message", second.stderr);
    second.child.kill("SIGINT");
    expect(await second.exit).toBe(130);
    expect(second.stderr()).toContain("interrupted");
    expect(existsSync(join(files, "b.ran"))).toBe(false);
    writeFileSync(join(files, "a.release"), "");
    expect(await first.exit).toBe(0);
  });

  test("a terminated holder keeps the lock until its command finishes, then exits with the command's status", async () => {
    const { state, files } = fixture();
    const first = start(state, ["sh", "-c", `trap 'touch "$1/term"; while [ ! -e "$1/finish" ]; do sleep 0.05; done; exit 7' TERM; touch "$1/a.started"; while :; do sleep 0.05; done`, "sh", files]);
    await until(() => existsSync(join(files, "a.started")), "the first command", first.stderr);
    first.child.kill("SIGTERM");
    await until(() => existsSync(join(files, "term")), "the forwarded SIGTERM", first.stderr);
    const probe = start(state, marker(files, "probe.ran"), { HUTERM_HOST_SMOKE_LOCK_TIMEOUT: "0" });
    expect(await probe.exit).toBe(75);
    writeFileSync(join(files, "finish"), "");
    expect(await first.exit).toBe(7);
    expect(existsSync(join(state, "owner"))).toBe(false);
  });

  test("a stale holder record is not reported as the holder", async () => {
    const { state, files } = fixture();
    const first = start(state, holder(files, "a"), {}, "smoke:first");
    await until(() => existsSync(join(files, "a.started")), "the first command", first.stderr);
    const gone = Bun.spawn(["true"]);
    await gone.exited;
    writeFileSync(join(state, "owner"), JSON.stringify({ pid: gone.pid, task: "smoke:stale", worktree: "/stale", started: "then" }));
    const second = start(state, marker(files, "b.ran"), { HUTERM_HOST_SMOKE_LOCK_TIMEOUT: "0" });
    expect(await second.exit).toBe(75);
    expect(second.stderr()).toContain("an unknown holder");
    expect(second.stderr()).not.toContain("smoke:stale");
    writeFileSync(join(files, "a.release"), "");
    expect(await first.exit).toBe(0);
  });
});

describe("the wrapped command", () => {
  // Inside a held lock, or off macOS, the wrapper runs its command directly.
  const passthrough = (state: string) => ({ HUTERM_HOST_SMOKE_LOCK_HELD: state });

  test("passes through its exit status and the signal that ended it", async () => {
    const { state } = fixture();
    expect(await start(state, ["sh", "-c", "exit 3"], passthrough(state)).exit).toBe(3);
    expect(await start(state, ["sh", "-c", "kill -TERM $$"], passthrough(state)).exit).toBe(143);
    expect(existsSync(join(state, "owner"))).toBe(false);
  });

  test("is required, and the timeout must be a non-negative number of seconds", async () => {
    const { state } = fixture();
    const missing = start(state, []);
    expect(await missing.exit).toBe(2);
    expect(missing.stderr()).toContain("usage:");
    const invalid = start(state, ["true"], { HUTERM_HOST_SMOKE_LOCK_TIMEOUT: "soon" });
    expect(await invalid.exit).toBe(2);
    expect(invalid.stderr()).toContain("HUTERM_HOST_SMOKE_LOCK_TIMEOUT");
  });
});
