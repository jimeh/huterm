/** Run the renderer smoke with the same portable process checks as native smokes. */
import { readdirSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { runSmokeProcess, type ProgressWatchdog } from "./smoke-process.ts";

export type RendererOutcome = {
  exitCode: number | null;
  signalCode: string | null;
  timedOut: boolean;
  stalled?: boolean;
  elapsedMs?: number;
  stdout: string;
  stderr: string;
};

/**
 * The smoke fails once 15 seconds pass without a new `RENDERER_SMOKE` line,
 * and after 60 seconds in total. A cold first launch on a hosted runner can
 * spend most of a fixed 15-second budget faulting in freshly linked pages, so
 * the deadline follows progress instead: a hang between two markers still
 * fails at 15 seconds.
 */
export const STALL_MS = 15_000;
export const TOTAL_MS = 60_000;
export const WATCHDOG: ProgressWatchdog = { pattern: /^RENDERER_SMOKE /, stallMs: STALL_MS };

const ALL_PANELS = "RENDERER_SMOKE all-panels prepared=3 checked=3 painted=3";
const PASSED = "RENDERER_SMOKE passed";

export function checkRenderer(outcome: RendererOutcome): void {
  if (outcome.timedOut) {
    const cause = outcome.stalled ? `no RENDERER_SMOKE progress for ${STALL_MS}ms` : `the ${TOTAL_MS}ms limit`;
    throw new Error(`renderer smoke timed out after ${outcome.elapsedMs ?? TOTAL_MS}ms (${cause})`);
  }
  if (outcome.signalCode !== null) throw new Error(`renderer smoke terminated by ${outcome.signalCode}`);
  if (outcome.exitCode !== 0) throw new Error(`renderer smoke exited ${outcome.exitCode}`);
  const lines = outcome.stdout.split(/\r?\n/);
  if (!lines.includes(ALL_PANELS)) throw new Error("renderer smoke did not confirm all panels were prepared, checked, and painted");
  if (!lines.includes(PASSED)) throw new Error("renderer smoke did not print its final success marker");
}

export async function writeFailureEvidence(directory: string, outcome: RendererOutcome): Promise<void> {
  const target = join(directory, "renderer");
  await mkdir(target, { recursive: true });
  await Promise.all([
    writeFile(join(target, "stdout.log"), outcome.stdout),
    writeFile(join(target, "stderr.log"), outcome.stderr),
    writeFile(join(target, "outcome.json"), `${JSON.stringify({ exitCode: outcome.exitCode, signalCode: outcome.signalCode, timedOut: outcome.timedOut, stalled: outcome.stalled ?? false }, null, 2)}\n`),
  ]);
}

/** Mesa's software Vulkan driver and LLVM, which the first Linux draw loads. */
export function mesaLibraries(roots = ["/usr/lib", "/usr/lib64"]): string[] {
  const list = (directory: string) => {
    try { return readdirSync(directory); } catch { return []; }
  };
  const directories = roots.flatMap((root) => [root, ...list(root).filter((name) => name.endsWith("-linux-gnu")).map((name) => join(root, name))]);
  return directories.flatMap((directory) => list(directory)
    .filter((name) => /^(libvulkan_lvp|libLLVM|libgallium)[.-].*\.so/.test(name))
    .map((name) => join(directory, name)));
}

/**
 * Before the timed smoke on Linux, flush dirty pages and read the smoke
 * binary and Mesa into the page cache. Five of six thread dumps from hung
 * runs showed a thread waiting on a page-cache folio (`folio_wait_bit_common`),
 * and the renderer is the first smoke after the binaries are linked. The
 * warm-up runs outside the smoke's deadline and reports how long it took.
 */
export async function warmUp(executable: string): Promise<string> {
  const started = performance.now();
  const sync = Bun.spawnSync(["sync"], { timeout: 120_000 });
  const synced = Math.round(performance.now() - started);
  let bytes = 0;
  for (const file of [executable, ...mesaLibraries()]) {
    bytes += (await readFile(file).catch(() => Buffer.alloc(0))).length;
  }
  const total = Math.round(performance.now() - started);
  return `RENDERER_WARMUP sync=${sync.exitCode ?? sync.signalCode} synced_ms=${synced} read_mib=${(bytes / 1048576).toFixed(1)} total_ms=${total}`;
}

if (import.meta.main) {
  const executable = Bun.argv[2];
  if (!executable) throw new Error("usage: check-renderer.ts <executable>");
  const evidence = process.env.HUTERM_SMOKE_EVIDENCE_DIR;
  if (process.platform === "linux") console.error(await warmUp(executable));
  const outcome = await runSmokeProcess([executable], {
    timeoutMs: TOTAL_MS,
    progress: WATCHDOG,
    evidenceDir: evidence ? join(evidence, "renderer-process") : undefined,
  });
  try {
    checkRenderer(outcome);
  } catch (error) {
    const evidence = process.env.HUTERM_SMOKE_EVIDENCE_DIR;
    if (evidence) await writeFailureEvidence(evidence, outcome);
    throw error;
  }
}
