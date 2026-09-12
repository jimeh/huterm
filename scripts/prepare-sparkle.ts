/** Prepare and verify the pinned Sparkle binary distribution. */
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readlinkSync, readdirSync, renameSync, rmSync } from "node:fs";
import { basename, dirname, join, relative, resolve, sep } from "node:path";
import { parseArgs } from "node:util";
import { fileHash, treeHash, withPrepareLock } from "./prepare-ghostty.ts";

export interface SparkleManifest {
  version: string;
  published_at: string;
  minimum_macos: string;
  source: { name: string; url: string; sha256: string; tree_sha256: string };
  framework: { identifier: string; license_sha256: string };
  license_url: string;
}

const repository = resolve(import.meta.dir, "..");

function object(value: unknown, label: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${label} must be an object`);
  return value as Record<string, unknown>;
}

export function readSparkleManifest(manifestPath: string): SparkleManifest {
  const value = object(JSON.parse(readFileSync(manifestPath, "utf8")), "Sparkle manifest");
  const source = object(value.source, "Sparkle source");
  const framework = object(value.framework, "Sparkle framework");
  for (const [label, candidate, pattern] of [
    ["version", value.version, /^\d+\.\d+\.\d+$/],
    ["published_at", value.published_at, /^\d{4}-\d{2}-\d{2}T/],
    ["minimum_macos", value.minimum_macos, /^\d+\.\d+(?:\.\d+)?$/],
    ["source name", source.name, /^Sparkle-[\w.-]+\.tar\.xz$/],
    ["source sha256", source.sha256, /^[0-9a-f]{64}$/],
    ["source tree_sha256", source.tree_sha256, /^[0-9a-f]{64}$/],
    ["framework identifier", framework.identifier, /^org\.sparkle-project\.Sparkle$/],
    ["license sha256", framework.license_sha256, /^[0-9a-f]{64}$/],
  ] as const) {
    if (typeof candidate !== "string" || !pattern.test(candidate)) throw new Error(`invalid Sparkle ${label}`);
  }
  if (typeof source.name !== "string" || typeof source.url !== "string" ||
      typeof value.version !== "string" || typeof value.license_url !== "string" ||
      source.name !== basename(source.name) ||
      source.url !== `https://github.com/sparkle-project/Sparkle/releases/download/${value.version}/${source.name}` ||
      value.license_url !== `https://github.com/sparkle-project/Sparkle/blob/${value.version}/LICENSE`) {
    throw new Error("invalid Sparkle provenance");
  }
  return value as unknown as SparkleManifest;
}

function plistValue(source: string, key: string): string | undefined {
  return new RegExp(`<key>${key}</key>\\s*<string>([^<]+)</string>`).exec(source)?.[1];
}

function validateSymlinks(root: string, directory = root): void {
  for (const name of readdirSync(directory)) {
    const entry = join(directory, name);
    const info = lstatSync(entry);
    if (info.isDirectory()) validateSymlinks(root, entry);
    if (!info.isSymbolicLink()) continue;
    const target = resolve(dirname(entry), readlinkSync(entry));
    if (target !== root && !target.startsWith(`${root}${sep}`)) throw new Error(`Sparkle archive symlink escapes extraction root: ${relative(root, entry)}`);
  }
}

export function verifySparkleDistribution(root: string, manifest: SparkleManifest): void {
  const info = lstatSync(root, { throwIfNoEntry: false });
  if (!info?.isDirectory() || info.isSymbolicLink() || treeHash(root) !== manifest.source.tree_sha256) {
    throw new Error(`Sparkle contents differ from the pin: ${root}; remove this generated directory and prepare again`);
  }
  validateSymlinks(root);
  const framework = join(root, "Sparkle.framework");
  const current = join(framework, "Versions/Current");
  if (!lstatSync(current).isSymbolicLink() || readlinkSync(current) !== "B") throw new Error("Sparkle framework has an unexpected Current version link");
  for (const tool of ["generate_appcast", "generate_keys", "sign_update"]) {
    const toolPath = join(root, "bin", tool);
    if (!lstatSync(toolPath).isFile() || (lstatSync(toolPath).mode & 0o111) === 0) throw new Error(`Sparkle tool is missing or not executable: ${tool}`);
  }
  const plist = readFileSync(join(framework, "Versions/B/Resources/Info.plist"), "utf8");
  if (plistValue(plist, "CFBundleIdentifier") !== manifest.framework.identifier ||
      plistValue(plist, "CFBundleShortVersionString") !== manifest.version ||
      plistValue(plist, "LSMinimumSystemVersion") !== manifest.minimum_macos) {
    throw new Error("Sparkle framework identity or minimum macOS version differs from the pin");
  }
  if (fileHash(join(root, "LICENSE")) !== manifest.framework.license_sha256) throw new Error("Sparkle license differs from the reviewed notice");
}

