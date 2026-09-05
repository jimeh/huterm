/** Prepare and verify the pinned native source without changing reviewed inputs. */
import { createHash } from "node:crypto";
import { closeSync, existsSync, lstatSync, mkdirSync, mkdtempSync, openSync, readdirSync, readFileSync, readlinkSync, renameSync, rmSync } from "node:fs";
import { basename, join, relative, resolve } from "node:path";
import { parseArgs } from "node:util";
import { dlopen } from "bun:ffi";

type Source = { name: string; url: string; sha256: string; tree_sha256: string };
type Manifest = { revision: string; zig: string; source: Source };
const repository = resolve(import.meta.dir, "..");

export function fileHash(filePath: string): string {
  return createHash("sha256").update(readFileSync(filePath)).digest("hex");
}

export function treeHash(root: string): string {
  const digest = createHash("sha256");
  function visit(directory: string): void {
    const children = readdirSync(directory).sort((a, b) => Buffer.compare(Buffer.from(a), Buffer.from(b)));
    const directories: string[] = [];
    for (const name of children) {
      const entry = join(directory, name);
      const info = lstatSync(entry);
      let value: string;
      if (info.isSymbolicLink()) value = `link:${readlinkSync(entry)}`;
      else if (info.isFile()) value = `file:${fileHash(entry)}`;
      else if (info.isDirectory()) {
        directories.push(entry);
        continue;
      } else throw new Error(`unexpected source entry: ${entry}`);
      digest.update(`${relative(root, entry)}\0${value}\0`);
    }
    // Match Python os.walk's sorted, top-down traversal, not a flat path sort.
    for (const directory of directories) visit(directory);
  }
  visit(root);
  return digest.digest("hex");
}

export function verifyTree(root: string, expected: string): void {
  const info = lstatSync(root, { throwIfNoEntry: false });
  if (!info?.isDirectory() || info.isSymbolicLink() || treeHash(root) !== expected) {
    throw new Error(`source contents differ from the pin: ${root}; remove this generated directory and prepare again`);
  }
}

/** Keep the kernel-owned lock so failed/killed processes cannot leave a stale lock. */
export async function withPrepareLock<T>(root: string, action: () => Promise<T>): Promise<T> {
  const library = process.platform === "darwin" ? "/usr/lib/libSystem.B.dylib" : "libc.so.6";
  const fd = openSync(join(root, ".prepare.lock"), "a");
  try {
    const libc = dlopen(library, { flock: { args: ["i32", "i32"], returns: "i32" } });
    try {
      if (libc.symbols.flock(fd, 2) !== 0) throw new Error(`could not lock Ghostty preparation: ${root}`);
      return await action();
    } finally {
      libc.close();
    }
  } finally {
    closeSync(fd);
  }
}

async function download(item: Source, directory: string): Promise<string> {
  const destination = join(directory, `${item.name}.tar.gz`);
  if (existsSync(destination)) {
    if (fileHash(destination) !== item.sha256) throw new Error(`archive checksum mismatch: ${destination}`);
    return destination;
  }
  const stage = mkdtempSync(join(directory, ".download-"));
  const temporary = join(stage, "archive");
  try {
    const response = await fetch(item.url, {
      headers: { "User-Agent": "huterm-native-prepare" },
      signal: AbortSignal.timeout(120_000),
    });
    if (!response.ok) throw new Error(`download failed: HTTP ${response.status} for ${item.name}`);
    await Bun.write(temporary, response);
    if (fileHash(temporary) !== item.sha256) throw new Error(`archive checksum mismatch for ${item.name}`);
    renameSync(temporary, destination);
  } finally {
    rmSync(stage, { recursive: true, force: true });
  }
  return destination;
}

