/** Validate the scroll benchmark log without changing its timing or queue budgets. */
const WARM_SAMPLES = 5;
const MIN_SAMPLES = 20;
const MIN_INPUT_SAMPLES = 10;
export const REQUIRED_SCROLL_SNAPSHOT_SAMPLES = WARM_SAMPLES + MIN_SAMPLES;

type Sample = Record<string, number>;

function fail(message: string): never {
  throw new Error(`scroll benchmark failed: ${message}`);
}

function field(sample: Sample, name: string): number {
  const value = sample[name];
  if (value === undefined || !Number.isSafeInteger(value)) {
    fail(`missing or invalid field: ${name}`);
  }
  return value;
}

function percentile(values: number[], fraction: number): number {
  const ordered = values.toSorted((a, b) => a - b);
  return ordered[Math.max(0, Math.ceil(ordered.length * fraction) - 1)]!;
}

function median(values: number[]): number {
  const ordered = values.toSorted((a, b) => a - b);
  const middle = Math.floor(ordered.length / 2);
  return Math.trunc(ordered.length % 2 ? ordered[middle]! : (ordered[middle - 1]! + ordered[middle]!) / 2);
}

function budget(value: number, limit: number, description: string): void {
  if (value >= limit) fail(`${description} ${value}us exceeds ${limit}us`);
}

export function checkScrollBenchmark(log: string): string {
  const lines = log.split(/\r?\n/);
  const samples = (prefix: string): Sample[] => lines
    .filter(line => line.startsWith(prefix))
    .map(line => Object.fromEntries([...line.matchAll(/([a-z_]+)=([0-9]+)/g)]
      .map(match => [match[1]!, Number(match[2])])));
  let snapshots = samples("huterm-scroll snapshot ");
  let paintSamples = samples("huterm-scroll sample ");
  const queue = samples("huterm-scroll queue ");
  if (snapshots.length < REQUIRED_SCROLL_SNAPSHOT_SAMPLES) {
    fail(`needed at least ${REQUIRED_SCROLL_SNAPSHOT_SAMPLES} snapshot samples, got ${snapshots.length}`);
  }
  if (!queue.length) fail("missing request queue diagnostics");
  if (snapshots.some(sample => field(sample, "requested") !== field(sample, "returned"))) {
    fail("a snapshot did not match its requested offset");
  }
  if (paintSamples.some(sample => field(sample, "requested") !== field(sample, "returned"))) {
    fail("a painted snapshot did not match its requested offset");
  }

  snapshots = snapshots.slice(WARM_SAMPLES);
  const inputSnapshots = snapshots.filter(sample => sample.input === 1);
  if (inputSnapshots.length < MIN_INPUT_SAMPLES) {
    fail(`needed at least ${MIN_INPUT_SAMPLES} matched snapshot input samples after warmup, got ${inputSnapshots.length}`);
  }
  const snapshotElapsed = snapshots.map(sample => field(sample, "snapshot_us"));
  const medianSnapshotElapsed = median(snapshotElapsed);
  const p95SnapshotElapsed = percentile(snapshotElapsed, 0.95);
  const p95SnapshotLatency = percentile(inputSnapshots.map(sample => field(sample, "latency_us")), 0.95);
  const medianWakeup = median(snapshots.map(sample => field(sample, "timer_wait_us")));
  budget(medianSnapshotElapsed, 8_000, "median snapshot elapsed time");
  budget(p95SnapshotElapsed, 16_700, "p95 snapshot elapsed time");
  budget(p95SnapshotLatency, 33_400, "p95 input-to-snapshot");
  budget(medianWakeup, 8_000, "median snapshot wakeup");
  if (queue.some(item => field(item, "maximum_concurrent") > 1)) {
    fail("more than one snapshot request was concurrently active");
  }
  if (queue.some(item => field(item, "maximum_queued") > 1)) {
    fail("more than one replacement snapshot was queued");
  }
  const latest = queue.at(-1)!;
  if (field(latest, "requests_started") - field(latest, "requests_completed") > 1) {
    fail("snapshot request backlog exceeded the single in-flight request");
  }
  if (field(latest, "requests_coalesced") === 0 || field(latest, "queued_updates") === 0) {
    fail("benchmark did not exercise queued request coalescing");
  }

  let presentation = "not_measured";
  let inputPaintSamples = 0;
  let medianPaintElapsed = 0;
  let p95PaintElapsed = 0;
  let p95PaintLatency = 0;
  if (paintSamples.length >= REQUIRED_SCROLL_SNAPSHOT_SAMPLES) {
    paintSamples = paintSamples.slice(WARM_SAMPLES);
    const inputPaint = paintSamples.filter(sample => sample.input === 1);
    if (inputPaint.length < MIN_INPUT_SAMPLES) {
      fail(`needed at least ${MIN_INPUT_SAMPLES} matched paint input samples after warmup, got ${inputPaint.length}`);
    }
    const combined = paintSamples.map(sample => field(sample, "snapshot_us") + field(sample, "prepare_us") + field(sample, "paint_us"));
    medianPaintElapsed = median(combined);
    p95PaintElapsed = percentile(combined, 0.95);
    p95PaintLatency = percentile(inputPaint.map(sample => field(sample, "latency_us")), 0.95);
    budget(medianPaintElapsed, 8_000, "median combined paint elapsed time");
    budget(p95PaintElapsed, 16_700, "p95 combined paint elapsed time");
    budget(p95PaintLatency, 33_400, "p95 input-to-matching-paint");
    const oneRowReuse = paintSamples.slice(1).some((current, index) =>
      Math.abs(field(current, "requested") - field(paintSamples[index]!, "returned")) === 1 && field(current, "rebuilt_rows") <= 1);
    if (!oneRowReuse) fail("no static one-row sample rebuilt at most the exposed row");
    presentation = "pass";
    inputPaintSamples = inputPaint.length;
  }

  const summary = {
    snapshot_samples: snapshots.length,
    input_snapshot_samples: inputSnapshots.length,
    median_snapshot_elapsed_us: medianSnapshotElapsed,
    p95_snapshot_elapsed_us: p95SnapshotElapsed,
    p95_input_to_snapshot_us: p95SnapshotLatency,
    median_wakeup_us: medianWakeup,
    paint_samples: paintSamples.length,
    input_paint_samples: inputPaintSamples,
    median_paint_elapsed_us: medianPaintElapsed,
    p95_paint_elapsed_us: p95PaintElapsed,
    p95_input_to_paint_us: p95PaintLatency,
    presentation,
    requests_started: field(latest, "requests_started"),
    requests_completed: field(latest, "requests_completed"),
    requests_coalesced: field(latest, "requests_coalesced"),
    maximum_concurrent: field(latest, "maximum_concurrent"),
    maximum_queued: field(latest, "maximum_queued"),
    budget: "pass",
  };
  const output = [`huterm-scroll summary ${Object.entries(summary).map(([key, value]) => `${key}=${value}`).join(" ")}`];
  if (presentation === "not_measured") {
    output.push(`Paint budgets were not measured because the host produced fewer than ${REQUIRED_SCROLL_SNAPSHOT_SAMPLES} paint samples.`);
  }
  output.push("Elapsed preparation and paint encoding do not prove GPU presentation.");
  return output.join("\n");
}

if (import.meta.main) {
  try {
    if (Bun.argv.length !== 3) fail("expected one benchmark log path");
    console.log(checkScrollBenchmark(await Bun.file(Bun.argv[2]!).text()));
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
