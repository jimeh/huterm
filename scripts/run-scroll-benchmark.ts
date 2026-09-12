import { REQUIRED_SCROLL_SNAPSHOT_SAMPLES } from "./check-scroll-benchmark.ts";

const DEFAULT_TIMEOUT_MS = 30_000;
const SNAPSHOT_PREFIX = "huterm-scroll snapshot ";
const QUEUE_PREFIX = "huterm-scroll queue ";
const SNAPSHOT_COLLECTION_TARGET = REQUIRED_SCROLL_SNAPSHOT_SAMPLES * 3;

type Outcome =
  | { kind: "ready" }
  | { kind: "exit"; code: number }
  | { kind: "timeout" };

function numericField(line: string, name: string): number | undefined {
  const match = line.match(new RegExp(`(?:^| )${name}=([0-9]+)(?: |$)`));
  return match ? Number(match[1]) : undefined;
}

function snapshotCount(log: string): number {
  return log.split(/\r?\n/).filter(line => line.startsWith(SNAPSHOT_PREFIX)).length;
}

export function scrollBenchmarkReady(log: string): boolean {
  if (snapshotCount(log) < SNAPSHOT_COLLECTION_TARGET) return false;
  const latestQueue = log.split(/\r?\n/).filter(line => line.startsWith(QUEUE_PREFIX)).at(-1);
  return latestQueue !== undefined
    && (numericField(latestQueue, "requests_coalesced") ?? 0) > 0
    && (numericField(latestQueue, "queued_updates") ?? 0) > 0;
}

async function collect(
  stream: ReadableStream<Uint8Array>,
  append: (text: string) => void,
): Promise<void> {
  const reader = stream.getReader();
  const decoder = new TextDecoder();
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    append(decoder.decode(value, { stream: true }));
  }
  append(decoder.decode());
}

export async function runScrollBenchmark(
  reportPath: string,
  command: string[],
  timeoutMs = DEFAULT_TIMEOUT_MS,
): Promise<void> {
  if (command.length === 0) throw new Error("scroll benchmark command is required");

  const child = Bun.spawn(command, { stdout: "pipe", stderr: "pipe" });
  let output = "";
  let resolveReady: (outcome: Outcome) => void = () => {};
  const ready = new Promise<Outcome>(resolve => {
    resolveReady = resolve;
  });
  const append = (text: string): void => {
    output += text;
    if (scrollBenchmarkReady(output)) resolveReady({ kind: "ready" });
  };
  const pumps = Promise.all([
    collect(child.stdout, append),
    collect(child.stderr, append),
  ]);
  const exited = child.exited.then(code => ({ kind: "exit", code }) as const);
  let timeout: ReturnType<typeof setTimeout> | undefined;
  const timedOut = new Promise<Outcome>(resolve => {
    timeout = setTimeout(() => resolve({ kind: "timeout" }), timeoutMs);
  });

  const outcome = await Promise.race([ready, exited, timedOut]);
  if (timeout !== undefined) clearTimeout(timeout);
  if (outcome.kind !== "exit") child.kill();
  await child.exited;
  await pumps;
  await Bun.write(reportPath, output);
  process.stdout.write(output);

  if (outcome.kind === "exit") {
    throw new Error(`Huterm scroll benchmark ended before collecting required evidence (status ${outcome.code})`);
  }
  if (outcome.kind === "timeout") {
    throw new Error(
      `Huterm scroll benchmark timed out after ${timeoutMs}ms with ${snapshotCount(output)} snapshot samples`,
    );
  }
}

if (import.meta.main) {
  const [reportPath, ...command] = Bun.argv.slice(2);
  try {
    if (reportPath === undefined) throw new Error("expected a report path and benchmark command");
    await runScrollBenchmark(reportPath, command);
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