export async function prepareSource(item: Source, root: string, check: boolean): Promise<void> {
  const source = join(root, "source");
  if (lstatSync(source, { throwIfNoEntry: false })) {
    verifyTree(source, item.tree_sha256);
    return;
  }
  if (check) throw new Error(`missing native source: ${source}`);
  const archives = join(root, "archives");
  mkdirSync(archives, { recursive: true });
  const archive = await download(item, archives);
  const stage = mkdtempSync(join(root, ".extract-"));
  try {
    await new Bun.Archive(await Bun.file(archive).bytes()).extract(stage);
    const children = readdirSync(stage);
    if (children.length !== 1) throw new Error("expected one root directory in the Ghostty archive");
    const extracted = join(stage, children[0]!);
    verifyTree(extracted, item.tree_sha256);
    renameSync(extracted, source);
  } finally {
    rmSync(stage, { recursive: true, force: true });
  }
}

function readManifest(manifestPath: string): Manifest {
  const manifest: unknown = JSON.parse(readFileSync(manifestPath, "utf8"));
  if (!manifest || typeof manifest !== "object" || !("revision" in manifest) || !("zig" in manifest) || !("source" in manifest)) {
    throw new Error("invalid Ghostty source manifest");
  }
  const { revision, zig, source } = manifest;
  if (typeof revision !== "string" || !/^[0-9a-f]{40}$/.test(revision) || typeof zig !== "string" || !source || typeof source !== "object") {
    throw new Error("invalid Ghostty source manifest");
  }
  if (!("name" in source) || typeof source.name !== "string" || source.name !== basename(source.name) || source.name === "." || source.name === ".." ||
      !("url" in source) || typeof source.url !== "string" || !source.url.startsWith("https://") ||
      !("sha256" in source) || typeof source.sha256 !== "string" || !/^[0-9a-f]{64}$/.test(source.sha256) ||
      !("tree_sha256" in source) || typeof source.tree_sha256 !== "string" || !/^[0-9a-f]{64}$/.test(source.tree_sha256)) {
    throw new Error("invalid Ghostty source manifest");
  }
  return { revision, zig, source: { name: source.name, url: source.url, sha256: source.sha256, tree_sha256: source.tree_sha256 } };
}

function verifyRevision(manifest: Manifest): void {
  const source = readFileSync(join(repository, "crates/huterm-core/src/lib.rs"), "utf8");
  const revision = /pub const GHOSTTY_REVISION: &str =\s*"([0-9a-f]{40})";/.exec(source)?.[1];
  const provenance = JSON.parse(readFileSync(join(repository, "third-party/ghostty/provenance.json"), "utf8"));
  if (revision !== manifest.revision) throw new Error("Rust Ghostty revision differs from the native source manifest");
  if (provenance["ghostty-MIT.txt"]?.url !== `https://github.com/ghostty-org/ghostty/blob/${manifest.revision}/LICENSE`) {
    throw new Error("Ghostty license provenance differs from the native source manifest");
  }
}

if (import.meta.main) {
  try {
    const { values } = parseArgs({
      args: Bun.argv.slice(2),
      options: { directory: { type: "string" }, manifest: { type: "string" }, check: { type: "boolean" }, help: { type: "boolean" } },
    });
    if (values.help) {
      console.log("Prepare verified Ghostty sources: [--directory DIR] [--manifest FILE] [--check]");
    } else {
      const manifest = readManifest(resolve(values.manifest ?? join(import.meta.dir, "ghostty-source.json")));
      verifyRevision(manifest);
      const root = resolve(values.directory ?? join(repository, ".native/ghostty"));
      mkdirSync(root, { recursive: true });
      await withPrepareLock(root, async () => {
        if (!values.check) {
          const result = Bun.spawnSync(["zig", "version"]);
          if (result.exitCode !== 0) throw new Error(`zig version failed: ${result.stderr.toString()}`);
          const version = result.stdout.toString().trim();
          if (version !== manifest.zig) throw new Error(`expected Zig ${manifest.zig}, got ${version}`);
        }
        await prepareSource(manifest.source, root, values.check ?? false);
      });
      console.log(`verified Ghostty ${manifest.revision} at ${join(root, "source")}`);
    }
  } catch (error) {
    console.error(`Ghostty preparation failed: ${error instanceof Error ? error.message : error}`);
    process.exitCode = 1;
  }
}
