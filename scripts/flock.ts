/** Non-blocking macOS `flock` for runners that coordinate across worktrees. */
import { dlopen } from "bun:ffi";
import { closeSync, openSync } from "node:fs";

let libc: ReturnType<typeof openLibc> | undefined;
function openLibc() {
  return dlopen("/usr/lib/libSystem.B.dylib", { flock: { args: ["i32", "i32"], returns: "i32" } });
}

/**
 * Kernel-owned locks release automatically if the runner dies. Bun.spawn
 * closes every other descriptor in its children, so a process a runner leaves
 * behind cannot keep holding one.
 */
export function tryLock(path: string, shared = false): (() => void) | undefined {
  libc ??= openLibc();
  const fd = openSync(path, "a");
  // LOCK_SH | LOCK_NB, or LOCK_EX | LOCK_NB
  if (libc.symbols.flock(fd, shared ? 5 : 6) !== 0) {
    closeSync(fd);
    return undefined;
  }
  return () => closeSync(fd);
}
