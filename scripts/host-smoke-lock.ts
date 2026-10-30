/**
 * Serialize host desktop smokes across worktrees: they share the real display,
 * pointer, keyboard, focus, and global shortcuts, so two at once fail like flakes.
 *
 *   bun scripts/host-smoke-lock.ts [--] <command...>
 *
 * Waits for one per-user kernel lock, records its holder beside it, and runs the
 * command while holding it. The command inherits HUTERM_HOST_SMOKE_LOCK_HELD, so
 * a nested wrapper runs its command directly instead of waiting on its ancestor.
 * Off macOS the command runs without a lock; Linux smokes use private displays.
 */
import { dlopen } from "bun:ffi";
import { closeSync, mkdirSync, openSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { constants as os, homedir } from "node:os";
import { join, resolve } from "node:path";

export const HELD = "HUTERM_HOST_SMOKE_LOCK_HELD";
const TIMEOUT = "HUTERM_HOST_SMOKE_LOCK_TIMEOUT";
const DEFAULT_TIMEOUT_SECONDS = 45 * 60;
const POLL_MS = 250;
const worktree = resolve(import.meta.dir, "..");

class LockError extends Error {
  constructor(message: string, readonly status: number) { super(message); }
}

export function stateDirectory(env = process.env): string {
  return resolve(env.HUTERM_HOST_SMOKE_STATE ?? join(homedir(), "Library/Caches/huterm-host-smoke"));
}

export function timeoutSeconds(value: string | undefined): number {
  if (value === undefined || value === "") return DEFAULT_TIMEOUT_SECONDS;
  const seconds = Number(value);
  if (!Number.isFinite(seconds) || seconds < 0) throw new LockError(`${TIMEOUT} must be a non-negative number of seconds, not ${JSON.stringify(value)}`, 2);
  return seconds;
}

const duration = (seconds: number) => seconds >= 60 && seconds % 60 === 0 ? `${seconds / 60} minutes` : `${seconds} seconds`;

let libc: ReturnType<typeof openLibc> | undefined;
function openLibc() {
  return dlopen("/usr/lib/libSystem.B.dylib", { flock: { args: ["i32", "i32"], returns: "i32" } });
}

/**
 * Kernel-owned locks release automatically if the holder dies. Bun.spawn
 * closes every other descriptor in its children, so a daemon a smoke leaves
 * behind, such as a tmux server, cannot keep holding it.
 */
function tryLock(path: string): (() => void) | undefined {
  libc ??= openLibc();
  const fd = openSync(path, "a");
  // LOCK_EX | LOCK_NB
  if (libc.symbols.flock(fd, 6) !== 0) {
    closeSync(fd);
    return undefined;
  }
  return () => closeSync(fd);
}

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

function isAncestor(pid: number): boolean {
  const listing = Bun.spawnSync(["ps", "-axo", "pid=,ppid="], { stdout: "pipe", stderr: "ignore" }).stdout.toString();
  const parents = new Map(listing.trim().split("\n").map((line) => line.trim().split(/\s+/).map(Number) as [number, number]));
  for (let current = parents.get(process.pid); current && current > 1; current = parents.get(current)) {
    if (current === pid) return true;
  }
  return false;
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
    if (key !== announced) {
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
  const command = argv[0] === "--" ? argv.slice(1) : argv;
  if (command.length === 0) throw new LockError("usage: bun scripts/host-smoke-lock.ts [--] <command...>", 2);
  const timeout = timeoutSeconds(process.env[TIMEOUT]);
  const directory = stateDirectory();
  const locking = process.platform === "darwin" && process.env[HELD] !== directory;

  let child: ReturnType<typeof Bun.spawn> | undefined;
  // The child usually shares the terminal's process group and receives a
  // terminal signal directly too; a repeat is harmless to the smoke harnesses.
  const forward = (signal: NodeJS.Signals) => {
    interrupted ??= signal;
    child?.kill(signal);
  };
  const handlers = (["SIGINT", "SIGTERM", "SIGHUP"] as const).map((signal) => [signal, () => forward(signal)] as const);
  for (const [signal, handler] of handlers) process.on(signal, handler);
  let release: (() => void) | undefined;
  try {
    if (locking) {
      mkdirSync(directory, { recursive: true });
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
