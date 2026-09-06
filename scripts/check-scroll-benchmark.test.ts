import { expect, test } from "bun:test";
import { checkScrollBenchmark } from "./check-scroll-benchmark.ts";

export function benchmarkLog(paint = false): string {
  const lines: string[] = [];
  for (let sequence = 0; sequence < 25; sequence++) {
    lines.push(`huterm-scroll snapshot sequence=${sequence} requested=${sequence} returned=${sequence} snapshot_us=1000 input=1 latency_us=2000 timer_wait_us=500`);
    if (paint) lines.push(`huterm-scroll sample sequence=${sequence} requested=${sequence} returned=${sequence} snapshot_us=1000 prepare_us=1000 paint_us=1000 input=1 latency_us=4000 timer_wait_us=500 rebuilt_rows=1 total_rows=32`);
  }
  lines.push("huterm-scroll queue requests_started=25 requests_completed=25 requests_coalesced=1 queued_updates=1 maximum_concurrent=1 maximum_queued=1");
  return lines.join("\n") + "\n";
}

test("accepts snapshot-only headless logs without claiming presentation", () => {
  const result = checkScrollBenchmark(benchmarkLog());
  expect(result).toContain("presentation=not_measured");
  expect(result).toContain("median_snapshot_elapsed_us=1000");
  expect(result).toContain("Paint budgets were not measured");
});

test("accepts enough paint samples and combines elapsed time", () => {
  const result = checkScrollBenchmark(benchmarkLog(true));
  expect(result).toContain("paint_samples=20");
  expect(result).toContain("presentation=pass");
  expect(result).toContain("median_paint_elapsed_us=3000");
});

for (const sequence of [0, 6]) {
  test(`rejects snapshot offset mismatch at sample ${sequence}`, () => {
    const log = benchmarkLog().replace(`requested=${sequence} returned=${sequence}`, `requested=${sequence} returned=99`);
    expect(() => checkScrollBenchmark(log)).toThrow("a snapshot did not match its requested offset");
  });
}

test("rejects paint mismatches even during warmup", () => {
  const log = benchmarkLog(true).replace("huterm-scroll sample sequence=0 requested=0 returned=0", "huterm-scroll sample sequence=0 requested=0 returned=1");
  expect(() => checkScrollBenchmark(log)).toThrow("a painted snapshot did not match its requested offset");
});

for (const [from, to, message] of [
  ["snapshot_us=1000", "snapshot_us=8000", "median snapshot elapsed time"],
  ["latency_us=2000", "latency_us=33400", "p95 input-to-snapshot"],
  ["timer_wait_us=500", "timer_wait_us=8000", "median snapshot wakeup"],
  ["maximum_concurrent=1", "maximum_concurrent=2", "concurrently active"],
  ["maximum_queued=1", "maximum_queued=2", "replacement snapshot"],
  ["requests_completed=25", "requests_completed=23", "backlog"],
  ["requests_coalesced=1", "requests_coalesced=0", "coalescing"],
  ["input=1", "input=0", "matched snapshot input"],
  ["paint_us=1000", "paint_us=6000", "combined paint elapsed time"],
  ["latency_us=4000", "latency_us=33400", "input-to-matching-paint"],
  ["rebuilt_rows=1", "rebuilt_rows=2", "static one-row sample"],
] as const) {
  test(`rejects ${message} at the failure boundary`, () => {
    expect(() => checkScrollBenchmark(benchmarkLog(true).replaceAll(from, to))).toThrow(message);
  });
}

test("p95 uses nearest-rank and excludes only the warmup samples", () => {
  const lines = benchmarkLog().split("\n");
  lines[0] = lines[0]!.replace("snapshot_us=1000", "snapshot_us=99999");
  lines[23] = lines[23]!.replace("snapshot_us=1000", "snapshot_us=16700");
  expect(checkScrollBenchmark(lines.join("\n"))).toContain("p95_snapshot_elapsed_us=1000");
  lines[24] = lines[24]!.replace("snapshot_us=1000", "snapshot_us=16700");
  expect(() => checkScrollBenchmark(lines.join("\n"))).toThrow("p95 snapshot elapsed time 16700us");
});

test("rejects missing samples, diagnostics, and numeric fields", () => {
  expect(() => checkScrollBenchmark("")).toThrow("needed at least 25");
  expect(() => checkScrollBenchmark(benchmarkLog().replace(/^huterm-scroll queue.*$/m, ""))).toThrow("missing request queue");
  expect(() => checkScrollBenchmark(benchmarkLog().replaceAll("snapshot_us=1000", ""))).toThrow("missing or invalid field");
});
