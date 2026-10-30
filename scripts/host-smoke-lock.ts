/**
 * Serialize host desktop smokes across worktrees: they share the real display,
 * pointer, keyboard, focus, and global shortcuts, so two at once fail like flakes.
 *
 *   bun scripts/host-smoke-lock.ts [--] <command...>
 *   bun scripts/host-smoke-lock.ts --held
 *
 * Waits for one per-user kernel lock, records its holder beside it, and runs the
 * command while holding it. The command inherits HUTERM_HOST_SMOKE_LOCK_HELD, so
 * a nested wrapper runs its command directly instead of waiting on its ancestor.
 * Off macOS the command runs without a lock; Linux smokes use private displays.
 * `--held` exits 0 when this process needs no lock of its own, for a shell
 * script that reruns itself under the wrapper.
 */
import { mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { constants as os, homedir } from "node:os";
import { join, resolve } from "node:path";
import { tryLock } from "./flock";

export const HELD = "HUTERM_HOST_SMOKE_LOCK_HELD";
const TIMEOUT = "HUTERM_HOST_SMOKE_LOCK_TIMEOUT";
const DEFAULT_TIMEOUT_SECONDS = 45 * 60;
const POLL_MS = 250;
/** How long processes a cancelled command leaves behind get to exit before SIGKILL. */
const STRAY_GRACE_MS = 2_000;
const worktree = resolve(import.meta.dir, "..");

class LockError extends Error {
  constructor(message: string, readonly status: number) { super(message); }
}

export function stateDirectory(env = process.env): string {
  return resolve(env.HUTERM_HOST_SMOKE_STATE ?? join(homedir(), "Library/Caches/huterm-host-smoke"));
}

export function timeoutSeconds(value: string | undefined): number {
  if (value === undefined || value.trim() === "") return DEFAULT_TIMEOUT_SECONDS;
  const seconds = Number(value);
  if (!Number.isFinite(seconds) || seconds < 0) throw new LockError(`${TIMEOUT} must be a non-negative number of seconds, not ${JSON.stringify(value)}`, 2);
  return seconds;
}

const duration = (seconds: number) => seconds >= 60 && seconds % 60 === 0 ? `${seconds / 60} minutes` : `${seconds} seconds`;

type Owner = { pid: number; task: string; step?: string; worktree: string; started: string };

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

/** The record names the holder only while its process lives; a killed holder leaves its record behind. */
function currentOwner(path: string): Owner | undefined {
  try {
    const owner = JSON.parse(readFileSync(path, "utf8")) as Owner;
    return Number.isInteger(owner.pid) && alive(owner.pid) ? owner : undefined;
  } catch {
    return undefined;
  }
}

function describeOwner(owner: Owner | undefined): string {
  if (!owner) return "an unknown holder";
  const task = owner.step ? `${owner.task} (step ${owner.step})` : owner.task;
  return `${task} in ${owner.worktree} (PID ${owner.pid}, since ${owner.started})`;
}

/** Each process's parent, or none where the process table is unreadable, as in some sandboxes. */
function parents(): Map<number, number> {
  try {
    const listing = Bun.spawnSync(["ps", "-axo", "pid=,ppid="], { stdout: "pipe", stderr: "ignore" });
    if (listing.exitCode !== 0) return new Map();
    return new Map(listing.stdout.toString().trim().split("\n").map((line) => line.trim().split(/\s+/).map(Number) as [number, number]));
  } catch {
    return new Map();
  }
}

function isAncestor(pid: number): boolean {
  const table = parents();
  for (let current = table.get(process.pid); current && current > 1; current = table.get(current)) {
    if (current === pid) return true;
  }
  return false;
}

function descendants(root: number): number[] {
  const children = new Map<number, number[]>();
  for (const [pid, parent] of parents()) children.set(parent, [...(children.get(parent) ?? []), pid]);
  const found: number[] = [];
  for (let queue = [root]; queue.length > 0; ) {
    for (const pid of children.get(queue.shift()!) ?? []) {
      found.push(pid);
      queue.push(pid);
    }
  }
  return found;
}

/**
 * A harness killed by a signal can orphan the app it launched, which would
 * then share the desktop with the next holder. Stop whatever is left.
 */
async function stopStrays(pids: Iterable<number>): Promise<void> {
  const strays = [...pids].filter(alive);
  for (const pid of strays) { try { process.kill(pid, "SIGTERM"); } catch {} }
  const deadline = performance.now() + STRAY_GRACE_MS;
  while (strays.some(alive) && performance.now() < deadline) await Bun.sleep(50);
  for (const pid of strays.filter(alive)) { try { process.kill(pid, "SIGKILL"); } catch {} }
}

let interrupted: NodeJS.Signals | undefined;

async function acquire(directory: string, timeout: number): Promise<() => void> {
  const lock = join(directory, "lock");
  const deadline = performance.now() + timeout * 1000;
  let announced: string | undefined;
  while (true) {
    const release = tryLock(lock);
    if (release) return release;
    const owner = currentOwner(join(directory, "owner"));
    const holder = describeOwner(owner);
    if (interrupted) throw new LockError(`host smoke lock: interrupted while waiting for ${holder}`, 128 + os.signals[interrupted]);
    if (performance.now() >= deadline) throw new LockError(`host smoke lock: timed out after ${duration(timeout)} waiting for ${holder}`, 75);
    const key = owner ? `${owner.pid} ${owner.started}` : "unknown";
    // A handoff briefly leaves no record; that is not a new holder.
    if (key !== announced && !(key === "unknown" && announced)) {
      if (owner && isAncestor(owner.pid)) {
        throw new LockError(`host smoke lock: held by an ancestor of this run, ${holder}, but ${HELD} did not reach this process; pass the environment through`, 1);
      }
      console.error(`Waiting for the host smoke lock held by ${holder}. Giving up after ${duration(timeout)}; set ${TIMEOUT} in seconds to change that.`);
      announced = key;
    }
    await Bun.sleep(Math.min(POLL_MS, Math.max(0, deadline - performance.now())));
  }
}

function publishOwner(path: string, command: string[]): void {
  const owner: Owner = {
    pid: process.pid,
    task: process.env.MISE_TASK_NAME || command.join(" "),
    ...(process.env.HUTERM_CI_SMOKE_STEP ? { step: process.env.HUTERM_CI_SMOKE_STEP } : {}),
    worktree,
    started: new Date().toISOString(),
  };
  const temporary = `${path}.${process.pid}`;
  writeFileSync(temporary, `${JSON.stringify(owner)}\n`);
  renameSync(temporary, path);
}

async function main(argv: string[]): Promise<number> {
  if (argv[0] === "--held") return process.platform !== "darwin" || process.env[HELD] === stateDirectory() ? 0 : 1;
  const command = argv[0] === "--" ? argv.slice(1) : argv;
  if (command.length === 0) throw new LockError("usage: bun scripts/host-smoke-lock.ts [--] <command...>", 2);
  const timeout = timeoutSeconds(process.env[TIMEOUT]);
  const directory = stateDirectory();
  const locking = process.platform === "darwin" && process.env[HELD] !== directory;

  let child: ReturnType<typeof Bun.spawn> | undefined;
  const strays = new Set<number>();
  // The child usually shares the terminal's process group and receives a
  // terminal signal directly too; a repeat is harmless to the smoke harnesses.
  // Its descendants are recorded first: once it exits, orphans lose their parent.
  const forward = (signal: NodeJS.Signals) => {
    interrupted ??= signal;
    if (!child) return;
    for (const pid of descendants(child.pid)) strays.add(pid);
    child.kill(signal);
  };
  const handlers = (["SIGINT", "SIGTERM", "SIGHUP"] as const).map((signal) => [signal, () => forward(signal)] as const);
  for (const [signal, handler] of handlers) process.on(signal, handler);
  let release: (() => void) | undefined;
  try {
    if (locking) {
      try {
        mkdirSync(directory, { recursive: true });
      } catch (error) {
        throw new LockError(`host smoke lock: cannot create ${directory}; set HUTERM_HOST_SMOKE_STATE to a writable directory: ${error instanceof Error ? error.message : String(error)}`, 1);
      }
      const unlock = await acquire(directory, timeout);
      const owner = join(directory, "owner");
      publishOwner(owner, command);
      // Remove the record before unlocking so it never deletes the next holder's.
      release = () => { rmSync(owner, { force: true }); unlock(); };
      if (interrupted) throw new LockError(`host smoke lock: interrupted before starting ${command.join(" ")}`, 128 + os.signals[interrupted]);
    }
    try {
      child = Bun.spawn(command, { stdio: ["inherit", "inherit", "inherit"], env: locking ? { ...process.env, [HELD]: directory } : process.env });
    } catch (error) {
      throw new LockError(`host smoke lock: could not start ${command[0]}: ${error instanceof Error ? error.message : String(error)}`, 127);
    }
    const status = await child.exited;
    await stopStrays(strays);
    const signal = child.signalCode;
    if (signal === null) return status;
    return 128 + (typeof signal === "number" ? signal : os.signals[signal]);
  } finally {
    release?.();
    for (const [signal, handler] of handlers) process.off(signal, handler);
  }
}

if (import.meta.main) {
  try {
    process.exitCode = await main(Bun.argv.slice(2));
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = error instanceof LockError ? error.status : 1;
  }
}
