/** Run the renderer smoke with the same portable process checks as native smokes. */
import { closeSync, fstatSync, openSync, readdirSync, readSync, realpathSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import { basename, join } from "node:path";
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

/** The absolute library paths in `ldd` output, including the dynamic loader. */
export function lddPaths(output: string): string[] {
  return output.split("\n").flatMap((line) => {
    const path = line.match(/=>\s*(\/\S+)/)?.[1] ?? line.match(/^\s*(\/\S+)\s+\(0x/)?.[1];
    return path ? [path] : [];
  });
}

function linkedLibraries(file: string): string[] {
  try {
    const result = Bun.spawnSync(["ldd", file], { timeout: 10_000, stderr: "ignore" });
    return result.exitCode === 0 ? lddPaths(result.stdout.toString()) : [];
  } catch {
    return [];
  }
}

/**
 * The Mesa libraries the first Linux draw can load, and the libraries they
 * link. wgpu starts both its Vulkan and GL backends, so the first launch
 * loads llvmpipe's Vulkan driver and, where glvnd's `libEGL` and Mesa's EGL
 * are installed, `libgallium` through them. `libgallium` is warmed whether
 * or not EGL is installed. Runner images carry several LLVM versions and
 * Mesa links one, so `ldd` picks it. When `ldd` names no LLVM, every
 * `libLLVM` file is added, so the warm-up never covers less than reading
 * them all by name did.
 */
export function mesaLibraries(roots = ["/usr/lib", "/usr/lib64"], linked = linkedLibraries): string[] {
  const list = (directory: string) => {
    try { return readdirSync(directory); } catch { return []; }
  };
  const directories = roots.flatMap((root) => [root, ...list(root).filter((name) => name.endsWith("-linux-gnu")).map((name) => join(root, name))]);
  const named = (pattern: RegExp) => directories.flatMap((directory) => list(directory)
    .filter((name) => pattern.test(name))
    .map((name) => join(directory, name)));
  const entries = named(/^(libvulkan_lvp\.so|libEGL\.so\.1|libEGL_mesa\.so\.0)$|^libgallium[.-].*\.so/);
  const found = [...entries, ...entries.flatMap((entry) => linked(entry))];
  if (found.some((path) => basename(path).startsWith("libLLVM"))) return found;
  return [...found, ...named(/^libLLVM[.-].*\.so/)];
}

const PT_LOAD = 1;
const PN_XNUM = 0xffff;
const CHUNK_BYTES = 8 * 1048576;

/**
 * The file ranges of a 64-bit little-endian ELF file's `PT_LOAD` segments,
 * which are the only parts the loader maps. Any other file, or one whose
 * program headers are truncated or point outside the file, counts as one
 * range covering all of it.
 */
export function loadedRanges(fd: number, size: number): Array<[offset: number, length: number]> {
  const whole: Array<[number, number]> = [[0, size]];
  const header = Buffer.alloc(64);
  if (readSync(fd, header, 0, header.length, 0) < header.length) return whole;
  if (header.readUInt32BE(0) !== 0x7f454c46 || header[4] !== 2 || header[5] !== 1) return whole;
  const tableOffset = Number(header.readBigUInt64LE(0x20));
  const entrySize = header.readUInt16LE(0x36);
  const count = header.readUInt16LE(0x38);
  if (entrySize < 56 || count === 0 || count === PN_XNUM) return whole;
  if (!Number.isSafeInteger(tableOffset) || tableOffset + entrySize * count > size) return whole;
  const table = Buffer.alloc(entrySize * count);
  if (readSync(fd, table, 0, table.length, tableOffset) < table.length) return whole;
  const ranges: Array<[number, number]> = [];
  for (let entry = 0; entry < table.length; entry += entrySize) {
    if (table.readUInt32LE(entry) !== PT_LOAD) continue;
    const offset = Number(table.readBigUInt64LE(entry + 8));
    if (!Number.isSafeInteger(offset) || offset > size) return whole;
    const length = Math.min(Number(table.readBigUInt64LE(entry + 32)), size - offset);
    if (length > 0) ranges.push([offset, length]);
  }
  return ranges.length > 0 ? ranges : whole;
}

/**
 * Read a file's mapped segments into the page cache and return its size and
 * the bytes read. A file that cannot be opened or read counts only what was
 * read before the failure: the warm-up is best effort.
 */
export function readLoadedSegments(file: string, chunk = Buffer.alloc(CHUNK_BYTES)): { size: number; read: number } {
  let size = 0;
  let read = 0;
  let fd: number;
  try { fd = openSync(file, "r"); } catch { return { size, read }; }
  try {
    size = fstatSync(fd).size;
    for (const [offset, length] of loadedRanges(fd, size)) {
      for (let done = 0; done < length;) {
        const count = readSync(fd, chunk, 0, Math.min(chunk.length, length - done), offset + done);
        if (count === 0) break;
        done += count;
        read += count;
      }
    }
  } catch {
    // Keep what was read; the timed smoke reports any real failure.
  } finally {
    try { closeSync(fd); } catch { /* already closed or invalid */ }
  }
  return { size, read };
}

/**
 * Before the timed smoke on Linux, flush dirty pages, then read the mapped
 * segments of the smoke binary, Mesa's driver, and the libraries the driver
 * links into the page cache. Five of six thread dumps from hung runs showed
 * a thread waiting on a page-cache folio (`folio_wait_bit_common`), and the
 * renderer is the first smoke after the binaries are linked. The `sync`
 * keeps the launch's page faults from queueing behind writeback of the build
 * output. The read skips sections the loader never maps and LLVM versions
 * Mesa does not link, and reads each file once however many names link to
 * it. The warm-up runs outside the
 * smoke's deadline and reports how long each part took, one line per file
 * and a summary line last.
 */
export function warmUp(executable: string): string {
  const started = performance.now();
  const sync = Bun.spawnSync(["sync"], { timeout: 120_000 });
  const synced = Math.round(performance.now() - started);
  const chunk = Buffer.alloc(CHUNK_BYTES);
  const mib = (bytes: number) => (bytes / 1048576).toFixed(1);
  const lines: string[] = [];
  let size = 0;
  let read = 0;
  for (const file of uniqueFiles([executable, ...mesaLibraries()])) {
    const fileStarted = performance.now();
    const segments = readLoadedSegments(file, chunk);
    size += segments.size;
    read += segments.read;
    lines.push(`RENDERER_WARMUP file=${file} file_mib=${mib(segments.size)} read_mib=${mib(segments.read)} ms=${Math.round(performance.now() - fileStarted)}`);
  }
  const total = Math.round(performance.now() - started);
  lines.push(`RENDERER_WARMUP sync=${sync.exitCode ?? sync.signalCode} synced_ms=${synced} file_mib=${mib(size)} read_mib=${mib(read)} total_ms=${total}`);
  return lines.join("\n");
}

/** The files behind these paths, each named once by its resolved path. */
function uniqueFiles(paths: string[]): string[] {
  const resolve = (path: string) => {
    try { return realpathSync(path); } catch { return path; }
  };
  return [...new Set(paths.map(resolve))];
}

if (import.meta.main) {
  const executable = Bun.argv[2];
  if (!executable) throw new Error("usage: check-renderer.ts <executable>");
  const evidence = process.env.HUTERM_SMOKE_EVIDENCE_DIR;
  if (process.platform === "linux") console.error(warmUp(executable));
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
