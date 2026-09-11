/** Shared release inventory, assembly, draft validation, and publication. */
import { appendFile, copyFile, mkdir, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "..");
export const schemaAssets = ["huterm.schema.json", "huterm-theme.schema.json"] as const;
type JsonObject = Record<string, unknown>;
export interface BuildInputs { sha: string; version: string }
export interface ReleaseInputs extends BuildInputs { tag: string }
interface ReleaseRecord { id: number; draft: boolean; prerelease: boolean; tag_name: string; target_commitish: string }
interface ReleaseAsset { name: string; size: number; state: string; digest: string | null }
export interface LocalAsset { name: string; path: string; size: number; digest: string }
interface CommandResult { stdout: string; stderr: string }

function objectValue(value: unknown, label: string): JsonObject {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${label} must be an object`);
  return value as JsonObject;
}

function requiredEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}

export function validateBuildInputs(sha: string, version: string): BuildInputs {
  if (!/^[0-9a-f]{40}$/.test(sha)) throw new Error(`invalid release SHA: ${sha}`);
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)) throw new Error(`invalid release version: ${version}`);
  return { sha, version };
}

export function validateReleaseInputs(sha: string, tag: string, version: string): ReleaseInputs {
  const build = validateBuildInputs(sha, version);
  if (tag !== `v${version}`) throw new Error(`release tag ${tag} does not match version ${version}`);
  return { ...build, tag };
}

function releaseRecords(value: unknown): ReleaseRecord[] {
  const pages = Array.isArray(value) ? value : [];
  return pages.flatMap(page => Array.isArray(page) ? page : [page]).map((item, index) => {
    const release = objectValue(item, `release ${index}`);
    if (typeof release.id !== "number" || !Number.isSafeInteger(release.id)) throw new Error(`release ${index} has an invalid id`);
    if (typeof release.draft !== "boolean" || typeof release.prerelease !== "boolean") throw new Error(`release ${index} has invalid state`);
    if (typeof release.tag_name !== "string" || typeof release.target_commitish !== "string") throw new Error(`release ${index} has invalid identity`);
    return release as unknown as ReleaseRecord;
  });
}

export function validateDraftRelease(value: unknown, inputs: ReleaseInputs, expectedId?: number): ReleaseRecord {
  const matching = releaseRecords(value).filter(release => release.tag_name === inputs.tag);
  if (matching.length !== 1) throw new Error(`expected one release for ${inputs.tag}, found ${matching.length}`);
  const release = matching[0]!;
  if (expectedId !== undefined && release.id !== expectedId) throw new Error(`release id ${release.id} does not match ${expectedId}`);
  if (!release.draft) throw new Error(`${inputs.tag} is not a draft release`);
  if (release.prerelease) throw new Error(`${inputs.tag} must not be a prerelease`);
  if (release.target_commitish !== inputs.sha) throw new Error(`${inputs.tag} targets ${release.target_commitish}, expected ${inputs.sha}`);
  return release;
}

export function validateWorkspaceVersions(value: unknown, expectedVersion: string): void {
  const metadata = objectValue(value, "Cargo metadata");
  if (!Array.isArray(metadata.packages)) throw new Error("Cargo metadata is missing packages");
  for (const name of ["huterm", "huterm-config", "huterm-core", "huterm-gpui", "huterm-protocol"]) {
    const matching = metadata.packages.filter(item => objectValue(item, "Cargo package").name === name);
    if (matching.length !== 1) throw new Error(`expected one Cargo package named ${name}, found ${matching.length}`);
    if (objectValue(matching[0], name).version !== expectedVersion) throw new Error(`${name} version does not match ${expectedVersion}`);
  }
}

export function isDispatchedBranchBuild(sha: string, event: string | undefined, ref: string | undefined, dispatchSha: string | undefined): boolean {
  return event === "workflow_dispatch" && ref?.startsWith("refs/heads/") === true && sha === dispatchSha;
}

export function releaseAssetNames(version: string) {
  validateBuildInputs("0".repeat(40), version);
  const macos = `Huterm-${version}-macOS-universal.zip`;
  const linuxX86 = [`Huterm-${version}-Linux-x86_64.AppImage`, `Huterm-${version}-Linux-x86_64.tar.gz`];
  const linuxArm = [`Huterm-${version}-Linux-aarch64.AppImage`, `Huterm-${version}-Linux-aarch64.tar.gz`];
  const platforms = [
    { id: "macos", payloads: [macos], manifest: "macos-SHA256SUMS" },
    { id: "linux-x86_64", payloads: linuxX86, manifest: "linux-x86_64-SHA256SUMS" },
    { id: "linux-aarch64", payloads: linuxArm, manifest: "linux-aarch64-SHA256SUMS" },
  ] as const;
  const payloads = [macos, ...linuxX86, ...linuxArm, ...schemaAssets];
  return { macos, platforms, payloads, checksums: "SHA256SUMS", all: [...payloads, "SHA256SUMS"] };
}

async function sha256(file: string): Promise<string> {
  return new Bun.CryptoHasher("sha256").update(await Bun.file(file).arrayBuffer()).digest("hex");
}

export async function writePlatformManifest(destination: string, files: string[]): Promise<void> {
  const names = files.map(file => basename(file));
  if (new Set(names).size !== names.length) throw new Error("platform payload names must be unique");
  const lines: string[] = [];
  for (const [index, file] of files.entries()) {
    const size = (await stat(file)).size;
    if (size <= 0) throw new Error(`${names[index]} is empty`);
    lines.push(`${await sha256(file)}  ${names[index]}\n`);
  }
  await writeFile(destination, lines.sort().join(""));
}

async function verifyPlatformManifest(directory: string, manifestName: string, expectedNames: readonly string[]): Promise<void> {
  const actualFiles = (await readdir(directory)).sort();
  const expectedFiles = [...expectedNames, manifestName].sort();
  if (actualFiles.join("\n") !== expectedFiles.join("\n")) throw new Error(`${basename(directory)} contains missing or unexpected platform files: ${actualFiles.join(", ")}`);
  const expectedLines = await Promise.all(expectedNames.map(async name => {
    const file = join(directory, name);
    if ((await stat(file)).size <= 0) throw new Error(`${name} is empty`);
    return `${await sha256(file)}  ${name}\n`;
  }));
  if (await readFile(join(directory, manifestName), "utf8") !== expectedLines.sort().join("")) throw new Error(`${manifestName} digest manifest does not match platform payloads`);
}

export async function assembleReleaseArtifacts(inputs: BuildInputs, incoming: string, dist: string, schemas = join(repoRoot, "schemas")): Promise<void> {
  const names = releaseAssetNames(inputs.version);
  const staging = `${dist}.staging-${process.pid}`;
  await rm(staging, { recursive: true, force: true });
  await mkdir(staging, { recursive: true });
  try {
    for (const platform of names.platforms) {
      const directory = join(incoming, platform.id);
      await verifyPlatformManifest(directory, platform.manifest, platform.payloads);
      for (const payload of platform.payloads) await copyFile(join(directory, payload), join(staging, payload));
    }
    for (const schema of schemaAssets) await copyFile(join(schemas, schema), join(staging, schema));
    await writePlatformManifest(join(staging, names.checksums), names.payloads.map(name => join(staging, name)));
    await verifyLocalAssets(inputs, staging, schemas);
    await rm(dist, { recursive: true, force: true });
    await mkdir(dist, { recursive: true });
    for (const name of names.all) await copyFile(join(staging, name), join(dist, name));
  } finally { await rm(staging, { recursive: true, force: true }); }
}

export async function verifyLocalAssets(inputs: BuildInputs, dist = requiredEnv("RELEASE_DIST_DIR"), schemas = join(repoRoot, "schemas")): Promise<LocalAsset[]> {
  const names = releaseAssetNames(inputs.version);
  const files = (await readdir(dist)).sort();
  const expected = [...names.all].sort();
  if (files.join("\n") !== expected.join("\n")) throw new Error(`release directory contains missing or unexpected files: ${files.join(", ")}`);
  const assets: LocalAsset[] = [];
  for (const name of expected) {
    const file = join(dist, name);
    const size = (await stat(file)).size;
    if (size <= 0) throw new Error(`${name} is empty`);
    assets.push({ name, path: file, size, digest: await sha256(file) });
  }
  for (const name of schemaAssets) if (!(await readFile(join(schemas, name))).equals(await readFile(join(dist, name)))) throw new Error(`${name} does not match the release checkout`);
  const checksumLines = await Promise.all(names.payloads.map(async name => `${await sha256(join(dist, name))}  ${name}\n`));
  if (await readFile(join(dist, names.checksums), "utf8") !== checksumLines.sort().join("")) throw new Error("SHA256SUMS does not match the release payloads");
  return assets;
}

function normalizedAssets(value: unknown): ReleaseAsset[] {
  const pages = Array.isArray(value) ? value : [];
  return pages.flatMap(page => Array.isArray(page) ? page : [page]).map((item, index) => {
    const asset = objectValue(item, `release asset ${index}`);
    if (typeof asset.name !== "string" || typeof asset.size !== "number" || typeof asset.state !== "string") throw new Error(`release asset ${index} is malformed`);
    if (asset.digest !== null && typeof asset.digest !== "string") throw new Error(`release asset ${index} has an invalid digest`);
    return asset as unknown as ReleaseAsset;
  });
}

export function validateReleaseAssets(value: unknown, localAssets: LocalAsset[]): void {
  const remote = normalizedAssets(value).sort((left, right) => left.name.localeCompare(right.name));
  const expected = [...localAssets].sort((left, right) => left.name.localeCompare(right.name));
  if (remote.map(asset => asset.name).join("\n") !== expected.map(asset => asset.name).join("\n")) throw new Error(`release asset names do not match: got [${remote.map(asset => asset.name).join(", ")}]`);
  for (const local of expected) {
    const asset = remote.find(item => item.name === local.name)!;
    if (asset.state !== "uploaded") throw new Error(`${local.name} is not a complete upload`);
    if (asset.size !== local.size || asset.size <= 0) throw new Error(`${local.name} size ${asset.size} does not match ${local.size}`);
    if (asset.digest !== `sha256:${local.digest}`) throw new Error(`${local.name} digest does not match the local asset`);
  }
}

function environment(additions: Record<string, string> = {}): Record<string, string> {
  const result: Record<string, string> = { ...additions };
  for (const [key, value] of Object.entries(process.env)) if (value !== undefined && result[key] === undefined) result[key] = value;
  return result;
}

async function runCaptured(command: string, args: string[]): Promise<CommandResult> {
  const child = Bun.spawn([command, ...args], { cwd: repoRoot, env: environment(), stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, status] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  if (status !== 0) throw new Error(`${command} exited with status ${status}: ${stderr.trim()}`);
  return { stdout, stderr };
}

async function runInherited(command: string, args: string[]): Promise<void> {
  const child = Bun.spawn([command, ...args], { cwd: repoRoot, env: environment(), stdin: "ignore", stdout: "inherit", stderr: "inherit" });
  if (await child.exited !== 0) throw new Error(`${command} failed`);
}

function repository(): string {
  const value = requiredEnv("GITHUB_REPOSITORY");
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(value)) throw new Error(`invalid GITHUB_REPOSITORY: ${value}`);
  return value;
}

async function releaseList(): Promise<unknown> { return JSON.parse((await runCaptured("gh", ["api", "--paginate", "--slurp", `repos/${repository()}/releases?per_page=100`])).stdout); }
async function assetList(id: number): Promise<unknown> { return JSON.parse((await runCaptured("gh", ["api", "--paginate", "--slurp", `repos/${repository()}/releases/${id}/assets?per_page=100`])).stdout); }

async function validateRepositorySource(inputs: BuildInputs, allowBranch = false): Promise<void> {
  const head = (await runCaptured("git", ["rev-parse", "HEAD"])).stdout.trim();
  if (head !== inputs.sha) throw new Error(`checkout ${head} does not match release SHA ${inputs.sha}`);
  if (!allowBranch) await runCaptured("git", ["merge-base", "--is-ancestor", inputs.sha, "refs/remotes/origin/main"]);
  validateWorkspaceVersions(JSON.parse((await runCaptured("cargo", ["metadata", "--locked", "--no-deps", "--format-version", "1"])).stdout), inputs.version);
}

async function validateRepositoryRelease(inputs: ReleaseInputs, expectedId?: number): Promise<ReleaseRecord> {
  const release = validateDraftRelease(await releaseList(), inputs, expectedId);
  await validateRepositorySource(inputs);
  const tagSha = (await runCaptured("git", ["rev-parse", `${inputs.tag}^{commit}`])).stdout.trim();
  if (tagSha !== inputs.sha) throw new Error(`${inputs.tag} points at ${tagSha}, expected ${inputs.sha}`);
  return release;
}

function currentBuildInputs(): BuildInputs { return validateBuildInputs(requiredEnv("RELEASE_SHA"), requiredEnv("RELEASE_VERSION")); }
function currentReleaseInputs(): ReleaseInputs { return validateReleaseInputs(requiredEnv("RELEASE_SHA"), requiredEnv("RELEASE_TAG"), requiredEnv("RELEASE_VERSION")); }

async function validateBuildCommand(): Promise<void> {
  const inputs = currentBuildInputs();
  await validateRepositorySource(inputs, isDispatchedBranchBuild(inputs.sha, process.env.GITHUB_EVENT_NAME, process.env.GITHUB_REF, process.env.GITHUB_SHA));
  console.log(`validated build ${inputs.version} at ${inputs.sha}`);
}

async function validateReleaseCommand(): Promise<void> {
  const release = await validateRepositoryRelease(currentReleaseInputs());
  await appendFile(requiredEnv("GITHUB_OUTPUT"), `release_id=${release.id}\n`);
  console.log(`validated draft ${release.tag_name} at ${release.target_commitish}`);
}

async function uploadAssets(): Promise<void> {
  const inputs = currentReleaseInputs();
  const releaseId = Number(requiredEnv("RELEASE_ID"));
  if (!Number.isSafeInteger(releaseId)) throw new Error("RELEASE_ID must be an integer");
  await validateRepositoryRelease(inputs, releaseId);
  const local = await verifyLocalAssets(inputs);
  await runInherited("gh", ["release", "upload", inputs.tag, ...local.map(asset => asset.path), "--clobber", "--repo", repository()]);
  let lastError: unknown;
  for (let attempt = 1; attempt <= 5; attempt++) {
    try { validateReleaseAssets(await assetList(releaseId), local); console.log(`verified ${local.length} draft release assets`); return; }
    catch (error) { lastError = error; if (attempt < 5) await Bun.sleep(attempt * 2_000); }
  }
  throw lastError;
}

async function publishRelease(): Promise<void> {
  const inputs = currentReleaseInputs();
  const releaseId = Number(requiredEnv("RELEASE_ID"));
  if (!Number.isSafeInteger(releaseId)) throw new Error("RELEASE_ID must be an integer");
  await validateRepositoryRelease(inputs, releaseId);
  validateReleaseAssets(await assetList(releaseId), await verifyLocalAssets(inputs));
  const published = objectValue(JSON.parse((await runCaptured("gh", ["api", "--method", "PATCH", `repos/${repository()}/releases/${releaseId}`, "-F", "draft=false", "-f", "make_latest=true"])).stdout), "published release");
  if (published.id !== releaseId || published.tag_name !== inputs.tag || published.target_commitish !== inputs.sha || published.draft !== false) throw new Error("GitHub returned an unexpected published release");
  console.log(`published ${inputs.tag} at ${inputs.sha}`);
}

async function main(): Promise<void> {
  switch (Bun.argv[2]) {
    case "validate-build": await validateBuildCommand(); break;
    case "validate-release": await validateReleaseCommand(); break;
    case "write-platform-manifest": {
      const destination = Bun.argv[3]; const files = Bun.argv.slice(4);
      if (!destination || files.length === 0) throw new Error("write-platform-manifest requires a destination and payload files");
      await writePlatformManifest(resolve(destination), files.map(file => resolve(file)));
      break;
    }
    case "assemble": await assembleReleaseArtifacts(currentBuildInputs(), requiredEnv("RELEASE_INCOMING_DIR"), requiredEnv("RELEASE_DIST_DIR")); break;
    case "verify-assets": await verifyLocalAssets(currentBuildInputs()); break;
    case "upload-assets": await uploadAssets(); break;
    case "publish": await publishRelease(); break;
    default: throw new Error("expected validate-build, validate-release, write-platform-manifest, assemble, verify-assets, upload-assets, or publish");
  }
}

if (import.meta.main) {
  try { await main(); } catch (error) { console.error(`Release failed: ${error instanceof Error ? error.message : String(error)}`); process.exitCode = 1; }
}
