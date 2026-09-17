/** Run Huterm against the echo workload and summarize its output latency probe. */
import { resolve } from "node:path";
import { runSmokeProcess } from "./smoke-process.ts";

const PREFIX = "huterm-render output ";

export type Interval = { snapshots: number; appliedMedianUs: number; appliedMaxUs: number; paintedMedianUs: number; paintedMaxUs: number };

export function parseIntervals(log: string): Interval[] {
  return log.split(/\r?\n/).filter(line => line.startsWith(PREFIX)).map(line => {
    const field = (name: string) => {
      const match = line.match(new RegExp(`(?:^| )${name}=([0-9]+)(?: |$)`));
      if (!match) throw new Error(`output latency line is missing ${name}: ${line}`);
      return Number(match[1]);
    };
    return { snapshots: field("snapshots"), appliedMedianUs: field("applied_us_median"), appliedMaxUs: field("applied_us_max"), paintedMedianUs: field("painted_us_median"), paintedMaxUs: field("painted_us_max") };
  });
}

function median(values: number[]): number {
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.floor(sorted.length / 2)] ?? 0;
}

/** The first interval includes startup, so it is excluded when others exist. */
export function summarize(intervals: Interval[]): string {
  const steady = intervals.length > 1 ? intervals.slice(1) : intervals;
  if (steady.length === 0) throw new Error("Huterm printed no output latency intervals");
  const snapshots = steady.reduce((total, interval) => total + interval.snapshots, 0);
  return `output-latency intervals=${steady.length} snapshots_per_second=${Math.round(snapshots / steady.length)} applied_us_median=${median(steady.map(interval => interval.appliedMedianUs))} applied_us_max=${Math.max(...steady.map(interval => interval.appliedMaxUs))} painted_us_median=${median(steady.map(interval => interval.paintedMedianUs))} painted_us_max=${Math.max(...steady.map(interval => interval.paintedMaxUs))}`;
}

if (import.meta.main) {
  const [executable, workload, mode = "echo", seconds = "8"] = Bun.argv.slice(2);
  if (!executable || !workload) throw new Error("usage: run-output-latency.ts <huterm> <render_workload> [echo|flood] [seconds]");
  // The terminal starts its shell from another directory, so the path must be absolute.
  const environment: NodeJS.ProcessEnv = { ...process.env, SHELL: resolve(workload), HUTERM_RENDER_STATS: "1" };
  if (mode === "echo") environment.HUTERM_RENDER_WORKLOAD = "echo";
  // Huterm runs until stopped, so reaching the deadline is the expected outcome.
  const outcome = await runSmokeProcess([executable], { timeoutMs: Number(seconds) * 1_000, env: environment, stream: false });
  if (!outcome.timedOut) throw new Error(`Huterm exited early (${outcome.exitCode ?? outcome.signalCode})\n${outcome.stderr}`);
  console.log(`mode=${mode} ${summarize(parseIntervals(outcome.stderr))}`);
}
