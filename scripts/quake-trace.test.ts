import { appendFile, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "bun:test";
import { analyzeFade, analyzeReversal, analyzeSlide, isIntermediateObservation, observationsForLatestGeneration, readQuakeTrace, retryInconclusiveOnce, type QuakeObservation } from "./quake-trace";

const observation = (progress: number, desired = false, frame: [number, number, number, number] = [0, -400 * (1 - progress), 800, 400], gap = 16_000, monotonic = 0): QuakeObservation => ({
  monotonic_us: monotonic,
  profile: "default",
  generation: 7,
  desired,
  progress,
  stage: progress === Number(desired) ? (desired ? "SettleVisible" : "SettleHidden") : "Animate",
  frame,
  opacity: progress,
  scheduler_gap_us: gap,
});
const expectation = { endpoint: [0, 0, 800, 400] as [number, number, number, number], edge: "top" as const, fade: true };

test("valid slide and fade histories pass", () => {
  const history = [observation(0.75), observation(0.45, false, undefined, undefined, 1), observation(0, false, undefined, undefined, 2)];
  expect(analyzeSlide(history, false, expectation).status).toBe("passed");
  expect(analyzeFade(history.map(item => ({ ...item, frame: expectation.endpoint })), false, expectation.endpoint).status).toBe("passed");
});

test("a scheduler-skipped history is inconclusive when its endpoint is correct", () => {
  expect(analyzeSlide([observation(0, false, [0, -400, 800, 400], 1_050_000)], false, expectation)).toMatchObject({ status: "inconclusive" });
});

test("wrong-axis movement fails immediately", () => {
  expect(() => analyzeSlide([observation(0.5, false, [20, -200, 800, 400]), observation(0, false, undefined, undefined, 1)], false, expectation)).toThrow("wrong axis");
});

test("wrong-direction movement fails immediately", () => {
  expect(() => analyzeSlide([observation(0.5, false, [0, 200, 800, 400]), observation(0, false, [0, 400, 800, 400], undefined, 1)], false, expectation)).toThrow("wrong direction");
});

test("under-travel that disagrees with progress fails immediately", () => {
  const history = [observation(0.75), observation(0.5, false, [0, -120, 800, 400], undefined, 1), observation(0, false, undefined, undefined, 2)];
  expect(() => analyzeSlide(history, false, expectation)).toThrow("does not match progress");
});

test("a hidden endpoint that leaves the window partly visible fails immediately", () => {
  const history = [observation(0.5, false, [0, -100, 800, 400]), observation(0, false, [0, -200, 800, 400], undefined, 1)];
  expect(() => analyzeSlide(history, false, expectation)).toThrow("hidden endpoint is not fully outside");
});

test("non-monotonic progress and target changes fail immediately", () => {
  const nonMonotonic = [observation(0.5, false, undefined, undefined, 0), observation(0.6, false, undefined, undefined, 1), observation(0, false, undefined, undefined, 2)];
  expect(() => analyzeSlide(nonMonotonic, false, expectation)).toThrow("progress moved away");
  const targetChange = [observation(0.5), observation(1, true, undefined, undefined, 1)];
  expect(() => analyzeSlide(targetChange, false, expectation)).toThrow("changed target");
});

test("wrong fade opacity fails immediately", () => {
  const history = [observation(0.5), observation(0, false, undefined, undefined, 1)].map(item => ({ ...item, frame: expectation.endpoint }));
  history[0]!.opacity = 0.9;
  expect(() => analyzeFade(history, false, expectation.endpoint)).toThrow("fade disagrees");
});

test("a valid reversal preserves continuity and reaches the endpoint", () => {
  const hiding = [observation(0.7), observation(0.45, false, undefined, undefined, 1)];
  const showing = [observation(0.47, true, [0, -212, 800, 400], 20_000, 20_001), observation(0.75, true, undefined, undefined, 20_002), observation(1, true, undefined, undefined, 20_003)];
  expect(analyzeReversal(hiding, showing, expectation, 1000).status).toBe("passed");
});

test("a discontinuous reversal fails immediately", () => {
  const hiding = [observation(0.7), observation(0.45, false, undefined, undefined, 1)];
  const showing = [observation(0.75, true, undefined, 10_000, 11_000), observation(1, true, undefined, undefined, 11_001)];
  expect(() => analyzeReversal(hiding, showing, expectation, 1000)).toThrow("reversal progress jumped");
});

test("reversal continuity allows progress explained by the retarget sample gap", () => {
  const hiding = [observation(0.7, false, undefined, undefined, 0), observation(0.45, false, undefined, undefined, 1_000)];
  const showing = [observation(0.65, true, undefined, 10_000, 201_000), observation(0.8, true, undefined, undefined, 201_001), observation(1, true, undefined, undefined, 201_002)];
  expect(analyzeReversal(hiding, showing, expectation, 1000).status).toBe("passed");
});

test("the shared intermediate window accepts progress between 0.7 and 0.8", () => {
  expect(isIntermediateObservation(observation(0.75, true))).toBeTrue();
  expect(analyzeFade([observation(0.75, true, expectation.endpoint), observation(1, true, expectation.endpoint, undefined, 1)], true, expectation.endpoint).status).toBe("passed");
});

test("latest-generation filtering excludes earlier attempts and other profiles", () => {
  const earlier = observation(0.5);
  const latest = { ...observation(0), generation: 8 };
  const otherProfile = { ...latest, profile: "scratch" };
  expect(observationsForLatestGeneration([earlier, latest, otherProfile], "default", false)).toEqual([latest]);
});

test("only one inconclusive result is retried", async () => {
  let attempts = 0;
  const result = await retryInconclusiveOnce(async () => ++attempts === 1
    ? { status: "inconclusive" as const, reason: "scheduler gap" }
    : { status: "passed" as const, intermediate: observation(0.5) });
  expect(result.attempts).toBe(2);
  expect(attempts).toBe(2);
});

test("a second inconclusive result fails", async () => {
  let attempts = 0;
  await expect(retryInconclusiveOnce(async () => {
    attempts++;
    return { status: "inconclusive" as const, reason: "scheduler gap" };
  })).rejects.toThrow("after one retry");
  expect(attempts).toBe(2);
});

test("trace reader waits for an incomplete trailing record", async () => {
  const directory = await mkdtemp(join(tmpdir(), "huterm-quake-trace-test-"));
  const file = join(directory, "trace.jsonl");
  const line = JSON.stringify(observation(0.5));
  const midpoint = Math.floor(line.length / 2);
  try {
    await writeFile(file, line.slice(0, midpoint));
    expect(await readQuakeTrace(file)).toEqual([]);
    await appendFile(file, `${line.slice(midpoint)}\n`);
    expect(await readQuakeTrace(file)).toEqual([observation(0.5)]);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
