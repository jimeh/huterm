import { expect, test } from "bun:test";
import { checkAppliedBudget, format, parseIntervals, summarize } from "./run-output-latency.ts";

const line = (snapshots: number, applied: number, painted: number, elapsed = 1_000_000) =>
  `huterm-render output elapsed_us=${elapsed} snapshots=${snapshots} applied_us_median=${applied} applied_us_max=${applied * 2} paints=${snapshots} painted_us_median=${painted} painted_us_max=${painted * 2}`;

test("summarizes steady intervals and skips the startup interval", () => {
  const log = ["huterm-render frames=60", line(3, 90_000, 95_000), line(10, 900, 9_000), line(12, 1_100, 9_400), line(10, 1_000, 9_200)].join("\n");
  expect(format(summarize(parseIntervals(log)))).toBe("output-latency intervals=3 snapshots_per_second=11 applied_us_median=1000 applied_us_max=2200 painted_us_median=9200 painted_us_max=18800");
});

test("computes the snapshot rate from elapsed time, not interval count", () => {
  // Two intervals that each ran 100 ms long hold 11 snapshots at 10 per second.
  const log = [line(1, 1, 1), line(11, 900, 9_000, 1_100_000), line(11, 900, 9_000, 1_100_000)].join("\n");
  expect(summarize(parseIntervals(log)).snapshotsPerSecond).toBe(10);
});

test("the applied budget fails on pump-paced medians and passes on activity-paced ones", () => {
  const pumpPaced = summarize(parseIntervals([line(1, 1, 1), line(10, 10_400, 16_000), line(10, 9_800, 16_200)].join("\n")));
  expect(() => checkAppliedBudget(pumpPaced, 5_000)).toThrow("applied_us_median=10400 exceeds the budget of 5000");
  const activityPaced = summarize(parseIntervals([line(1, 1, 1), line(10, 400, 6_000), line(10, 500, 6_300)].join("\n")));
  expect(() => checkAppliedBudget(activityPaced, 5_000)).not.toThrow();
});

test("rejects logs without probe output and malformed probe lines", () => {
  expect(() => summarize(parseIntervals("huterm-render frames=60"))).toThrow("no output latency intervals");
  // A lone startup interval is not steady state.
  expect(() => summarize(parseIntervals(line(3, 90_000, 95_000)))).toThrow("no output latency intervals");
  expect(() => parseIntervals("huterm-render output snapshots=1")).toThrow("missing elapsed_us");
});
