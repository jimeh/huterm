/** Run every renderer benchmark scenario, aggregate repeated runs, and compare reports. */
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import { parseArgs } from "node:util";
import { runSmokeProcess } from "./smoke-process.ts";

export const SCENARIOS = ["ascii", "blocks", "boxes", "churn", "scroll", "selection"] as const;
const PREFIX = "RENDERER_BENCH ";
const PASSED = "RENDERER_BENCH passed";
const TIMEOUT_MS = 60_000;
const FRAME_STALL_EXIT_CODE = 3;

type Fields = Record<string, string>;
export type Phase = { minNs: number; medianNs: number; p95Ns: number; counts: Record<string, number> };
export type ScenarioReport = { prepare: Phase; paint: Phase };
export type Report = { revision: string; platform: string; runs: number; scenarios: Record<string, ScenarioReport> };

function fields(line: string): Fields {
  return Object.fromEntries(line.slice(PREFIX.length).split(" ").filter(part => part.includes("=")).map(part => {
    const index = part.indexOf("=");
    return [part.slice(0, index), part.slice(index + 1)];
  }));
}

/** Parse one process's output. A clipped grid or missing phase invalidates the run. */
export function parseRun(scenario: string, stdout: string): Record<"prepare" | "paint", Fields> {
  const lines = stdout.split(/\r?\n/);
  if (!lines.includes(PASSED)) throw new Error(`${scenario}: benchmark did not print its final success marker`);
  const phases = new Map(lines.filter(line => line.startsWith(PREFIX) && line !== PASSED).map(line => {
    const parsed = fields(line);
    return [parsed.phase, parsed] as const;
  }));
  for (const phase of phases.values()) {
    if (phase.scenario !== scenario) throw new Error(`${scenario}: benchmark reported scenario ${phase.scenario}`);
  }
  const [prepare, paint, window] = ["prepare", "paint", "window"].map(name => phases.get(name));
  if (!prepare || !paint || !window) throw new Error(`${scenario}: benchmark output is missing a phase`);
  if (window.fits !== "true") throw new Error(`${scenario}: the ${window.grid} grid was clipped by the window, which understates paint cost`);
  return { prepare, paint };
}

function median(values: number[]): number {
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.ceil((sorted.length - 1) / 2)] ?? 0;
}

const TIMINGS = new Set(["samples", "min_ns", "median_ns", "p95_ns", "max_ns"]);

/** Combine repeated processes: the median of medians resists one preempted run. */
export function aggregate(runs: Fields[]): Phase {
  const numbers = (name: string) => runs.map(run => Number(run[name]));
  const last = runs.at(-1) ?? {};
  const counts = Object.fromEntries(Object.entries(last)
    .filter(([name, value]) => !TIMINGS.has(name) && /^[0-9]+$/.test(value))
    .map(([name, value]) => [name, Number(value)]));
  return { minNs: Math.min(...numbers("min_ns")), medianNs: median(numbers("median_ns")), p95Ns: median(numbers("p95_ns")), counts };
}

const micros = (nanoseconds: number) => (nanoseconds / 1_000).toFixed(1).padStart(9);

function change(before: number, after: number): string {
  if (before === 0) return "n/a".padStart(8);
  const percent = ((after - before) / before) * 100;
  return `${percent >= 0 ? "+" : ""}${percent.toFixed(1)}%`.padStart(8);
}

