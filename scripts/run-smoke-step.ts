/**
 * Supervise one CI smoke step, preserving evidence even if its harness hangs.
 *
 * A failure that matches a registered known flake (scripts/flakes.ts) is rerun
 * once when the registry allows it and the step's time budget leaves room for
 * another attempt. Each retry keeps its own evidence, so the first attempt's
 * files survive for triage.
 */
import { join } from "node:path";
import {
  classify, currentPlatform, decideRetry, loadFlakes, reportFailure, reportRetry, retriedEntries, retryEnabled,
  type Flake, type Platform,
} from "./flakes.ts";
import { checkSmokeProcess, runSmokeProcess, type SmokeOutcome } from "./smoke-process.ts";

const steps = new Set([
  "renderer", "presentation-queries", "linux-desktop", "linux-input", "linux-palette", "linux-overlays", "linux-viewers", "linux-settings", "linux-client-frame", "linux-integration",
  "linux-clipboard", "linux-fullscreen", "linux-quake", "linux-restore", "macos-menus", "macos-updater", "macos-input",
  "macos-clipboard", "macos-refresh",
  "macos-palette", "macos-overlays", "macos-viewers", "macos-settings", "macos-titlebar", "macos-integration", "macos-quit", "macos-fullscreen", "macos-quake", "macos-restore",
]);

/** One attempt's limit. The Actions step allows 6 minutes, which also covers the host-lock wait. */
export const ATTEMPT_TIMEOUT_MS = 300_000;
/** Inner native supervisors force cleanup after one second; let them finish. */
export const GRACE_MS = 5_000;
/** Time for every attempt together, from the supervisor's start. */
export const STEP_BUDGET_MS = 330_000;
/** Time kept back for cleanup and reporting after the last attempt. */
export const RESERVE_MS = 10_000;
/** A retry needs at least this long, and at least twice the failed attempt's duration. */
export const MIN_RETRY_MS = 30_000;

export type AttemptRunner = (attempt: number, options: { timeoutMs: number; evidenceDir?: string; env: NodeJS.ProcessEnv }) => Promise<SmokeOutcome>;

/** The text a smoke failure is classified on: the error, then both streams. */
export function failureText(error: string, outcome: SmokeOutcome): string {
  return `error: ${error}\n--- stdout ---\n${outcome.stdout}\n--- stderr ---\n${outcome.stderr}\n`;
}

function failure(outcome: SmokeOutcome, label: string): string | undefined {
  try {
    checkSmokeProcess(outcome, label);
    return undefined;
  } catch (error) {
    return (error as Error).message;
  }
}

/**
 * Run `step` until it passes, or until a failure is unmatched, exhausts its
 * retries, or no longer fits the time budget. Throws the final failure.
 */
export async function superviseSmokeStep(options: {
  step: string;
  platform: Platform | undefined;
  flakes: Flake[];
  run: AttemptRunner;
  env?: NodeJS.ProcessEnv;
  evidence?: string;
  budgetMs?: number;
  clock?: () => number;
}): Promise<{ attempts: number; retried?: string }> {
  const env = options.env ?? process.env;
  const clock = options.clock ?? (() => performance.now());
  const budget = options.budgetMs ?? STEP_BUDGET_MS;
  const started = clock();
  const label = `smoke step ${options.step}`;
  let retried: string | undefined;
  let overdue = false;
  for (let attempt = 1; ; attempt++) {
    const remaining = budget - (clock() - started) - GRACE_MS - RESERVE_MS;
    const timeoutMs = Math.min(ATTEMPT_TIMEOUT_MS, remaining);
    // The first attempt keeps the established evidence paths; retries get their own.
    const evidenceDir = options.evidence
      ? attempt === 1 ? join(options.evidence, "steps", options.step) : join(options.evidence, "steps", options.step, `attempt-${attempt}`)
      : undefined;
    const attemptEnv = attempt === 1 || !options.evidence
      ? env
      : { ...env, HUTERM_SMOKE_EVIDENCE_DIR: join(options.evidence, "retries", `${options.step}-attempt-${attempt}`) };
    const attemptStarted = clock();
    const outcome = await options.run(attempt, { timeoutMs, evidenceDir, env: attemptEnv });
    const elapsed = clock() - attemptStarted;
    const error = failure(outcome, label);
    const selector = `smoke:${options.step}`;
    if (retried) {
      // A rerun counts only if it passes, or fails with the same signature.
      const again = error && options.platform
        ? classify(options.flakes, { kind: "smoke", platform: options.platform, step: options.step, text: failureText(error, outcome) })
        : undefined;
      reportRetry({ event: "retried", flake: retried, kind: "smoke", selector, platform: options.platform ?? "linux-x86_64", attempt: attempt - 1, passed: !error, overdue }, env);
      if (!error) return { attempts: attempt, retried };
      if (again?.id !== retried) throw new Error(`${error} (rerun of known flake ${retried} failed with a different signature)`);
    } else if (!error) {
      return { attempts: attempt };
    }
    const text = failureText(error, outcome);
    const flake = options.platform ? classify(options.flakes, { kind: "smoke", platform: options.platform, step: options.step, text }) : undefined;
    let decision = decideRetry(flake, { attempt, retriedEntries: retriedEntries(env), enabled: retryEnabled(env) });
    if (outcome.interrupted) decision = { retry: false, flake, reason: `the supervisor received ${outcome.interrupted}` };
    const nextRemaining = budget - (clock() - started) - GRACE_MS - RESERVE_MS;
    if (decision.retry && nextRemaining < Math.max(MIN_RETRY_MS, 2 * elapsed)) {
      decision = { retry: false, flake, reason: `${decision.flake.id} matches, but ${Math.round(nextRemaining / 1000)}s left in the step budget cannot fit another ${Math.round(elapsed / 1000)}s attempt` };
    }
    if (!decision.retry) {
      if (options.platform) {
        reportFailure({ event: "failed", flake: decision.flake?.id, kind: "smoke", selector, platform: options.platform, attempt, reason: decision.flake ? decision.reason : undefined, excerpt: error }, env);
      }
      throw new Error(error);
    }
    console.error(`flakes: ${label} failed with the signature of known flake ${decision.flake.id}${decision.overdue ? " (review overdue)" : ""}; retrying once (docs/agents/flakes/${decision.flake.id}.md)`);
    retried = decision.flake.id;
    overdue = decision.overdue;
  }
}

if (import.meta.main) {
  const step = process.env.HUTERM_CI_SMOKE_STEP;
  if (!step || !steps.has(step)) throw new Error(`expected one named smoke step, received ${step}`);
  const { flakes, errors } = loadFlakes();
  // A broken registry must not block smokes; flakes:check reports it.
  for (const error of errors) console.error(`flakes: ignoring invalid entry: ${error}`);
  await superviseSmokeStep({
    step,
    platform: currentPlatform(),
    flakes,
    evidence: process.env.HUTERM_SMOKE_EVIDENCE_DIR,
    run: (_attempt, { timeoutMs, evidenceDir, env }) => runSmokeProcess(["mise", "run", "ci:smoke:run"], { timeoutMs, graceMs: GRACE_MS, evidenceDir, env }),
  });
}
