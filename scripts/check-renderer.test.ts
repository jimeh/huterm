import { expect, test } from "bun:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkRenderer, readLoadedSegments, writeFailureEvidence, type RendererOutcome } from "./check-renderer.ts";

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
  expect(() => checkRenderer(outcome({ exitCode: null, signalCode: "SIGTERM", timedOut: true, stalled: true, elapsedMs: 17_250 })))
    .toThrow("renderer smoke timed out after 17250ms (no RENDERER_SMOKE progress for 15000ms)");
  expect(() => checkRenderer(outcome({ exitCode: null, signalCode: "SIGKILL" }))).toThrow("terminated by SIGKILL");
  expect(() => checkRenderer(outcome({ exitCode: 7 }))).toThrow("exited 7");
});

test("renderer failures retain process evidence", async () => {
  const directory = await mkdtemp(join(tmpdir(), "huterm-renderer-evidence-"));
  try {
    await writeFailureEvidence(directory, outcome({ exitCode: 7, stderr: "native failure\n" }));
    expect(await readFile(join(directory, "renderer", "stdout.log"), "utf8")).toContain("all-panels");
    expect(await readFile(join(directory, "renderer", "stderr.log"), "utf8")).toBe("native failure\n");
    expect(JSON.parse(await readFile(join(directory, "renderer", "outcome.json"), "utf8"))).toEqual({ exitCode: 7, signalCode: null, timedOut: false, stalled: false });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

/** A 64-bit little-endian ELF image with the given program headers and `size` bytes in all. */
function elf(size: number, segments: Array<{ type: number; offset: number; length: number }>): Buffer {
  const image = Buffer.alloc(size);
  image.writeUInt32BE(0x7f454c46, 0);
  image[4] = 2;
  image[5] = 1;
  image.writeBigUInt64LE(64n, 0x20);
  image.writeUInt16LE(56, 0x36);
  image.writeUInt16LE(segments.length, 0x38);
  segments.forEach(({ type, offset, length }, index) => {
    const entry = 64 + index * 56;
    image.writeUInt32LE(type, entry);
    image.writeBigUInt64LE(BigInt(offset), entry + 8);
    image.writeBigUInt64LE(BigInt(length), entry + 32);
  });
  return image;
}

test("the warm-up reads only an ELF file's loaded segments, and all of any other file", async () => {
  const directory = await mkdtemp(join(tmpdir(), "huterm-renderer-warmup-"));
  try {
    const binary = join(directory, "binary");
    // Two PT_LOAD segments, the second running past the end of the file, and a PT_NOTE.
    await writeFile(binary, elf(100_000, [
      { type: 1, offset: 0, length: 4096 },
      { type: 4, offset: 4096, length: 512 },
      { type: 1, offset: 90_000, length: 20_000 },
    ]));
    expect(readLoadedSegments(binary, Buffer.alloc(1000))).toEqual({ size: 100_000, read: 4096 + 10_000 });

    const script = join(directory, "script");
    await writeFile(script, "#!/bin/sh\n".repeat(500));
    expect(readLoadedSegments(script)).toEqual({ size: 5000, read: 5000 });

    expect(readLoadedSegments(join(directory, "missing"))).toEqual({ size: 0, read: 0 });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
