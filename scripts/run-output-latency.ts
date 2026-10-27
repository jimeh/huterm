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

const KEYS_PREFIX = "huterm-render keys ";

export type KeyInterval = { keys: number; echoes: number; snapshots: number; appliedMedianUs: number; appliedMaxUs: number; paintedMedianUs: number; paintedMaxUs: number };

export function parseKeyIntervals(log: string): KeyInterval[] {
  return log.split(/\r?\n/).filter(line => line.startsWith(KEYS_PREFIX)).map(line => {
    const field = (name: string) => {
      const match = line.match(new RegExp(`(?:^| )${name}=([0-9]+)(?: |$)`));
      if (!match) throw new Error(`key latency line is missing ${name}: ${line}`);
      return Number(match[1]);
    };
    return { keys: field("keys"), echoes: field("echoes"), snapshots: field("snapshots"), appliedMedianUs: field("applied_us_median"), appliedMaxUs: field("applied_us_max"), paintedMedianUs: field("painted_us_median"), paintedMaxUs: field("painted_us_max") };
  });
}

export type KeySummary = { intervals: number; keys: number; echoes: number; snapshotsPerKey: number; appliedMedianUs: number; appliedMaxUs: number; paintedMedianUs: number; paintedMaxUs: number };

/** Like `summarize`, the first interval includes startup and is excluded. */
export function summarizeKeys(intervals: KeyInterval[]): KeySummary {
  const steady = intervals.slice(1);
  if (steady.length === 0) throw new Error("Huterm printed no key latency intervals");
  const keys = steady.reduce((total, interval) => total + interval.keys, 0);
  const echoes = steady.reduce((total, interval) => total + interval.echoes, 0);
  if (echoes === 0) throw new Error(`none of ${keys} keys produced an echo snapshot; key input is not reaching the terminal`);
  const snapshots = steady.reduce((total, interval) => total + interval.snapshots, 0);
  return {
    intervals: steady.length,
    keys,
    echoes,
    snapshotsPerKey: Math.round(snapshots / keys * 100) / 100,
    appliedMedianUs: median(steady.map(interval => interval.appliedMedianUs)),
    appliedMaxUs: Math.max(...steady.map(interval => interval.appliedMaxUs)),
    paintedMedianUs: median(steady.map(interval => interval.paintedMedianUs)),
    paintedMaxUs: Math.max(...steady.map(interval => interval.paintedMaxUs)),
  };
}

export function formatKeys(summary: KeySummary): string {
  return `key-latency intervals=${summary.intervals} keys=${summary.keys} echoes=${summary.echoes} snapshots_per_key=${summary.snapshotsPerKey} applied_us_median=${summary.appliedMedianUs} applied_us_max=${summary.appliedMaxUs} painted_us_median=${summary.paintedMedianUs} painted_us_max=${summary.paintedMaxUs}`;
}

/**
 * Gates keystroke snapshot scheduling. Each echoed key needs one snapshot; a
 * second, started before the echo exists, delays the echo to the next frame.
 */
export function checkSnapshotsPerKeyBudget(summary: KeySummary, budget: number): void {
  if (summary.snapshotsPerKey > budget) {
    throw new Error(`snapshots_per_key=${summary.snapshotsPerKey} exceeds the budget of ${budget}: key input is requesting snapshots that precede its echo`);
  }
}

const CYCLE_PREFIX = "HUTERM_BENCH compression cycle=";

export type Cycle = { tagged: boolean; appliedUs: number; steps: number; stepMeanUs: number };

/** Parses the compression driver's timed cycles; skipped cycles carry no sample. */
export function parseCycles(log: string): Cycle[] {
  return log.split(/\r?\n/).filter(line => line.startsWith(CYCLE_PREFIX) && !line.includes(" skipped=")).map(line => {
    const field = (name: string) => {
      const match = line.match(new RegExp(`(?:^| )${name}=([0-9a-z]+)(?: |$)`));
      if (!match) throw new Error(`compression cycle line is missing ${name}: ${line}`);
      return match[1];
    };
    return { tagged: field("tagged") === "true", appliedUs: Number(field("applied_us")), steps: Number(field("steps")), stepMeanUs: Number(field("step_us_mean")) };
  });
}

