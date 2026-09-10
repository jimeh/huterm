import { expect, test } from "bun:test";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkRenderer, writeFailureEvidence, type RendererOutcome } from "./check-renderer.ts";

const outcome = (changes: Partial<RendererOutcome> = {}): RendererOutcome => ({
  exitCode: 0,
  signalCode: null,
  timedOut: false,
  stdout: "RENDERER_SMOKE prepared size=12\nRENDERER_SMOKE prepared size=16\nRENDERER_SMOKE prepared size=20\nRENDERER_SMOKE all-panels prepared=3 checked=3 painted=3\nRENDERER_SMOKE passed\n",
  stderr: "",
  ...changes,
});

test("renderer checker requires every completion marker", () => {
  expect(() => checkRenderer(outcome())).not.toThrow();
  expect(() => checkRenderer(outcome({ stdout: "RENDERER_SMOKE passed\n" }))).toThrow("all panels");
  expect(() => checkRenderer(outcome({ stdout: "RENDERER_SMOKE all-panels prepared=3 checked=3 painted=3\n" }))).toThrow("final success");
});

test("renderer checker distinguishes process failures", () => {
  expect(() => checkRenderer(outcome({ exitCode: null, signalCode: "SIGTERM", timedOut: true }))).toThrow("timed out");
  expect(() => checkRenderer(outcome({ exitCode: null, signalCode: "SIGKILL" }))).toThrow("terminated by SIGKILL");
  expect(() => checkRenderer(outcome({ exitCode: 7 }))).toThrow("exited 7");
});

test("renderer failures retain process evidence", async () => {
  const directory = await mkdtemp(join(tmpdir(), "huterm-renderer-evidence-"));
  try {
    await writeFailureEvidence(directory, outcome({ exitCode: 7, stderr: "native failure\n" }));
    expect(await readFile(join(directory, "renderer", "stdout.log"), "utf8")).toContain("all-panels");
    expect(await readFile(join(directory, "renderer", "stderr.log"), "utf8")).toBe("native failure\n");
    expect(JSON.parse(await readFile(join(directory, "renderer", "outcome.json"), "utf8"))).toEqual({ exitCode: 7, signalCode: null, timedOut: false });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
