/** Run Huterm against the echo workload and summarize its output latency probe. */
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { runSmokeProcess } from "./smoke-process.ts";

const PREFIX = "huterm-render output ";

export type Interval = { elapsedUs: number; snapshots: number; appliedMedianUs: number; appliedMaxUs: number; paintedMedianUs: number; paintedMaxUs: number };

export function parseIntervals(log: string): Interval[] {
  return log.split(/\r?\n/).filter(line => line.startsWith(PREFIX)).map(line => {
    const field = (name: string) => {
      const match = line.match(new RegExp(`(?:^| )${name}=([0-9]+)(?: |$)`));
      if (!match) throw new Error(`output latency line is missing ${name}: ${line}`);
      return Number(match[1]);
    };
    return { elapsedUs: field("elapsed_us"), snapshots: field("snapshots"), appliedMedianUs: field("applied_us_median"), appliedMaxUs: field("applied_us_max"), paintedMedianUs: field("painted_us_median"), paintedMaxUs: field("painted_us_max") };
  });
}

function median(values: number[]): number {
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.floor(sorted.length / 2)] ?? 0;
}

export type Summary = { intervals: number; snapshotsPerSecond: number; appliedMedianUs: number; appliedMaxUs: number; paintedMedianUs: number; paintedMaxUs: number };

/** The first interval includes startup, so it is always excluded. */
export function summarize(intervals: Interval[]): Summary {
  const steady = intervals.slice(1);
  if (steady.length === 0) throw new Error("Huterm printed no output latency intervals");
  const snapshots = steady.reduce((total, interval) => total + interval.snapshots, 0);
  // Intervals close at the first paint after one second, so their length varies.
  const elapsedSeconds = steady.reduce((total, interval) => total + interval.elapsedUs, 0) / 1_000_000;
  return {
    intervals: steady.length,
    snapshotsPerSecond: Math.round(snapshots / elapsedSeconds),
    appliedMedianUs: median(steady.map(interval => interval.appliedMedianUs)),
    appliedMaxUs: Math.max(...steady.map(interval => interval.appliedMaxUs)),
    paintedMedianUs: median(steady.map(interval => interval.paintedMedianUs)),
    paintedMaxUs: Math.max(...steady.map(interval => interval.paintedMaxUs)),
  };
}

export function format(summary: Summary): string {
  return `output-latency intervals=${summary.intervals} snapshots_per_second=${summary.snapshotsPerSecond} applied_us_median=${summary.appliedMedianUs} applied_us_max=${summary.appliedMaxUs} painted_us_median=${summary.paintedMedianUs} painted_us_max=${summary.paintedMaxUs}`;
}

/**
 * Gates the activity-driven snapshot path. Without it, an isolated update
 * waits for the 16 ms refresh pump and the median sits near 8 to 12 ms on
 * every host measured; with it, the median is well under 1 ms.
 */
export function checkAppliedBudget(summary: Summary, budgetUs: number): void {
  if (summary.appliedMedianUs > budgetUs) {
    throw new Error(`applied_us_median=${summary.appliedMedianUs} exceeds the budget of ${budgetUs} µs: output is not reaching a snapshot until the refresh pump`);
  }
}

if (import.meta.main) {
  const [executable, workload, mode = "echo", seconds = "8"] = Bun.argv.slice(2);
  if (!executable || !workload) throw new Error("usage: run-output-latency.ts <huterm> <render_workload> [echo|flood] [seconds]");
  if (mode !== "echo" && mode !== "flood") throw new Error(`mode must be echo or flood, not ${mode}`);
  const timeoutMs = Number(seconds) * 1_000;
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) throw new Error(`seconds must be a positive number, not ${seconds}`);
  const budgetValue = process.env.HUTERM_OUTPUT_LATENCY_APPLIED_BUDGET_US;
  const budgetUs = budgetValue === undefined ? undefined : Number(budgetValue);
  if (budgetUs !== undefined && (!Number.isFinite(budgetUs) || budgetUs <= 0)) throw new Error(`HUTERM_OUTPUT_LATENCY_APPLIED_BUDGET_US must be a positive number, not ${budgetValue}`);
  // The terminal starts its shell from another directory, so the path must be absolute.
  // An empty configuration keeps the user's font, theme, and global shortcuts
  // out of the measurement; a running Huterm would otherwise own the shortcuts.
  const configDirectory = await mkdtemp(join(tmpdir(), "huterm-output-latency-"));
  const config = join(configDirectory, "config.toml");
  await Bun.write(config, "");
  // "events" records the probe without requesting a frame per display tick;
  // continuous drawing would otherwise hold the main thread in present.
  // Always set the workload so an inherited value cannot change the mode.
  const environment: NodeJS.ProcessEnv = { ...process.env, SHELL: resolve(workload), HUTERM_CONFIG_FILE: config, HUTERM_RENDER_STATS: "events", HUTERM_RENDER_WORKLOAD: mode };
  try {
    // Huterm runs until stopped, so reaching the deadline is the expected outcome.
    const outcome = await runSmokeProcess([executable], { timeoutMs, env: environment, stream: false });
    if (!outcome.timedOut) throw new Error(`Huterm exited early (${outcome.exitCode ?? outcome.signalCode})\n${outcome.stderr}`);
    for (const line of outcome.stderr.split(/\r?\n/).filter(line => line.startsWith("HUTERM_BENCH "))) console.log(line);
    const summary = summarize(parseIntervals(outcome.stderr));
    console.log(`mode=${mode} ${format(summary)}`);
    if (budgetUs !== undefined) {
      if (mode !== "echo") throw new Error("the applied latency budget applies to echo mode only");
      checkAppliedBudget(summary, budgetUs);
      console.log(`mode=${mode} applied_budget_us=${budgetUs} passed`);
    }
  } finally {
    await rm(configDirectory, { recursive: true, force: true });
  }
}
