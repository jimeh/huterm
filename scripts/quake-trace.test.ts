import { expect, test } from "bun:test";
import { analyzeFade, analyzeReversal, analyzeSlide, observationsForLatestGeneration, retryInconclusiveOnce, type QuakeObservation } from "./quake-trace";

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

test("wrong fade opacity fails immediately", () => {
  const history = [observation(0.5), observation(0, false, undefined, undefined, 1)].map(item => ({ ...item, frame: expectation.endpoint }));
  history[0]!.opacity = 0.9;
  expect(() => analyzeFade(history, false, expectation.endpoint)).toThrow("fade disagrees");
});

test("a valid reversal preserves continuity and reaches the endpoint", () => {
  const hiding = [observation(0.7), observation(0.45, false, undefined, undefined, 1)];
  const showing = [observation(0.47, true, [0, -212, 800, 400], 20_000), observation(0.75, true, undefined, undefined, 1), observation(1, true, undefined, undefined, 2)];
  expect(analyzeReversal(hiding, showing, expectation, 1000).status).toBe("passed");
});

test("a discontinuous reversal fails immediately", () => {
  const hiding = [observation(0.7), observation(0.45, false, undefined, undefined, 1)];
  const showing = [observation(0.75, true, undefined, 10_000), observation(1, true, undefined, undefined, 2)];
  expect(() => analyzeReversal(hiding, showing, expectation, 1000)).toThrow("reversal progress jumped");
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
