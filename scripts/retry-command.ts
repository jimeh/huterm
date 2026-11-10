/**
 * Runs a command up to a number of attempts, ending each attempt that outlives
 * its time limit. CI uses it where a step needs a per-attempt bound, which
 * macOS runners cannot get from `timeout`.
 *
 * usage: bun scripts/retry-command.ts --attempts <n> --timeout <seconds> -- <command> [args...]
 */
import { spawn } from "node:child_process";
import { constants } from "node:os";

export type RetryOptions = {
  attempts: number;
  timeoutMs: number;
  /** How long a timed-out attempt gets after SIGTERM before SIGKILL. */
  graceMs?: number;
  log?: (message: string) => void;
};

type Attempt = { code: number; timedOut: boolean };

const describe = (command: string[]) => command.join(" ");

const forwardedSignals: NodeJS.Signals[] = ["SIGINT", "SIGTERM", "SIGHUP"];

/**
 * Each attempt runs in its own process group, so a timeout also ends any
 * process the command started, rather than leaving it to compete with the
 * next attempt. That group no longer receives signals sent to the helper's
 * group, such as a terminal's Ctrl-C, so the helper forwards them.
 */
function runAttempt(command: string[], timeoutMs: number, graceMs: number): Promise<Attempt> {
  const [executable, ...args] = command;
  return new Promise((resolve, reject) => {
    const child = spawn(executable!, args, { stdio: "inherit", detached: true });
    let timedOut = false;
    let force: ReturnType<typeof setTimeout> | undefined;
    const signal = (value: NodeJS.Signals) => {
      try {
        process.kill(-child.pid!, value);
      } catch (error) {
        // macOS answers EPERM, not ESRCH, for a group whose members have all
        // exited but are not yet reaped.
        const code = (error as NodeJS.ErrnoException).code;
        if (code !== "ESRCH" && code !== "EPERM") throw error;
      }
    };
    const forward = (value: NodeJS.Signals) => {
      signal(value);
      process.exit(128 + constants.signals[value]);
    };
    for (const value of forwardedSignals) process.on(value, forward);
    const settle = () => {
      clearTimeout(deadline);
      clearTimeout(force);
      for (const value of forwardedSignals) process.off(value, forward);
    };
    const deadline = setTimeout(() => {
      timedOut = true;
      signal("SIGTERM");
      force = setTimeout(() => signal("SIGKILL"), graceMs);
    }, timeoutMs);
    child.once("error", (error) => {
      settle();
      reject(error);
    });
    child.once("exit", (code, signalCode) => {
      settle();
      // The command may have exited on SIGTERM while its group lives on.
      if (timedOut) signal("SIGKILL");
      resolve({ code: code ?? 128 + (signalCode ? constants.signals[signalCode] : 0), timedOut });
    });
  });
}

/** Returns 0 once an attempt succeeds, or the last attempt's exit status (124 for a timeout). */
export async function retryCommand(command: string[], options: RetryOptions): Promise<number> {
  if (!command.length) throw new Error("retry-command: the command is empty");
  if (!Number.isInteger(options.attempts) || options.attempts < 1) throw new Error("retry-command: attempts must be a positive integer");
  if (!Number.isFinite(options.timeoutMs) || options.timeoutMs <= 0) throw new Error("retry-command: the timeout must be positive");
  const log = options.log ?? ((message: string) => console.error(message));
  const seconds = options.timeoutMs / 1000;
  let last: Attempt = { code: 1, timedOut: false };
  for (let attempt = 1; attempt <= options.attempts; attempt++) {
    last = await runAttempt(command, options.timeoutMs, options.graceMs ?? 10_000);
    if (!last.timedOut && last.code === 0) return 0;
    const outcome = last.timedOut ? `timed out after ${seconds} s` : `failed with exit status ${last.code}`;
    const next = attempt < options.attempts ? "; retrying" : "";
    log(`retry-command: attempt ${attempt} of ${options.attempts} of \`${describe(command)}\` ${outcome}${next}`);
  }
  const attempts = options.attempts === 1 ? "1 attempt" : `${options.attempts} attempts`;
  // The ci-zig-install-hang known-flake signature matches "has timed out after".
  const summary = last.timedOut
    ? `has timed out after ${attempts} of ${seconds} s`
    : `failed after ${attempts}`;
  log(`retry-command: \`${describe(command)}\` ${summary}`);
  return last.timedOut ? 124 : last.code;
}

function parseArgs(argv: string[]): { command: string[]; attempts: number; timeoutMs: number } | undefined {
  const separator = argv.indexOf("--");
  if (separator < 0) return undefined;
  const flags = argv.slice(0, separator);
  const command = argv.slice(separator + 1);
  const values = new Map<string, string>();
  for (let index = 0; index < flags.length; index += 2) {
    const [flag, value] = [flags[index]!, flags[index + 1]];
    if (!["--attempts", "--timeout"].includes(flag) || value === undefined || values.has(flag)) return undefined;
    values.set(flag, value);
  }
  const attempts = Number(values.get("--attempts"));
  const timeout = Number(values.get("--timeout"));
  if (!command.length || !Number.isInteger(attempts) || attempts < 1 || !(timeout > 0)) return undefined;
  return { command, attempts, timeoutMs: timeout * 1000 };
}

if (import.meta.main) {
  const parsed = parseArgs(process.argv.slice(2));
  if (!parsed) {
    console.error("usage: bun scripts/retry-command.ts --attempts <n> --timeout <seconds> -- <command> [args...]");
    process.exit(2);
  }
  process.exit(await retryCommand(parsed.command, parsed));
}
