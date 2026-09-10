/** Run the renderer smoke with the same portable process checks as native smokes. */
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

export type RendererOutcome = {
  exitCode: number | null;
  signalCode: string | null;
  timedOut: boolean;
  stdout: string;
  stderr: string;
};

const ALL_PANELS = "RENDERER_SMOKE all-panels prepared=3 checked=3 painted=3";
const PASSED = "RENDERER_SMOKE passed";

export function checkRenderer(outcome: RendererOutcome): void {
  if (outcome.timedOut) throw new Error("renderer smoke timed out after 15000ms");
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
    writeFile(join(target, "outcome.json"), `${JSON.stringify({ exitCode: outcome.exitCode, signalCode: outcome.signalCode, timedOut: outcome.timedOut }, null, 2)}\n`),
  ]);
}

if (import.meta.main) {
  const executable = Bun.argv[2];
  if (!executable) throw new Error("usage: check-renderer.ts <executable>");
  const timeout = 15_000;
  const started = performance.now();
  const result = Bun.spawnSync([executable], {
    stdout: "pipe", stderr: "pipe", timeout,
  });
  const outcome: RendererOutcome = {
    exitCode: result.exitCode,
    signalCode: result.signalCode ?? null,
    timedOut: result.exitCode === null && performance.now() - started >= timeout - 100,
    stdout: result.stdout.toString(),
    stderr: result.stderr.toString(),
  };
  process.stdout.write(outcome.stdout);
  process.stderr.write(outcome.stderr);
  try {
    checkRenderer(outcome);
  } catch (error) {
    const evidence = process.env.HUTERM_SMOKE_EVIDENCE_DIR;
    if (evidence) await writeFailureEvidence(evidence, outcome);
    throw error;
  }
}
