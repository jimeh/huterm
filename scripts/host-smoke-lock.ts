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
import { spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { constants as os, homedir } from "node:os";
import { join, resolve } from "node:path";
import { tryLock } from "./flock";

export const HELD = "HUTERM_HOST_SMOKE_LOCK_HELD";
const TIMEOUT = "HUTERM_HOST_SMOKE_LOCK_TIMEOUT";
const DEFAULT_TIMEOUT_SECONDS = 45 * 60;
const POLL_MS = 250;
/** How long processes the command leaves behind get to exit before SIGKILL. */
const STRAY_GRACE_MS = 2_000;
/** How long SIGKILLed processes get to finish exiting before the lock is released anyway. */
const KILL_GRACE_MS = 1_000;
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

/** Signals every process left in the command's process group; false once the group is empty. */
function signalGroup(leader: number, signal: NodeJS.Signals | 0): boolean {
  try {
    process.kill(-leader, signal);
    return true;
  } catch {
    return false;
  }
}

/**
 * A harness can exit, or die from a signal, while an app it launched keeps
 * running. The next holder must not share the desktop with it, so stop the
 * rest of the command's process group before releasing the lock.
 */
async function stopGroup(leader: number): Promise<void> {
  const emptied = async (graceMs: number) => {
    const deadline = performance.now() + graceMs;
    while (signalGroup(leader, 0)) {
      if (performance.now() >= deadline) return false;
      await Bun.sleep(50);
    }
    return true;
  };
  if (!signalGroup(leader, "SIGTERM") || await emptied(STRAY_GRACE_MS)) return;
  signalGroup(leader, "SIGKILL");
  // A process stuck in the kernel must not keep the lock forever.
  if (!await emptied(KILL_GRACE_MS)) console.error(`host smoke lock: process group ${leader} survived SIGKILL; releasing the lock anyway`);
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

  let child: ChildProcess | undefined;
  // A locked command leads its own process group, so a terminal's Ctrl-C
  // reaches it once, through this handler, and so does anything it started.
  // Without the lock the command stays in this group, so a nested run is
  // still inside the cleanup of the wrapper that holds the lock. A hangup
  // reaches the group as SIGTERM: most harnesses die at once on SIGHUP,
  // and runSmokeProcess, which puts its app in another group, stops that
  // group on either. It takes a SIGHUP of its own as SIGTERM, for a run
  // without a wrapper that holds the lock.
  const forward = (signal: NodeJS.Signals) => {
    interrupted ??= signal;
    if (!child?.pid) return;
    if (locking) signalGroup(child.pid, signal === "SIGHUP" ? "SIGTERM" : signal);
    else child.kill(signal);
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
    const spawned = spawn(command[0]!, command.slice(1), {
      stdio: "inherit",
      detached: locking,
      env: locking ? { ...process.env, [HELD]: directory } : process.env,
    });
    child = spawned;
    const [status, signal] = await new Promise<[number | null, NodeJS.Signals | null]>((resolve, reject) => {
      spawned.once("error", (error) => reject(new LockError(`host smoke lock: could not start ${command[0]}: ${error.message}`, 127)));
      spawned.once("exit", (code, signal) => resolve([code, signal]));
    });
    if (locking) await stopGroup(spawned.pid!);
    return signal ? 128 + os.signals[signal] : status ?? 1;
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