export function verifySparklePolicy(
  manifest: SparkleManifest,
  licensePath: string,
  now = new Date(),
): void {
  const published = new Date(manifest.published_at);
  if (Number.isNaN(published.valueOf())) throw new Error("Sparkle publication date is invalid");
  if (now.valueOf() - published.valueOf() < 3 * 24 * 60 * 60 * 1_000) {
    throw new Error(`Sparkle ${manifest.version} has not cleared the three-day release-age policy`);
  }
  if (fileHash(licensePath) !== manifest.framework.license_sha256) {
    throw new Error("committed Sparkle license differs from the reviewed distribution notice");
  }
}

async function run(command: string, args: string[], cwd?: string): Promise<string> {
  const processHandle = Bun.spawn([command, ...args], { cwd, stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, status] = await Promise.all([new Response(processHandle.stdout).text(), new Response(processHandle.stderr).text(), processHandle.exited]);
  if (status !== 0) throw new Error(`${command} failed with status ${status}: ${stderr.trim()}`);
  return stdout;
}

async function download(manifest: SparkleManifest, directory: string): Promise<string> {
  const destination = join(directory, manifest.source.name);
  if (existsSync(destination)) {
    if (fileHash(destination) !== manifest.source.sha256) throw new Error(`Sparkle archive checksum mismatch: ${destination}`);
    return destination;
  }
  const stage = mkdtempSync(join(directory, ".download-"));
  const temporary = join(stage, "archive");
  try {
    const response = await fetch(manifest.source.url, { headers: { "User-Agent": "huterm-native-prepare" }, signal: AbortSignal.timeout(120_000) });
    if (!response.ok) throw new Error(`Sparkle download failed: HTTP ${response.status}`);
    await Bun.write(temporary, await response.arrayBuffer());
    if (fileHash(temporary) !== manifest.source.sha256) throw new Error("Sparkle archive checksum mismatch");
    renameSync(temporary, destination);
  } finally {
    rmSync(stage, { recursive: true, force: true });
  }
  return destination;
}

export async function prepareSparkle(manifest: SparkleManifest, root: string, check: boolean): Promise<void> {
  const distribution = join(root, "distribution");
  if (lstatSync(distribution, { throwIfNoEntry: false })) {
    verifySparkleDistribution(distribution, manifest);
    return;
  }
  if (check) throw new Error(`missing Sparkle distribution: ${distribution}`);
  const archives = join(root, "archives");
  mkdirSync(archives, { recursive: true });
  const archive = await download(manifest, archives);
  const names = (await run("tar", ["-tf", archive])).split(/\r?\n/).filter(Boolean);
  if (names.some(name => name.startsWith("/") || name.split("/").includes(".."))) throw new Error("Sparkle archive contains an unsafe path");
  const stage = mkdtempSync(join(root, ".extract-"));
  try {
    await run("tar", ["-xf", archive, "-C", stage]);
    validateSymlinks(stage);
    verifySparkleDistribution(stage, manifest);
    renameSync(stage, distribution);
  } finally {
    rmSync(stage, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  try {
    const { values } = parseArgs({ args: Bun.argv.slice(2), options: { directory: { type: "string" }, manifest: { type: "string" }, check: { type: "boolean" }, "manifest-only": { type: "boolean" }, help: { type: "boolean" } } });
    if (values.help) {
      console.log("Prepare verified Sparkle binaries: [--directory DIR] [--manifest FILE] [--check]");
    } else {
      const manifest = readSparkleManifest(resolve(values.manifest ?? join(import.meta.dir, "sparkle-source.json")));
      verifySparklePolicy(manifest, join(repository, "third-party/sparkle/LICENSE"));
      if (values["manifest-only"]) {
        console.log(`verified Sparkle ${manifest.version} provenance and license policy`);
        process.exit(0);
      }
      const root = resolve(values.directory ?? join(repository, ".native/sparkle"));
      mkdirSync(root, { recursive: true });
      await withPrepareLock(root, () => prepareSparkle(manifest, root, values.check ?? false));
      console.log(`verified Sparkle ${manifest.version} at ${join(root, "distribution")}`);
    }
  } catch (error) {
    console.error(`Sparkle preparation failed: ${error instanceof Error ? error.message : error}`);
    process.exitCode = 1;
  }
}