export function renderTable(report: Report, baseline?: Report): string {
  const lines = [`revision ${report.revision} on ${report.platform}, ${report.runs} runs per scenario${baseline ? `, compared with ${baseline.revision}` : ""}`];
  lines.push(`${"scenario".padEnd(10)} ${"phase".padEnd(8)} ${"median µs".padStart(9)} ${"min µs".padStart(9)} ${"p95 µs".padStart(9)}${baseline ? ` ${"median".padStart(8)} ${"min".padStart(8)}` : ""}  counts`);
  for (const [name, scenario] of Object.entries(report.scenarios)) {
    for (const phase of ["prepare", "paint"] as const) {
      const current = scenario[phase];
      const previous = baseline?.scenarios[name]?.[phase];
      const delta = baseline ? ` ${previous ? change(previous.medianNs, current.medianNs) : "new".padStart(8)} ${previous ? change(previous.minNs, current.minNs) : "".padStart(8)}` : "";
      const counts = Object.entries(current.counts).map(([key, value]) => {
        const before = previous?.counts[key];
        return before !== undefined && before !== value ? `${key}=${before}->${value}` : `${key}=${value}`;
      }).join(" ");
      lines.push(`${name.padEnd(10)} ${phase.padEnd(8)} ${micros(current.medianNs)} ${micros(current.minNs)} ${micros(current.p95Ns)}${delta}  ${counts}`);
    }
  }
  return lines.join("\n");
}

async function revision(): Promise<string> {
  const fromEnvironment = process.env.HUTERM_SOURCE_REVISION;
  if (fromEnvironment) return fromEnvironment;
  const head = Bun.spawnSync(["git", "rev-parse", "--short", "HEAD"]);
  const dirty = Bun.spawnSync(["git", "status", "--porcelain"]);
  if (head.exitCode !== 0) return "unknown";
  return `${head.stdout.toString().trim()}${dirty.stdout.toString().trim() ? "-dirty" : ""}`;
}

if (import.meta.main) {
  const { values, positionals } = parseArgs({
    args: Bun.argv.slice(2),
    allowPositionals: true,
    options: { runs: { type: "string", default: "5" }, scenarios: { type: "string" }, output: { type: "string" }, compare: { type: "string" } },
  });
  const executable = positionals[0];
  if (!executable) throw new Error("usage: run-renderer-bench.ts <executable> [--runs N] [--scenarios a,b] [--output report.json] [--compare baseline.json]");
  const runs = Number(values.runs);
  if (!Number.isInteger(runs) || runs <= 0) throw new Error("--runs must be a positive integer");
  const selected = values.scenarios ? values.scenarios.split(",") : [...SCENARIOS];
  const report: Report = { revision: await revision(), platform: `${process.platform}-${process.arch}`, runs, scenarios: {} };
  for (const scenario of selected) {
    const collected: Record<"prepare" | "paint", Fields[]> = { prepare: [], paint: [] };
    for (let run = 0; run < runs; run += 1) {
      const launch = () => runSmokeProcess([executable], { timeoutMs: TIMEOUT_MS, env: { ...process.env, HUTERM_RENDERER_BENCH_SCENARIO: scenario }, stream: false });
      let outcome = await launch();
      if (outcome.exitCode === FRAME_STALL_EXIT_CODE) {
        // One Xvfb run in roughly a hundred stopped receiving frames; the cause is not
        // established. A stalled process reports no timings, so a retry cannot bias them.
        console.error(`${scenario}: the display delivered too few frames; retrying once`);
        outcome = await launch();
      }
      if (outcome.timedOut || outcome.exitCode !== 0) {
        throw new Error(`${scenario}: benchmark ${outcome.timedOut ? "timed out" : `exited ${outcome.exitCode ?? outcome.signalCode}`}\n${outcome.stderr}`);
      }
      const parsed = parseRun(scenario, outcome.stdout);
      collected.prepare.push(parsed.prepare);
      collected.paint.push(parsed.paint);
    }
    report.scenarios[scenario] = { prepare: aggregate(collected.prepare), paint: aggregate(collected.paint) };
  }
  const baseline = values.compare ? JSON.parse(await readFile(values.compare, "utf8")) as Report : undefined;
  console.log(renderTable(report, baseline));
  if (values.output) {
    await mkdir(dirname(values.output), { recursive: true });
    await writeFile(values.output, `${JSON.stringify(report, null, 2)}\n`);
  }
}
