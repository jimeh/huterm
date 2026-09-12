/** Bounded native smoke supervision with live output and durable timing evidence. */
import { spawn, type ChildProcessByStdio } from "node:child_process";
import { appendFileSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Readable } from "node:stream";

export type SmokeOutcome = {
  exitCode: number | null;
  signalCode: string | null;
  timedOut: boolean;
  stdout: string;
  stderr: string;
  elapsedMs: number;
};

export async function runSmokeProcess(command: string[], options: {
  timeoutMs: number;
  graceMs?: number;
  evidenceDir?: string;
  env?: NodeJS.ProcessEnv;
  stream?: boolean;
}): Promise<SmokeOutcome> {
  const [executable, ...args] = command;
  if (!executable) throw new Error("smoke command is empty");
  if (!Number.isFinite(options.timeoutMs) || options.timeoutMs <= 0) throw new Error("invalid smoke timeout");
  const started = performance.now();
  const elapsed = () => Math.round(performance.now() - started);
  const directory = options.evidenceDir;
  if (directory) {
    mkdirSync(directory, { recursive: true });
    rmSync(join(directory, "outcome.json"), { force: true });
    for (const name of ["stdout.log", "stderr.log", "events.jsonl"]) writeFileSync(join(directory, name), "");
  }
  const event = (kind: string, details: object = {}) => {
    if (directory) appendFileSync(join(directory, "events.jsonl"), `${JSON.stringify({ elapsedMs: elapsed(), kind, ...details })}\n`);
  };
  let child: ChildProcessByStdio<null, Readable, Readable> | undefined;
  let stdout = "";
  let stderr = "";
  let timedOut = false;
  let interrupted: NodeJS.Signals | null = null;
  let force: ReturnType<typeof setTimeout> | undefined;
  let deadline: ReturnType<typeof setTimeout> | undefined;
  const signal = (value: NodeJS.Signals) => {
    if (!child?.pid) return;
    try {
      if (process.platform === "win32") child.kill(value);
      else process.kill(-child.pid, value);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error;
    }
  };
  const terminate = (value: NodeJS.Signals) => {
    signal(value);
    if (!force) force = setTimeout(() => { event("force-kill"); signal("SIGKILL"); }, options.graceMs ?? 1_000);
  };
  const onTerm = () => { interrupted = "SIGTERM"; event("interrupted", { signal: interrupted }); terminate(interrupted); };
  const onInt = () => { interrupted = "SIGINT"; event("interrupted", { signal: interrupted }); terminate(interrupted); };
  // A child can signal us as soon as spawn starts it. Install handlers first.
  process.on("SIGTERM", onTerm);
  process.on("SIGINT", onInt);
  try {
    // A private process group lets the deadline also stop Xvfb, window managers,
    // and fixture helpers when a harness is stuck in its own cleanup.
    const spawned = spawn(executable, args, {
      detached: process.platform !== "win32",
      stdio: ["ignore", "pipe", "pipe"],
      env: options.env ?? process.env,
    });
    child = spawned;
    event("started", { command, pid: child.pid, timeoutMs: options.timeoutMs });
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
      if (options.stream !== false) process.stdout.write(chunk);
      if (directory) appendFileSync(join(directory, "stdout.log"), chunk);
      event("stdout", { text: chunk });
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
      if (options.stream !== false) process.stderr.write(chunk);
      if (directory) appendFileSync(join(directory, "stderr.log"), chunk);
      event("stderr", { text: chunk });
    });
    deadline = setTimeout(() => {
      timedOut = true;
      event("deadline");
      terminate("SIGTERM");
    }, options.timeoutMs);
    const status = await new Promise<{ exitCode: number | null; signalCode: string | null }>((resolve, reject) => {
      spawned.once("error", reject);
      spawned.once("exit", (exitCode, signalCode) => event("exit", { exitCode, signalCode }));
      // Wait for output as well as exit; descendants holding pipes are also bounded.
      spawned.once("close", (exitCode, signalCode) => resolve({ exitCode, signalCode }));
    });
    const outcome = { ...status, signalCode: interrupted ?? status.signalCode, timedOut, stdout, stderr, elapsedMs: elapsed() };
    event("closed", status);
    if (directory) writeFileSync(join(directory, "outcome.json"), `${JSON.stringify({ exitCode: outcome.exitCode, signalCode: outcome.signalCode, timedOut, elapsedMs: outcome.elapsedMs }, null, 2)}\n`);
    return outcome;
  } catch (error) {
    event("error", { message: String(error) });
    throw error;
  } finally {
    process.off("SIGTERM", onTerm);
    process.off("SIGINT", onInt);
    clearTimeout(deadline);
    clearTimeout(force);
    signal("SIGKILL");
  }
}

export function checkSmokeProcess(outcome: SmokeOutcome, label: string): asserts outcome is SmokeOutcome & { exitCode: 0 } {
  if (outcome.timedOut) throw new Error(`${label} timed out after ${outcome.elapsedMs}ms`);
  if (outcome.signalCode !== null) throw new Error(`${label} terminated by ${outcome.signalCode}`);
  if (outcome.exitCode !== 0) throw new Error(`${label} exited ${outcome.exitCode}`);
}
