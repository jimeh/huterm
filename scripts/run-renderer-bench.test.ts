import { expect, test } from "bun:test";
import { aggregate, parseRun, renderTable, type Report } from "./run-renderer-bench.ts";

const output = (fits: boolean) => [
  "RENDERER_BENCH scenario=ascii phase=prepare samples=60 min_ns=100 median_ns=200 p95_ns=300 max_ns=900 rebuilt_rows=50 cache_hits=7000 cache_misses=0",
  "RENDERER_BENCH scenario=ascii phase=paint samples=30 min_ns=400 median_ns=500 p95_ns=600 max_ns=700 backgrounds=300 glyphs=7000 builtins=0 rectangles=0 paths=0 decorations=40",
  `RENDERER_BENCH scenario=ascii phase=window grid=160x50 scale=1 fits=${fits}`,
  "RENDERER_BENCH passed",
].join("\n");

test("parses both measured phases", () => {
  const run = parseRun("ascii", output(true));
  expect(run.prepare.median_ns).toBe("200");
  expect(run.paint.backgrounds).toBe("300");
});

test("rejects clipped, incomplete, and mismatched runs", () => {
  expect(() => parseRun("ascii", output(false))).toThrow("clipped");
  expect(() => parseRun("ascii", output(true).replace("RENDERER_BENCH passed", ""))).toThrow("success marker");
  expect(() => parseRun("ascii", output(true).split("\n").filter(line => !line.includes("phase=paint")).join("\n"))).toThrow("missing a phase");
  expect(() => parseRun("boxes", output(true))).toThrow("reported scenario ascii");
});

test("aggregates with the median of medians and the overall minimum", () => {
  const run = (min: number, median: number) => ({ samples: "30", min_ns: `${min}`, median_ns: `${median}`, p95_ns: "9", max_ns: "9", paths: "12" });
  expect(aggregate([run(5, 50), run(3, 900), run(4, 40)])).toEqual({ minNs: 3, medianNs: 50, p95Ns: 9, counts: { paths: 12 } });
});

test("reports timing and count changes against a baseline", () => {
  const report = (medianNs: number, paths: number): Report => ({
    revision: `r${medianNs}`,
    platform: "linux-x64",
    runs: 1,
    scenarios: { boxes: { prepare: { minNs: 1_000, medianNs: 2_000, p95Ns: 3_000, counts: {} }, paint: { minNs: medianNs, medianNs, p95Ns: medianNs, counts: { paths } } } },
  });
  const table = renderTable(report(500_000, 0), report(1_000_000, 900));
  expect(table).toContain("compared with r1000000");
  expect(table).toContain("-50.0%");
  expect(table).toContain("paths=900->0");
});
