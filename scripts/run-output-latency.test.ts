import { expect, test } from "bun:test";
import { parseIntervals, summarize } from "./run-output-latency.ts";

const line = (snapshots: number, applied: number, painted: number) =>
  `huterm-render output snapshots=${snapshots} applied_us_median=${applied} applied_us_max=${applied * 2} paints=${snapshots} painted_us_median=${painted} painted_us_max=${painted * 2}`;

test("summarizes steady intervals and skips the startup interval", () => {
  const log = ["huterm-render frames=60", line(3, 90_000, 95_000), line(10, 900, 9_000), line(12, 1_100, 9_400), line(10, 1_000, 9_200)].join("\n");
  expect(summarize(parseIntervals(log))).toBe("output-latency intervals=3 snapshots_per_second=11 applied_us_median=1000 applied_us_max=2200 painted_us_median=9200 painted_us_max=18800");
});

test("rejects logs without probe output and malformed probe lines", () => {
  expect(() => summarize(parseIntervals("huterm-render frames=60"))).toThrow("no output latency intervals");
  expect(() => parseIntervals("huterm-render output snapshots=1")).toThrow("missing applied_us_median");
});