export type CycleSummary = { cycles: number; tagged: number; appliedMedianUs: number; appliedMaxUs: number; stepMeanMedianUs: number };

/**
 * Summarizes keys typed during a pass, or every cycle for the uncompressed
 * baseline. A compressed run needs `minimumTagged` keys that the runtime
 * handled while a pass was in progress, or it measured nothing.
 */
export function summarizeCycles(cycles: Cycle[], compressed: boolean, minimumTagged: number): CycleSummary {
  const tagged = cycles.filter(cycle => cycle.tagged);
  const measured = compressed ? tagged : cycles;
  if (compressed && tagged.length < minimumTagged) throw new Error(`only ${tagged.length} of ${cycles.length} keys reached the runtime during a pass; at least ${minimumTagged} are needed`);
  if (measured.length === 0) throw new Error("Huterm printed no compression cycles");
  const applied = measured.map(cycle => cycle.appliedUs);
  return {
    cycles: cycles.length,
    tagged: tagged.length,
    appliedMedianUs: median(applied),
    appliedMaxUs: Math.max(...applied),
    stepMeanMedianUs: median(measured.map(cycle => cycle.stepMeanUs)),
  };
}

export function formatCycles(summary: CycleSummary): string {
  return `compression-latency cycles=${summary.cycles} tagged=${summary.tagged} applied_us_median=${summary.appliedMedianUs} applied_us_max=${summary.appliedMaxUs} step_us_mean_median=${summary.stepMeanMedianUs}`;
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
  const [executable, workload, mode = "echo", secondsArgument] = Bun.argv.slice(2);
  // Compression mode stops when its driver finishes; the default only bounds it.
  const seconds = secondsArgument ?? (mode === "compression" ? "600" : "8");
  if (!executable || !workload) throw new Error("usage: run-output-latency.ts <huterm> <render_workload> [echo|flood|keys|compression] [seconds]");
  if (mode !== "echo" && mode !== "flood" && mode !== "keys" && mode !== "compression") throw new Error(`mode must be echo, flood, keys, or compression, not ${mode}`);
  // Compression mode: HUTERM_BENCH_HISTORY picks build-log or styles history,
  // HUTERM_BENCH_COMPRESSION=off measures the uncompressed baseline, and
  // HUTERM_BENCH_KEY_DELAY_MS sets when its key follows each resize.
  const history = process.env.HUTERM_BENCH_HISTORY ?? "build-log";
  if (history !== "build-log" && history !== "styles") throw new Error(`HUTERM_BENCH_HISTORY must be build-log or styles, not ${history}`);
  const compressed = (process.env.HUTERM_BENCH_COMPRESSION ?? "on") !== "off";
  const keyDelay = process.env.HUTERM_BENCH_KEY_DELAY_MS ?? "300";
  const timeoutMs = Number(seconds) * 1_000;
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) throw new Error(`seconds must be a positive number, not ${seconds}`);
  const budgetValue = process.env.HUTERM_OUTPUT_LATENCY_APPLIED_BUDGET_US;
  const budgetUs = budgetValue === undefined ? undefined : Number(budgetValue);
  if (budgetUs !== undefined && (!Number.isFinite(budgetUs) || budgetUs <= 0)) throw new Error(`HUTERM_OUTPUT_LATENCY_APPLIED_BUDGET_US must be a positive number, not ${budgetValue}`);
  const keyBudgetValue = process.env.HUTERM_OUTPUT_LATENCY_SNAPSHOTS_PER_KEY_BUDGET;
  const keyBudget = keyBudgetValue === undefined ? undefined : Number(keyBudgetValue);
  if (keyBudget !== undefined && (!Number.isFinite(keyBudget) || keyBudget <= 0)) throw new Error(`HUTERM_OUTPUT_LATENCY_SNAPSHOTS_PER_KEY_BUDGET must be a positive number, not ${keyBudgetValue}`);
  if (budgetUs !== undefined && mode !== "echo") throw new Error("the applied latency budget applies to echo mode only");
  if (keyBudget !== undefined && mode !== "keys") throw new Error("the snapshots-per-key budget applies to keys mode only");
  // The terminal starts its shell from another directory, so the path must be absolute.
  // An isolated configuration keeps the user's font, theme, and global shortcuts
  // out of the measurement; a running Huterm would otherwise own the shortcuts.
  const refresh = process.env.HUTERM_BENCH_REFRESH;
  if (refresh !== undefined && refresh !== "display" && refresh !== "unlimited") throw new Error(`HUTERM_BENCH_REFRESH must be display or unlimited, not ${refresh}`);
  const configDirectory = await mkdtemp(join(tmpdir(), "huterm-output-latency-"));
  const config = join(configDirectory, "config.toml");
  // Outside compression mode, a pass that starts mid-run would change what is measured.
  const terminal = mode === "compression" ? `scrollback_rows = 100000\nscrollback_compression = ${compressed}\n` : "scrollback_compression = false\n";
  await Bun.write(config, `[terminal]\n${terminal}${refresh === undefined ? "" : `refresh = "${refresh}"\n`}`);
  // "events" records the probe without requesting a frame per display tick;
  // continuous drawing would otherwise hold the main thread in present.
  // Always set the workload so an inherited value cannot change the mode.
  // Only keys mode types into the window; an inherited driver must not add keys to other modes.
  const environment: NodeJS.ProcessEnv = { ...process.env, SHELL: resolve(workload), HUTERM_CONFIG_FILE: config, HUTERM_RENDER_STATS: "events", HUTERM_RENDER_WORKLOAD: mode, HUTERM_KEY_BENCH: mode === "keys" ? "1" : mode === "compression" ? "compression" : "0", HUTERM_RENDER_HISTORY: history };
  delete environment.HUTERM_KEY_BENCH_DELAY_MS;
  if (mode === "compression" && !compressed) environment.HUTERM_KEY_BENCH_DELAY_MS = keyDelay;
  try {
    // Huterm runs until stopped, so reaching the deadline is the expected outcome.
    const outcome = await runSmokeProcess([executable], {
      timeoutMs, env: environment, stream: false,
      stopWhen: mode === "compression" ? stderr => stderr.includes("HUTERM_BENCH compression done") : undefined,
    });
    if (!outcome.timedOut && !outcome.stopped) throw new Error(`Huterm exited early (${outcome.exitCode ?? outcome.signalCode})\n${outcome.stderr}`);
    if (mode === "compression" && !outcome.stopped) throw new Error(`the compression driver did not finish within ${seconds} s`);
    for (const line of outcome.stderr.split(/\r?\n/).filter(line => line.startsWith("HUTERM_BENCH "))) console.log(line);
    if (mode === "compression") {
      const summary = summarizeCycles(parseCycles(outcome.stderr), compressed, 40);
      console.log(`mode=${mode} history=${history} compression=${compressed ? "on" : "off"} ${formatCycles(summary)}`);
    } else if (mode === "keys") {
      const summary = summarizeKeys(parseKeyIntervals(outcome.stderr));
      console.log(`mode=${mode} ${formatKeys(summary)}`);
      if (keyBudget !== undefined) {
        checkSnapshotsPerKeyBudget(summary, keyBudget);
        console.log(`mode=${mode} snapshots_per_key_budget=${keyBudget} passed`);
      }
    } else {
      const summary = summarize(parseIntervals(outcome.stderr));
      console.log(`mode=${mode} ${format(summary)}`);
      if (budgetUs !== undefined) {
        checkAppliedBudget(summary, budgetUs);
        console.log(`mode=${mode} applied_budget_us=${budgetUs} passed`);
      }
    }
  } finally {
    await rm(configDirectory, { recursive: true, force: true });
  }
}
