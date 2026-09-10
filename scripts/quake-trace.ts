import { readFile } from "node:fs/promises";

export type QuakeObservation = {
  monotonic_us: number;
  profile: string;
  generation: number;
  desired: boolean;
  progress: number;
  stage: string;
  frame: [number, number, number, number];
  opacity: number;
  scheduler_gap_us: number;
};

export type TraceVerdict =
  | { status: "passed"; intermediate: QuakeObservation }
  | { status: "inconclusive"; reason: string };

export type SlideExpectation = {
  endpoint: [number, number, number, number];
  edge: "top" | "bottom" | "left" | "right";
  fade: boolean;
};

export const INTERMEDIATE_PROGRESS_MIN = 0.2;
export const INTERMEDIATE_PROGRESS_MAX = 0.8;

const near = (left: number, right: number, tolerance = 2) =>
  Math.abs(left - right) <= tolerance;

export function isIntermediateObservation(observation: QuakeObservation): boolean {
  return observation.stage === "Animate"
    && observation.progress > INTERMEDIATE_PROGRESS_MIN
    && observation.progress < INTERMEDIATE_PROGRESS_MAX;
}

function validateCommon(observations: QuakeObservation[], desired: boolean): void {
  if (observations.length === 0) throw new Error("animation trace is empty");
  const target = desired ? 1 : 0;
  for (const [index, observation] of observations.entries()) {
    if (observation.desired !== desired) throw new Error("animation trace changed target");
    if (observation.frame.length !== 4 || observation.frame.some(value => !Number.isFinite(value))) throw new Error("animation trace has an invalid frame");
    if (![observation.progress, observation.opacity, observation.monotonic_us, observation.scheduler_gap_us].every(Number.isFinite)) throw new Error("animation trace has a non-finite value");
    if (index > 0) {
      const previous = observations[index - 1]!;
      if (observation.monotonic_us < previous.monotonic_us) throw new Error("animation trace time moved backwards");
      if (desired ? observation.progress + 0.001 < previous.progress : observation.progress - 0.001 > previous.progress) throw new Error("animation progress moved away from its target");
    }
  }
  if (!near(observations.at(-1)!.progress, target, 0.001)) throw new Error(`animation trace did not reach progress ${target}`);
}

export function observationsForLatestGeneration(observations: QuakeObservation[], profile: string, desired: boolean): QuakeObservation[] {
  const matching = observations.filter(observation => observation.profile === profile && observation.desired === desired);
  const generation = matching.at(-1)?.generation;
  return generation === undefined ? [] : matching.filter(observation => observation.generation === generation);
}

export function analyzeSlide(observations: QuakeObservation[], desired: boolean, expectation: SlideExpectation): TraceVerdict {
  validateCommon(observations, desired);
  const axis = expectation.edge === "left" || expectation.edge === "right" ? 0 : 1;
  const orthogonal = axis === 0 ? 1 : 0;
  const direction = expectation.edge === "top" || expectation.edge === "left" ? -1 : 1;
  const trajectorySample = observations.find(observation => observation.progress < 0.999);
  if (!trajectorySample) return { status: "inconclusive", reason: skippedReason(observations) };
  const hiddenAxis = expectation.endpoint[axis]
    + (trajectorySample.frame[axis] - expectation.endpoint[axis]) / (1 - trajectorySample.progress);
  const hiddenDisplacement = hiddenAxis - expectation.endpoint[axis];
  const extent = axis === 0 ? expectation.endpoint[2] : expectation.endpoint[3];
  if (hiddenDisplacement * direction <= 0) {
    throw new Error(`native slide moved in the wrong direction: ${expectation.endpoint} -> ${hiddenAxis}`);
  }
  if (hiddenDisplacement * direction < extent - 2) {
    throw new Error(`native slide hidden endpoint is not fully outside: ${expectation.endpoint} -> ${hiddenAxis}`);
  }
  for (const observation of observations) {
    const moved = observation.frame;
    if (!near(moved[orthogonal], expectation.endpoint[orthogonal]) || !near(moved[2], expectation.endpoint[2]) || !near(moved[3], expectation.endpoint[3])) {
      throw new Error(`native slide changed the wrong axis or size: ${expectation.endpoint} -> ${moved}`);
    }
    const displacement = moved[axis] - expectation.endpoint[axis];
    if (observation.progress < 0.999 && displacement * direction <= 0) {
      throw new Error(`native slide moved in the wrong direction: ${expectation.endpoint} -> ${moved}`);
    }
    const expectedAxis = expectation.endpoint[axis] + hiddenDisplacement * (1 - observation.progress);
    if (!near(moved[axis], expectedAxis)) {
      throw new Error(`native slide frame does not match progress: expected ${expectedAxis}, observed ${moved[axis]}`);
    }
    if (expectation.fade && !near(observation.opacity, observation.progress, 0.03)) throw new Error(`native fade disagrees with progress: ${observation.opacity}/${observation.progress}`);
    if (!expectation.fade && !near(observation.opacity, 1, 0.001)) throw new Error(`slide changed opacity: ${observation.opacity}`);
  }
  const intermediate = observations.find(isIntermediateObservation);
  return intermediate
    ? { status: "passed", intermediate }
    : { status: "inconclusive", reason: skippedReason(observations) };
}

