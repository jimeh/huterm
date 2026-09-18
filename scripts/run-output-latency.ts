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

/** The first interval includes startup, so it is always excluded. */
export function summarize(intervals: Interval[]): string {
  const steady = intervals.slice(1);
  if (steady.length === 0) throw new Error("Huterm printed no output latency intervals");
  const snapshots = steady.reduce((total, interval) => total + interval.snapshots, 0);
  // Intervals close at the first paint after one second, so their length varies.
  const elapsedSeconds = steady.reduce((total, interval) => total + interval.elapsedUs, 0) / 1_000_000;
  return `output-latency intervals=${steady.length} snapshots_per_second=${Math.round(snapshots / elapsedSeconds)} applied_us_median=${median(steady.map(interval => interval.appliedMedianUs))} applied_us_max=${Math.max(...steady.map(interval => interval.appliedMaxUs))} painted_us_median=${median(steady.map(interval => interval.paintedMedianUs))} painted_us_max=${Math.max(...steady.map(interval => interval.paintedMaxUs))}`;
}

if (import.meta.main) {
  const [executable, workload, mode = "echo", seconds = "8"] = Bun.argv.slice(2);
  if (!executable || !workload) throw new Error("usage: run-output-latency.ts <huterm> <render_workload> [echo|flood] [seconds]");
  if (mode !== "echo" && mode !== "flood") throw new Error(`mode must be echo or flood, not ${mode}`);
  const timeoutMs = Number(seconds) * 1_000;
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) throw new Error(`seconds must be a positive number, not ${seconds}`);
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
    console.log(`mode=${mode} ${summarize(parseIntervals(outcome.stderr))}`);
  } finally {
    await rm(configDirectory, { recursive: true, force: true });
  }
}
