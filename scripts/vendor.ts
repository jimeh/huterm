/** Reproduce patched registry crates without modifying the build inputs. */
import { createHash } from "node:crypto";
import { cpSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, readlinkSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

export type Source = {
  name: string;
  version: string;
  url: string;
  sha256: string;
  patches: { name: string; file: string; description: string; upstream?: string }[];
  upstream: { repository: string; revision: string; path: string; dirty?: boolean };
};
const repository = resolve(import.meta.dir, "..");
const vendorRoot = join(repository, "third-party/vendor");
export const hash = (bytes: Uint8Array) => createHash("sha256").update(bytes).digest("hex");
export const id = (source: Source) => `${source.name}-${source.version}`;

export function readSources(file: string): Source[] {
  const data = JSON.parse(readFileSync(file, "utf8"));
  if (data?.schema !== 2 || !Array.isArray(data.sources)) throw new Error("invalid vendor manifest");
  const names = new Set<string>();
  const patches = new Set<string>();
  for (const source of data.sources) {
    if (!source || typeof source.name !== "string" || !/^[a-zA-Z0-9_-]+$/.test(source.name) ||
        typeof source.version !== "string" || !/^\d+\.\d+\.\d+(?:[-+][a-zA-Z0-9.-]+)?$/.test(source.version) ||
        source.url !== `https://static.crates.io/crates/${source.name}/${id(source)}.crate` ||
        typeof source.sha256 !== "string" || !/^[0-9a-f]{64}$/.test(source.sha256) ||
        !source.upstream || typeof source.upstream.repository !== "string" || !source.upstream.repository.startsWith("https://") ||
        typeof source.upstream.revision !== "string" || !/^[0-9a-f]{40}$/.test(source.upstream.revision) ||
        typeof source.upstream.path !== "string" || (source.upstream.dirty !== undefined && typeof source.upstream.dirty !== "boolean") ||
        !Array.isArray(source.patches) || names.has(source.name)) throw new Error("invalid or duplicate vendor source");
    names.add(source.name);
    const patchNames = new Set<string>();
    for (const patch of source.patches) {
      if (!patch || typeof patch.name !== "string" || !/^[a-z0-9][a-z0-9-]*$/.test(patch.name) ||
          typeof patch.file !== "string" || !/^patches\/(?:[a-zA-Z0-9_-]+\/)*[a-zA-Z0-9_.-]+\.patch$/.test(patch.file) ||
          typeof patch.description !== "string" || !patch.description.trim() ||
          (patch.upstream !== undefined && (typeof patch.upstream !== "string" || !patch.upstream.startsWith("https://"))) ||
          patchNames.has(patch.name) || patches.has(patch.file)) throw new Error("invalid or duplicate vendor patch");
      patchNames.add(patch.name);
      patches.add(patch.file);
    }
  }
  return data.sources;
}

function checkedArchive(file: string, source: Source): string {
  if (hash(readFileSync(file)) !== source.sha256) throw new Error(`archive checksum mismatch: ${file}`);
  return file;
}

export async function archiveFor(source: Source, cache: string, cargoHome = process.env.CARGO_HOME ?? join(homedir(), ".cargo")): Promise<string> {
  mkdirSync(cache, { recursive: true });
  const destination = join(cache, `${source.sha256}.crate`);
  if (lstatSync(destination, { throwIfNoEntry: false })) return checkedArchive(destination, source);
  const registry = join(cargoHome, "registry/cache");
  for (const registryName of existsSync(registry) ? readdirSync(registry, { withFileTypes: true }) : []) {
    if (!registryName.isDirectory()) continue;
    const candidate = join(registry, registryName.name, `${id(source)}.crate`);
    if (lstatSync(candidate, { throwIfNoEntry: false })) return checkedArchive(candidate, source);
  }
  const response = await fetch(source.url, { signal: AbortSignal.timeout(120_000) });
  if (!response.ok) throw new Error(`archive download failed: HTTP ${response.status} for ${id(source)}`);
  // Buffer HTTPS responses before writing; Bun's streaming write can stall on Linux.
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (hash(bytes) !== source.sha256) throw new Error(`archive checksum mismatch for ${id(source)}`);
  const stage = mkdtempSync(join(cache, ".download-"));
  try {
    writeFileSync(join(stage, "archive"), bytes);
    renameSync(join(stage, "archive"), destination);
  } finally { rmSync(stage, { recursive: true, force: true }); }
  return destination;
}

/** Include hidden files, executable bits, and symlink targets; Git cannot retain empty directories. */
export function treeEntries(root: string): Map<string, string> {
  if (!lstatSync(root).isDirectory()) throw new Error(`expected source directory: ${root}`);
  const entries = new Map<string, string>();
  function visit(directory: string, prefix: string) {
    for (const name of readdirSync(directory).sort()) {
      if (name === ".git") throw new Error(`unexpected Git metadata in vendor source: ${directory}`);
      const file = join(directory, name);
      const key = `${prefix}${name}`;
      const info = lstatSync(file);
      if (info.isDirectory()) visit(file, `${key}/`);
      else if (info.isSymbolicLink()) entries.set(key, `link:${readlinkSync(file)}`);
      else if (info.isFile()) entries.set(key, `file:${info.mode & 0o111 ? "x" : "-"}:${hash(readFileSync(file))}`);
      else throw new Error(`unsupported vendor entry: ${file}`);
    }
  }
  visit(root, "");
  return entries;
}

export function gitBytes(cwd: string, args: string[], diff = false) {
  const env = { ...process.env };
  for (const key of Object.keys(env)) if (key.startsWith("GIT_")) delete env[key];
  // Scratch trees may be inside Huterm: never discover its ancestor repository.
  env.GIT_CEILING_DIRECTORIES = dirname(resolve(cwd));
  env.GIT_CONFIG_NOSYSTEM = "1";
  env.GIT_CONFIG_GLOBAL = "/dev/null";
  env.GIT_EDITOR = "true";
  env.GIT_TERMINAL_PROMPT = "0";
  const result = Bun.spawnSync(["git", "-c", "core.filemode=true", "-c", "core.autocrlf=false", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", "-c", "gc.auto=0", "-c", "maintenance.auto=false", "-c", "user.name=Vendor tooling", "-c", "user.email=vendor@localhost", ...args], { cwd, env, stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0 && !(diff && result.exitCode === 1)) throw new Error(`git ${args[0]} failed: ${result.stderr.toString().trim()}`);
  return result.stdout;
}

export function git(cwd: string, args: string[], diff = false): string {
  return gitBytes(cwd, args, diff).toString();
}

export function differences(expected: Map<string, string>, actual: Map<string, string>): string[] {
  return [...new Set([...expected.keys(), ...actual.keys()])].sort().filter((key) => expected.get(key) !== actual.get(key));
}

export async function extract(source: Source, archive: string, destination: string): Promise<void> {
  const stage = mkdtempSync(join(tmpdir(), "huterm-vendor-unpack-"));
  try {
    await new Bun.Archive(readFileSync(checkedArchive(archive, source))).extract(stage);
    const children = readdirSync(stage);
    if (children.length !== 1 || children[0] !== id(source)) throw new Error(`unexpected archive root for ${id(source)}`);
    const original = join(stage, id(source));
    treeEntries(original);
    const vcs = JSON.parse(readFileSync(join(original, ".cargo_vcs_info.json"), "utf8"));
    if (vcs.git?.sha1 !== source.upstream.revision || vcs.path_in_vcs !== source.upstream.path ||
        Boolean(vcs.git?.dirty) !== Boolean(source.upstream.dirty)) throw new Error(`archive VCS metadata differs from manifest: ${id(source)}`);
    cpSync(original, destination, { recursive: true, dereference: false, verbatimSymlinks: true });
  } finally { rmSync(stage, { recursive: true, force: true }); }
}

export function checkIdentity(source: Source, current: string): void {
  const crate = Bun.TOML.parse(readFileSync(join(current, "Cargo.toml"), "utf8")) as { package?: { name?: string; version?: string } };
  if (crate.package?.name !== source.name || crate.package?.version !== source.version) throw new Error(`crate identity differs from manifest: ${id(source)}`);
}

export function applyPatch(directory: string, file: string): void {
  if (readFileSync(file, "utf8").trim()) git(directory, ["apply", "--whitespace=nowarn", resolve(file)]);
}

export function diffTrees(before: string, after: string): string {
  const stage = mkdtempSync(join(tmpdir(), "huterm-vendor-diff-"));
  try {
    cpSync(before, join(stage, "a"), { recursive: true, dereference: false, verbatimSymlinks: true });
    cpSync(after, join(stage, "b"), { recursive: true, dereference: false, verbatimSymlinks: true });
    return git(stage, ["diff", "--no-index", "--no-prefix", "--no-ext-diff", "--no-textconv", "--binary", "--full-index", "--", "a", "b"], true);
  } finally { rmSync(stage, { recursive: true, force: true }); }
}

export async function reproduce(source: Source, root: string, archive: string): Promise<void> {
  const stage = mkdtempSync(join(tmpdir(), "huterm-vendor-"));
  try {
    await extract(source, archive, stage);
    const current = join(root, id(source));
    const actual = treeEntries(current);
    checkIdentity(source, current);
    for (const patch of source.patches) applyPatch(stage, join(root, patch.file));
    const changed = differences(treeEntries(stage), actual);
    if (changed.length) throw new Error(`${id(source)} differs from archive + patches:\n${changed.map((file) => `  ${file}`).join("\n")}\nUse vendor:status to resume an edit session; do not refresh unrelated drift.`);
  } finally { rmSync(stage, { recursive: true, force: true }); }
}

async function main() {
  const [mode, selected, patch, ...rest] = Bun.argv.slice(2);
  if (!mode || !["check", "start", "finish", "continue", "status", "cancel", "reopen"].includes(mode) ||
      (rest.length > 0 && !(mode === "start" && rest.length === 1 && rest[0] === "--adopt-edits")) ||
      (mode === "start" ? !selected || !patch : patch !== undefined) ||
      (!["check", "status"].includes(mode) && !selected)) {
    throw new Error("usage: vendor.ts check|status [crate] | start <crate> <patch> [--adopt-edits] | finish|continue|reopen|cancel <crate>");
  }
  const sources = readSources(join(vendorRoot, "sources.json"));
  const cargo = Bun.TOML.parse(readFileSync(join(repository, "Cargo.toml"), "utf8")) as { patch?: { "crates-io"?: Record<string, { path?: string }> } };
  const overrides = cargo.patch?.["crates-io"] ?? {};
  if (Object.keys(overrides).length !== sources.length || sources.some((source) => overrides[source.name]?.path !== `third-party/vendor/${id(source)}`)) throw new Error("Cargo patches differ from the vendor manifest");
  const wanted = selected ? sources.filter((source) => source.name === selected) : sources;
  if (selected && !wanted.length) throw new Error(`unknown vendored crate: ${selected}`);
  const { runSession, sessionStatus, withVendorLock } = await import("./vendor-session");
  const storage = join(repository, ".native/vendor");
  for (const source of wanted) {
    await withVendorLock(storage, source.name, async () => {
      if (mode === "status") console.log(sessionStatus(source, storage));
      else if (mode === "check") {
        const status = sessionStatus(source, storage);
        if (!status.endsWith(": no active session")) throw new Error(`${status}\nFinish or cancel this session before verification.`);
        await reproduce(source, vendorRoot, await archiveFor(source, join(storage, "archives")));
        console.log(`verified ${id(source)}: archive + patches matches vendored source`);
      } else {
        const archive = mode === "start" ? await archiveFor(source, join(storage, "archives")) : undefined;
        await runSession(mode, source, vendorRoot, storage, patch, archive, rest[0] === "--adopt-edits");
        console.log(sessionStatus(source, storage));
      }
    });
  }
  if (!sources.length) console.log("no vendored crates to verify");
}

if (import.meta.main) {
  try { await main(); }
  catch (error) { console.error(error instanceof Error ? error.message : error); process.exitCode = 1; }
}
