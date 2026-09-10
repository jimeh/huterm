import { readFile } from "node:fs/promises";

export type QuakeObservation = {
  monotonic_us: number;
  profile: string;
  generation: number;
  desired: boolean;
  progress: number;
  stage: string;
  frame: [number, number, number, number];
  target_frame: [number, number, number, number];
  display_frame: [number, number, number, number];
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

function validateFrame(name: string, frame: number[]): void {
  if (frame.length !== 4 || frame.some(value => !Number.isFinite(value))) throw new Error(`animation trace has an invalid ${name}`);
}

function framesMatch(left: number[], right: number[], tolerance: number): boolean {
  return left.every((value, index) => near(value, right[index]!, tolerance));
}

function validateCommon(observations: QuakeObservation[], desired: boolean, requireEndpoint = true): void {
  if (observations.length === 0) throw new Error("animation trace is empty");
  const target = desired ? 1 : 0;
  const first = observations[0]!;
  for (const [index, observation] of observations.entries()) {
    if (observation.desired !== desired) throw new Error("animation trace changed target");
    if (observation.generation !== first.generation) throw new Error("animation trace changed generation");
    validateFrame("frame", observation.frame);
    validateFrame("target frame", observation.target_frame);
    validateFrame("display frame", observation.display_frame);
    if (!framesMatch(observation.target_frame, first.target_frame, 0.001)) throw new Error("animation target frame changed within a generation");
    if (!framesMatch(observation.display_frame, first.display_frame, 0.001)) throw new Error("animation display frame changed within a generation");
    if (![observation.progress, observation.opacity, observation.monotonic_us, observation.scheduler_gap_us].every(Number.isFinite)) throw new Error("animation trace has a non-finite value");
    if (index > 0) {
      const previous = observations[index - 1]!;
      if (observation.monotonic_us < previous.monotonic_us) throw new Error("animation trace time moved backwards");
      if (desired ? observation.progress + 0.001 < previous.progress : observation.progress - 0.001 > previous.progress) throw new Error("animation progress moved away from its target");
    }
  }
  if (requireEndpoint && !near(observations.at(-1)!.progress, target, 0.001)) throw new Error(`animation trace did not reach progress ${target}`);
}

export function observationsForLatestGeneration(observations: QuakeObservation[], profile: string, desired: boolean): QuakeObservation[] {
  const matching = observations.filter(observation => observation.profile === profile && observation.desired === desired);
  const generation = matching.at(-1)?.generation;
  return generation === undefined ? [] : matching.filter(observation => observation.generation === generation);
}

function hiddenFrame(target: [number, number, number, number], display: [number, number, number, number], edge: SlideExpectation["edge"]): [number, number, number, number] {
  const hidden: [number, number, number, number] = [...target];
  switch (edge) {
    case "top": hidden[1] = display[1] - target[3]; break;
    case "bottom": hidden[1] = display[1] + display[3]; break;
    case "left": hidden[0] = display[0] - target[2]; break;
    case "right": hidden[0] = display[0] + display[2]; break;
  }
  return hidden;
}

function validateSlideTrajectory(observations: QuakeObservation[], desired: boolean, expectation: SlideExpectation, requireEndpoint: boolean): void {
  validateCommon(observations, desired, requireEndpoint);
  const target = observations[0]!.target_frame;
  const display = observations[0]!.display_frame;
  if (!framesMatch(target, expectation.endpoint, 2)) throw new Error(`animation target disagrees with native endpoint: ${target} -> ${expectation.endpoint}`);
  const hidden = hiddenFrame(target, display, expectation.edge);
  for (const observation of observations) {
    const expected = target.map((value, index) => value + (hidden[index]! - value) * (1 - observation.progress));
    if (!framesMatch(observation.frame, expected, 2)) {
      throw new Error(`native slide frame does not match fixed trajectory: expected ${expected}, observed ${observation.frame}`);
    }
    if (expectation.fade && !near(observation.opacity, observation.progress, 0.03)) throw new Error(`native fade disagrees with progress: ${observation.opacity}/${observation.progress}`);
    if (!expectation.fade && !near(observation.opacity, 1, 0.001)) throw new Error(`slide changed opacity: ${observation.opacity}`);
  }
}

export function analyzeSlide(observations: QuakeObservation[], desired: boolean, expectation: SlideExpectation): TraceVerdict {
  validateSlideTrajectory(observations, desired, expectation, true);
  const intermediate = observations.find(isIntermediateObservation);
  return intermediate
    ? { status: "passed", intermediate }
    : { status: "inconclusive", reason: skippedReason(observations) };
}

export function analyzeFade(observations: QuakeObservation[], desired: boolean, endpoint: [number, number, number, number]): TraceVerdict {
  validateCommon(observations, desired);
  const target = observations[0]!.target_frame;
  if (!framesMatch(target, endpoint, 2)) throw new Error(`animation target disagrees with native endpoint: ${target} -> ${endpoint}`);
  for (const observation of observations) {
    if (!framesMatch(observation.frame, target, 2)) throw new Error(`native fade moved frame: ${target} -> ${observation.frame}`);
    if (!near(observation.opacity, observation.progress, 0.03)) throw new Error(`native fade disagrees with progress: ${observation.opacity}/${observation.progress}`);
  }
  const intermediate = observations.find(isIntermediateObservation);
  return intermediate
    ? { status: "passed", intermediate }
    : { status: "inconclusive", reason: skippedReason(observations) };
}

export function analyzeReversal(hiding: QuakeObservation[], showing: QuakeObservation[], expectation: SlideExpectation, durationMs: number): TraceVerdict {
  if (hiding.length === 0) throw new Error("reversal hide trace is empty");
  validateSlideTrajectory(hiding, false, expectation, false);
  const before = hiding.at(-1)!;
  if (!isIntermediateObservation(before)) return { status: "inconclusive", reason: skippedReason(hiding) };
  const result = analyzeSlide(showing, true, expectation);
  const after = showing[0]!;
  if (!framesMatch(before.target_frame, after.target_frame, 0.001)) throw new Error("reversal target frame changed across retarget");
  if (!framesMatch(before.display_frame, after.display_frame, 0.001)) throw new Error("reversal display frame changed across retarget");
  if (after.monotonic_us < before.monotonic_us) throw new Error("reversal trace time moved backwards");
  const retargetGapUs = after.monotonic_us - before.monotonic_us;
  const allowedProgress = retargetGapUs / (durationMs * 1000) + 0.03;
  if (Math.abs(after.progress - before.progress) > allowedProgress) throw new Error(`reversal progress jumped: ${before.progress} -> ${after.progress}`);
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
