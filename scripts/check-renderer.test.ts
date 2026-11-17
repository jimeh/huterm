import { expect, test } from "bun:test";
import { closeSync, openSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkRenderer, lddPaths, loadedRanges, mesaLibraries, readLoadedSegments, writeFailureEvidence, type RendererOutcome } from "./check-renderer.ts";

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
function elf(size: number, segments: Array<{ type: number; offset: number; length: number }>, tableOffset = 64n): Buffer {
  const image = Buffer.alloc(size);
  image.writeUInt32BE(0x7f454c46, 0);
  image[4] = 2;
  image[5] = 1;
  image.writeBigUInt64LE(tableOffset, 0x20);
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

function rangesOf(file: string): Array<[number, number]> {
  const fd = openSync(file, "r");
  try { return loadedRanges(fd, Bun.file(file).size); } finally { closeSync(fd); }
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
    expect(rangesOf(binary)).toEqual([[0, 4096], [90_000, 10_000]]);
    expect(readLoadedSegments(binary, Buffer.alloc(1000))).toEqual({ size: 100_000, read: 4096 + 10_000 });

    const script = join(directory, "script");
    await writeFile(script, "#!/bin/sh\n".repeat(500));
    expect(readLoadedSegments(script)).toEqual({ size: 5000, read: 5000 });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("the warm-up reads a malformed ELF file whole and skips what it cannot read", async () => {
  const directory = await mkdtemp(join(tmpdir(), "huterm-renderer-warmup-"));
  try {
    const unsafeTable = join(directory, "unsafe-table");
    await writeFile(unsafeTable, elf(1000, [{ type: 1, offset: 0, length: 10 }], 0xffff_ffff_ffff_ffffn));
    expect(rangesOf(unsafeTable)).toEqual([[0, 1000]]);

    const truncatedTable = join(directory, "truncated-table");
    await writeFile(truncatedTable, elf(1000, [{ type: 1, offset: 0, length: 10 }], 990n));
    expect(rangesOf(truncatedTable)).toEqual([[0, 1000]]);

    const unsafeSegment = join(directory, "unsafe-segment");
    const image = elf(1000, [{ type: 1, offset: 0, length: 10 }]);
    image.writeBigUInt64LE(0xffff_ffff_ffff_ffffn, 64 + 8);
    await writeFile(unsafeSegment, image);
    expect(rangesOf(unsafeSegment)).toEqual([[0, 1000]]);
    expect(readLoadedSegments(unsafeSegment)).toEqual({ size: 1000, read: 1000 });

    const subdirectory = join(directory, "subdirectory");
    await mkdir(subdirectory);
    expect(readLoadedSegments(subdirectory).read).toBe(0);
    expect(readLoadedSegments(join(directory, "missing"))).toEqual({ size: 0, read: 0 });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("ldd output yields every resolved library path and the dynamic loader", () => {
  expect(lddPaths([
    "\tlinux-vdso.so.1 (0x0000ffff9a1f0000)",
    "\tlibLLVM.so.20.1 => /lib/aarch64-linux-gnu/libLLVM.so.20.1 (0x0000ffff8d000000)",
    "\tlibmissing.so.1 => not found",
    "\t/lib/ld-linux-aarch64.so.1 (0x0000ffff9a1b0000)",
  ].join("\n"))).toEqual(["/lib/aarch64-linux-gnu/libLLVM.so.20.1", "/lib/ld-linux-aarch64.so.1"]);
});

test("the warm-up reads the Mesa libraries ldd names, and every LLVM and Gallium library unless it names both", async () => {
  const root = await mkdtemp(join(tmpdir(), "huterm-renderer-mesa-"));
  try {
    const libraries = join(root, "x86_64-linux-gnu");
    await mkdir(libraries);
    const names = ["libvulkan_lvp.so", "libEGL.so.1", "libEGL_mesa.so.0", "libgallium-25.2.8.so", "libLLVM-20.so.1", "libLLVM-17.so.1", "libz.so.1", "libgbm.so.1"];
    for (const name of names) await writeFile(join(libraries, name), "");
    const at = (name: string) => join(libraries, name);
    // As on Mesa 25.2, the Vulkan driver links LLVM but not libgallium; Mesa's EGL links both.
    const graph: Record<string, string[]> = {
      [at("libvulkan_lvp.so")]: [at("libLLVM-20.so.1"), at("libz.so.1")],
      [at("libEGL_mesa.so.0")]: [at("libgallium-25.2.8.so"), at("libgbm.so.1"), at("libLLVM-20.so.1")],
    };
    const sorted = (paths: string[]) => [...new Set(paths)].sort();
    const entries = [at("libvulkan_lvp.so"), at("libEGL.so.1"), at("libEGL_mesa.so.0")];
    const linked = [at("libLLVM-20.so.1"), at("libz.so.1"), at("libgallium-25.2.8.so"), at("libgbm.so.1")];
    const byName = [at("libgallium-25.2.8.so"), at("libLLVM-20.so.1"), at("libLLVM-17.so.1")];
    expect(sorted(mesaLibraries([root], (file) => graph[file] ?? []))).toEqual(sorted([...entries, ...linked]));

    const vulkanOnly = (file: string) => file === at("libvulkan_lvp.so") ? graph[file]! : [];
    expect(sorted(mesaLibraries([root], vulkanOnly))).toEqual(sorted([...entries, ...graph[at("libvulkan_lvp.so")]!, ...byName]));

    // The real ldd fails on these empty files, or is missing on macOS, and either way the warm-up falls back.
    expect(sorted(mesaLibraries([root]))).toEqual(sorted([...entries, ...byName]));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