export function analyzeFade(observations: QuakeObservation[], desired: boolean, endpoint: [number, number, number, number]): TraceVerdict {
  validateCommon(observations, desired);
  for (const observation of observations) {
    if (observation.frame.some((value, index) => !near(value, endpoint[index]!))) throw new Error(`native fade moved frame: ${endpoint} -> ${observation.frame}`);
    if (!near(observation.opacity, observation.progress, 0.03)) throw new Error(`native fade disagrees with progress: ${observation.opacity}/${observation.progress}`);
  }
  const intermediate = observations.find(isIntermediateObservation);
  return intermediate
    ? { status: "passed", intermediate }
    : { status: "inconclusive", reason: skippedReason(observations) };
}

export function analyzeReversal(hiding: QuakeObservation[], showing: QuakeObservation[], expectation: SlideExpectation, durationMs: number): TraceVerdict {
  if (hiding.length === 0) throw new Error("reversal hide trace is empty");
  const before = hiding.at(-1)!;
  if (!isIntermediateObservation(before)) return { status: "inconclusive", reason: skippedReason(hiding) };
  const result = analyzeSlide(showing, true, expectation);
  const after = showing[0]!;
  if (after.monotonic_us < before.monotonic_us) throw new Error("reversal trace time moved backwards");
  const retargetGapUs = after.monotonic_us - before.monotonic_us;
  const allowedProgress = retargetGapUs / (durationMs * 1000) + 0.03;
  if (Math.abs(after.progress - before.progress) > allowedProgress) throw new Error(`reversal progress jumped: ${before.progress} -> ${after.progress}`);
  const allowedPixels = Math.max(expectation.endpoint[2], expectation.endpoint[3]) * allowedProgress + 2;
  if (after.frame.some((value, index) => Math.abs(value - before.frame[index]!) > (index < 2 ? allowedPixels : 2))) throw new Error(`reversal frame jumped: ${before.frame} -> ${after.frame}`);
  if (Math.abs(after.opacity - before.opacity) > allowedProgress + 0.03) throw new Error(`reversal opacity jumped: ${before.opacity} -> ${after.opacity}`);
  return result;
}

function skippedReason(observations: QuakeObservation[]): string {
  const largestGap = Math.max(...observations.map(observation => observation.scheduler_gap_us));
  return `scheduler skipped observable intermediate state; largest gap=${(largestGap / 1000).toFixed(1)}ms`;
}

export async function retryInconclusiveOnce<T extends TraceVerdict>(run: (attempt: number) => Promise<T>): Promise<{ verdict: Extract<T, { status: "passed" }>; attempts: number }> {
  for (let attempt = 1; attempt <= 2; attempt++) {
    const verdict = await run(attempt);
    if (verdict.status === "passed") return { verdict: verdict as Extract<T, { status: "passed" }>, attempts: attempt };
    if (attempt === 2) throw new Error(`animation remained inconclusive after one retry: ${verdict.reason}`);
  }
  throw new Error("unreachable animation retry state");
}

export async function readQuakeTrace(file: string): Promise<QuakeObservation[]> {
  const text = await readFile(file, "utf8").catch(error => {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return "";
    throw error;
  });
  const finalNewline = text.lastIndexOf("\n");
  if (finalNewline < 0) return [];
  return text.slice(0, finalNewline).split(/\r?\n/).filter(Boolean).map(line => JSON.parse(line) as QuakeObservation);
}
