/** Build and verify relocatable Linux tarballs and AppImages. */
import { createHash } from "node:crypto";
import { constants } from "node:fs";
import { chmod, copyFile, cp, mkdir, mkdtemp, open, readFile, readdir, readlink, realpath, rename, rm, stat, symlink, utimes, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, join, relative, resolve, sep } from "node:path";

const repoRoot = resolve(import.meta.dir, "..");
const linuxAssets = join(repoRoot, "assets/linux");
const toolManifestPath = join(linuxAssets, "appimage-tools.json");
const policyPath = join(linuxAssets, "package-policy.json");
const maximumGlibc = "2.35";

export type LinuxArchitecture = "x86_64" | "aarch64";
interface ToolAsset { url: string; sha256: string }
interface ToolDefinition { version: string; x86_64: ToolAsset; aarch64: ToolAsset }
interface ToolManifest { version: number; tools: { appimagetool: ToolDefinition; runtime: ToolDefinition } }
interface PrivateLibraryPolicy { licenseSource: string; licenseFile: string }
interface NoticePolicy { source: string; target: string }
interface AppImageNoticePolicy extends NoticePolicy { sourceUrl: string; sha256: string }
interface AppImageEnvelopePolicy { noticeDirectory: string; symlinks: Record<string, string>; files: Record<string, string>; notices: AppImageNoticePolicy[] }
interface PackagePolicy { version: number; privateLibraries: Record<string, PrivateLibraryPolicy>; hostLibraries: string[]; notices: NoticePolicy[]; appImageEnvelope: AppImageEnvelopePolicy }
interface PrivateLibraryRecord { file: string; soname: string; sha256: string; binaryPackage: string; sourcePackage: string; packageVersion: string; licenseFile: string }
interface PackageManifest { version: number; architecture: LinuxArchitecture; releaseCommit: string; sourceDateEpoch: number; tools: { appimagetool: string; runtime: string }; privateLibraries: PrivateLibraryRecord[] }
interface CommandResult { stdout: string; stderr: string; exitCode: number }

function record(value: unknown, label: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${label} must be an object`);
  return value as Record<string, unknown>;
}

function stringField(value: Record<string, unknown>, key: string, label: string): string {
  const field = value[key];
  if (typeof field !== "string" || field.length === 0) throw new Error(`${label}.${key} must be a non-empty string`);
  return field;
}

export function normalizeLinuxArchitecture(value: string): LinuxArchitecture {
  switch (value.toLowerCase()) {
    case "amd64": case "x86_64": return "x86_64";
    case "arm64": case "aarch64": return "aarch64";
    default: throw new Error(`unsupported Linux architecture ${JSON.stringify(value)}; expected x86_64 or aarch64`);
  }
}

export function validateDependencyPolicy(privateLibraries: string[], hostLibraries: string[], needed: string[]): void {
  const privateSet = new Set(privateLibraries);
  const hostSet = new Set(hostLibraries);
  for (const name of privateSet) if (hostSet.has(name)) throw new Error(`dynamic dependency is both private and host-owned: ${name}`);
  for (const name of needed) if (!privateSet.has(name) && !hostSet.has(name)) throw new Error(`unclassified dynamic dependency: ${name}`);
}

export function validateRunpath(value: string, file: string, library = false): void {
  const expected = library ? "$ORIGIN" : "$ORIGIN/../lib/huterm";
  const components = value.split(":");
  const unsafe = value !== expected || components.some(component => component.length === 0 || component.startsWith("/") || !component.startsWith("$ORIGIN"));
  if (unsafe) throw new Error(`unsafe runpath on ${file}: ${JSON.stringify(value)}; expected ${expected}`);
}

export function assertElfArchitecture(bytes: Uint8Array, expectedValue: string): LinuxArchitecture {
  const expected = normalizeLinuxArchitecture(expectedValue);
  const buffer = Buffer.from(bytes);
  if (buffer.length < 20 || buffer.subarray(0, 4).toString("hex") !== "7f454c46") throw new Error("file is not an ELF executable");
  if (buffer[5] !== 1 && buffer[5] !== 2) throw new Error("ELF header has an invalid byte order");
  const machine = buffer[5] === 1 ? buffer.readUInt16LE(18) : buffer.readUInt16BE(18);
  const actual = machine === 62 ? "x86_64" : machine === 183 ? "aarch64" : undefined;
  if (!actual) throw new Error(`unsupported ELF machine ${machine}`);
  if (actual !== expected) throw new Error(`ELF architecture ${actual}, expected ${expected}`);
  return actual;
}

function compareVersions(left: string, right: string): number {
  const a = left.split(".").map(Number);
  const b = right.split(".").map(Number);
  for (let index = 0; index < Math.max(a.length, b.length); index++) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
}

export function highestRequiredGlibc(symbolTable: string): { required: string; weak: string[] } {
  const required = new Set<string>();
  const weak = new Set<string>();
  for (const line of symbolTable.split(/\r?\n/)) {
    const version = /\(GLIBC_(\d+\.\d+)\)/.exec(line)?.[1];
    if (!version) continue;
    if (/\s[wW]\s+/.test(line)) weak.add(version); else required.add(version);
  }
  const highest = [...required].sort(compareVersions).at(-1) ?? "0.0";
  if (compareVersions(highest, maximumGlibc) > 0) throw new Error(`required GLIBC_${highest} exceeds ${maximumGlibc}`);
  return { required: highest, weak: [...weak].sort(compareVersions) };
}

export function validateGlibcVersionInfo(versionInfo: string, weakVersions: string[]): void {
  const weak = new Set(weakVersions);
  const tags = new Set([...versionInfo.matchAll(/\bName:\s+(GLIBC_[A-Za-z0-9_.-]+)/g)].map(match => match[1]!));
  for (const tag of tags) {
    const version = /^GLIBC_(\d+(?:\.\d+)+)$/.exec(tag)?.[1];
    if (!version) throw new Error(`unsupported glibc version tag ${tag}`);
    if (compareVersions(version, maximumGlibc) > 0 && !weak.has(version)) {
      throw new Error(`required ${tag} exceeds ${maximumGlibc}`);
    }
  }
}

export function validateResolvedLibraryEntries(
  bundle: string,
  privateNames: string[],
  hostNames: string[],
  entries: Map<string, string>,
): void {
  const privateSet = new Set(privateNames);
  const hostSet = new Set(hostNames);
  const bundleRoot = `${resolve(bundle)}${sep}`;
  for (const [name, filePath] of entries) {
    const inside = resolve(filePath).startsWith(bundleRoot);
    if (privateSet.has(name)) {
      if (!inside) throw new Error(`${name} resolved outside the bundle: ${filePath}`);
    } else if (hostSet.has(name)) {
      if (inside) throw new Error(`host-owned ${name} resolved inside the bundle`);
    } else {
      throw new Error(`unclassified resolved dependency: ${name} => ${filePath}`);
    }
  }
}

export async function withPrivateExecutableCopy<T>(source: string, operation: (executable: string) => Promise<T>): Promise<T> {
  const root = await mkdtemp(join(tmpdir(), "huterm-private-executable-"));
  const executable = join(root, basename(source));
  try {
    const bytes = await readRegularFile(source);
    const destination = await open(executable, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL, 0o700);
    try {
      await destination.writeFile(bytes);
      await destination.sync();
      await destination.chmod(0o700);
    } finally { await destination.close(); }
    return await operation(executable);
  } finally { await rm(root, { recursive: true, force: true }); }
}

export function validateToolManifest(value: unknown): ToolManifest {
  const manifest = record(value, "AppImage tool manifest");
  if (manifest.version !== 1) throw new Error("unsupported AppImage tool manifest version");
  const tools = record(manifest.tools, "AppImage tool manifest.tools");
  const parsed: Partial<ToolManifest["tools"]> = {};
  for (const toolName of ["appimagetool", "runtime"] as const) {
    const tool = record(tools[toolName], toolName);
    const version = stringField(tool, "version", toolName);
    const definition: Partial<ToolDefinition> = { version };
    for (const architecture of ["x86_64", "aarch64"] as const) {
      const asset = record(tool[architecture], `${toolName}.${architecture}`);
      const url = stringField(asset, "url", `${toolName}.${architecture}`);
      const sha256 = stringField(asset, "sha256", `${toolName}.${architecture}`);
      const parsedUrl = new URL(url);
      if (parsedUrl.protocol !== "https:" || parsedUrl.hostname !== "github.com" || !parsedUrl.pathname.includes(`/releases/download/${version}/`)) {
        throw new Error(`${toolName}.${architecture} must use a pinned release URL for ${version}`);
      }
      if (!/^[a-f0-9]{64}$/.test(sha256)) throw new Error(`${toolName}.${architecture} must have a lowercase SHA-256 digest`);
      definition[architecture] = { url, sha256 };
    }
    parsed[toolName] = definition as ToolDefinition;
  }
  return { version: 1, tools: parsed as ToolManifest["tools"] };
}

export function verifyToolBytes(bytes: Uint8Array, expected: string, label: string): void {
  const actual = createHash("sha256").update(bytes).digest("hex");
  if (actual !== expected) throw new Error(`${label} digest ${actual} does not match ${expected}`);
}

export function validatePackageManifest(value: unknown): PackageManifest {
  const manifest = record(value, "package manifest");
  if (manifest.version !== 1) throw new Error("unsupported package manifest version");
  const architecture = normalizeLinuxArchitecture(stringField(manifest, "architecture", "package manifest"));
  const releaseCommit = stringField(manifest, "releaseCommit", "package manifest");
  if (!/^[a-f0-9]{40}$/.test(releaseCommit)) throw new Error("package manifest.releaseCommit must be a full Git SHA");
  if (!Number.isSafeInteger(manifest.sourceDateEpoch) || Number(manifest.sourceDateEpoch) <= 0) throw new Error("package manifest.sourceDateEpoch must be a positive integer");
  const tools = record(manifest.tools, "package manifest.tools");
  const parsedTools = { appimagetool: stringField(tools, "appimagetool", "package manifest.tools"), runtime: stringField(tools, "runtime", "package manifest.tools") };
  if (!Array.isArray(manifest.privateLibraries)) throw new Error("package manifest.privateLibraries must be an array");
  const privateLibraries = manifest.privateLibraries.map((item, index) => {
    const library = record(item, `package manifest.privateLibraries[${index}]`);
    const parsed = {
      file: stringField(library, "file", `private library ${index}`),
      soname: stringField(library, "soname", `private library ${index}`),
      sha256: stringField(library, "sha256", `private library ${index}`),
      binaryPackage: stringField(library, "binaryPackage", `private library ${index}`),
      sourcePackage: stringField(library, "sourcePackage", `private library ${index}`),
      packageVersion: stringField(library, "packageVersion", `private library ${index}`),
      licenseFile: stringField(library, "licenseFile", `private library ${index}`),
    };
    if (!/^[a-f0-9]{64}$/.test(parsed.sha256)) throw new Error(`private library ${index}.sha256 must be a lowercase SHA-256 digest`);
    return parsed;
  });
  return { version: 1, architecture, releaseCommit, sourceDateEpoch: Number(manifest.sourceDateEpoch), tools: parsedTools, privateLibraries };
}

function mergedEnvironment(additions: Record<string, string | undefined> = {}): Record<string, string> {
  const environment: Record<string, string> = {};
  for (const [key, value] of Object.entries(process.env)) if (value !== undefined) environment[key] = value;
  for (const [key, value] of Object.entries(additions)) if (value === undefined) delete environment[key]; else environment[key] = value;
  return environment;
}

async function runCaptured(command: string, args: string[], options: { cwd?: string; env?: Record<string, string | undefined>; allowFailure?: boolean } = {}): Promise<CommandResult> {
  const child = Bun.spawn([command, ...args], { cwd: options.cwd ?? repoRoot, env: mergedEnvironment(options.env), stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, exitCode] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  if (exitCode !== 0 && !options.allowFailure) {
    if (stdout) process.stdout.write(stdout);
    if (stderr) process.stderr.write(stderr);
    throw new Error(`${command} exited with status ${exitCode}`);
  }
  return { stdout, stderr, exitCode };
}

async function runInherited(command: string, args: string[], options: { cwd?: string; env?: Record<string, string | undefined> } = {}): Promise<void> {
  const child = Bun.spawn([command, ...args], { cwd: options.cwd ?? repoRoot, env: mergedEnvironment(options.env), stdin: "ignore", stdout: "inherit", stderr: "inherit" });
  const status = await child.exited;
  if (status !== 0) throw new Error(`${command} exited with status ${status}`);
}

async function runToFile(command: string, args: string[], destination: string): Promise<void> {
  const child = Bun.spawn([command, ...args], { cwd: repoRoot, env: mergedEnvironment(), stdin: "ignore", stdout: Bun.file(destination), stderr: "inherit" });
  const status = await child.exited;
  if (status !== 0) throw new Error(`${command} exited with status ${status}`);
}

async function commandExists(command: string): Promise<boolean> {
  return (await runCaptured("sh", ["-c", "command -v \"$1\" >/dev/null 2>&1", "sh", command], { allowFailure: true })).exitCode === 0;
}

async function requireCommands(commands: string[]): Promise<void> {
  const missing: string[] = [];
  for (const command of commands) if (!await commandExists(command)) missing.push(command);
  if (missing.length > 0) throw new Error(`Linux packaging requires: ${missing.join(", ")}`);
}

async function sha256(file: string): Promise<string> { return createHash("sha256").update(await readRegularFile(file)).digest("hex"); }
async function readRegularFile(file: string): Promise<Buffer> {
  const handle = await open(file, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const before = await handle.stat({ bigint: true });
    if (!before.isFile()) throw new Error(`${file} is not a regular file`);
    const bytes = await handle.readFile();
    const after = await handle.stat({ bigint: true });
    if (before.dev !== after.dev || before.ino !== after.ino || before.size !== after.size ||
        before.mtimeNs !== after.mtimeNs || before.ctimeNs !== after.ctimeNs || BigInt(bytes.length) !== after.size) {
      throw new Error(`${file} changed while it was being read`);
    }
    return bytes;
  } finally { await handle.close(); }
}
function neededLibraries(dynamic: string): string[] { return [...dynamic.matchAll(/\(NEEDED\).*?\[([^\]]+)\]/g)].map(match => match[1]!).sort(); }
function dynamicRunpath(dynamic: string): string {
  const matches = [...dynamic.matchAll(/\((?:RUNPATH|RPATH)\).*?\[([^\]]*)\]/g)].map(match => match[1]!);
  if (matches.length !== 1) throw new Error(`expected one ELF runpath, found ${matches.length}`);
  return matches[0]!;
}
function dynamicSoname(dynamic: string): string {
  const matches = [...dynamic.matchAll(/\(SONAME\).*?\[([^\]]+)\]/g)].map(match => match[1]!);
  if (matches.length !== 1) throw new Error(`expected one ELF SONAME, found ${matches.length}`);
  return matches[0]!;
}
async function readDynamic(file: string): Promise<string> { return (await runCaptured("readelf", ["-dW", file])).stdout; }
function parseLdd(output: string): Map<string, string> {
  const resolved = new Map<string, string>();
  for (const line of output.split(/\r?\n/)) {
    const match = /^\s*(\S+)\s+=>\s+(\/\S+)\s+\(/.exec(line);
    if (match) {
      resolved.set(match[1]!, match[2]!);
      continue;
    }
    const direct = /^\s*(\/\S+)\s+\(/.exec(line);
    if (direct) resolved.set(basename(direct[1]!), direct[1]!);
  }
  return resolved;
}

function safeLeaf(value: string, label: string): string {
  if (value.includes("/") || value === "." || value === "..") throw new Error(`${label} must be a single path component`);
  return value;
}

async function readPackagePolicy(): Promise<PackagePolicy> {
  const value = record(JSON.parse(await readFile(policyPath, "utf8")), "Linux package policy");
  if (value.version !== 1) throw new Error("unsupported Linux package policy version");
  const privateValue = record(value.privateLibraries, "privateLibraries");
  const privateLibraries: Record<string, PrivateLibraryPolicy> = {};
  for (const [name, item] of Object.entries(privateValue)) {
    if (!/^lib[^/]+\.so(?:\.\d+)*$/.test(name)) throw new Error(`invalid private library name: ${name}`);
    const fields = record(item, `privateLibraries.${name}`);
    privateLibraries[name] = { licenseSource: stringField(fields, "licenseSource", `privateLibraries.${name}`), licenseFile: stringField(fields, "licenseFile", `privateLibraries.${name}`) };
  }
  if (!Array.isArray(value.hostLibraries) || value.hostLibraries.some(item => typeof item !== "string" || item.length === 0)) throw new Error("hostLibraries must contain library names");
  if (!Array.isArray(value.notices)) throw new Error("notices must be an array");
  const notices = value.notices.map((item, index) => {
    const fields = record(item, `notices[${index}]`);
    return { source: stringField(fields, "source", `notices[${index}]`), target: stringField(fields, "target", `notices[${index}]`) };
  });
  const envelopeValue = record(value.appImageEnvelope, "appImageEnvelope");
  const noticeDirectory = safeLeaf(stringField(envelopeValue, "noticeDirectory", "appImageEnvelope"), "appImageEnvelope.noticeDirectory");
  const symlinkValue = record(envelopeValue.symlinks, "appImageEnvelope.symlinks");
  const symlinks: Record<string, string> = {};
  for (const [name, targetValue] of Object.entries(symlinkValue)) {
    const target = typeof targetValue === "string" ? targetValue : "";
    safeLeaf(name, `appImageEnvelope symlink ${name}`);
    if (!target || target.startsWith("/") || target.split("/").includes("..")) throw new Error(`unsafe AppImage symlink target ${target}`);
    symlinks[name] = target;
  }
  const fileValue = record(envelopeValue.files, "appImageEnvelope.files");
  const files: Record<string, string> = {};
  for (const [name, sourceValue] of Object.entries(fileValue)) {
    const source = typeof sourceValue === "string" ? sourceValue : "";
    safeLeaf(name, `appImageEnvelope file ${name}`);
    if (!source.startsWith("usr/") || source.split("/").includes("..")) throw new Error(`unsafe AppImage file source ${source}`);
    files[name] = source;
  }
  const rootNames = [noticeDirectory, "usr", ...Object.keys(symlinks), ...Object.keys(files)];
  if (new Set(rootNames).size !== rootNames.length) throw new Error("AppImage envelope root paths must be unique");
  if (!Array.isArray(envelopeValue.notices)) throw new Error("appImageEnvelope.notices must be an array");
  const envelopeNotices = envelopeValue.notices.map((item, index) => {
    const fields = record(item, `appImageEnvelope.notices[${index}]`);
    const source = stringField(fields, "source", `appImageEnvelope.notices[${index}]`);
    const target = safeLeaf(stringField(fields, "target", `appImageEnvelope.notices[${index}]`), `appImageEnvelope.notices[${index}].target`);
    const sourceUrl = stringField(fields, "sourceUrl", `appImageEnvelope.notices[${index}]`);
    const digest = stringField(fields, "sha256", `appImageEnvelope.notices[${index}]`);
    if (!source.startsWith("third-party/appimage-runtime/") || source.split("/").includes("..")) throw new Error(`unsafe AppImage notice source ${source}`);
    if (new URL(sourceUrl).protocol !== "https:") throw new Error(`AppImage notice source must use HTTPS: ${sourceUrl}`);
    if (!/^[a-f0-9]{64}$/.test(digest)) throw new Error(`AppImage notice ${target} must have a lowercase SHA-256 digest`);
    return { source, target, sourceUrl, sha256: digest };
  });
  if (new Set(envelopeNotices.map(notice => notice.target)).size !== envelopeNotices.length) throw new Error("AppImage notice targets must be unique");
  validateDependencyPolicy(Object.keys(privateLibraries), value.hostLibraries as string[], []);
  return { version: 1, privateLibraries, hostLibraries: value.hostLibraries as string[], notices, appImageEnvelope: { noticeDirectory, symlinks, files, notices: envelopeNotices } };
}

async function readToolManifest(): Promise<ToolManifest> { return validateToolManifest(JSON.parse(await readFile(toolManifestPath, "utf8"))); }

async function packageForFile(file: string): Promise<{ binaryPackage: string; sourcePackage: string; packageVersion: string }> {
  const canonical = await realpath(file);
  const ownership = await runCaptured("dpkg-query", ["-S", canonical]);
  const separator = ownership.stdout.indexOf(": ");
  if (separator < 1) throw new Error(`dpkg-query did not identify ${canonical}`);
  const owner = ownership.stdout.slice(0, separator).trim();
  const details = await runCaptured("dpkg-query", ["-W", "-f=${binary:Package}\t${source:Package}\t${Version}", owner]);
  const [binaryPackage, sourcePackage, packageVersion] = details.stdout.trim().split("\t");
  if (!binaryPackage || !sourcePackage || !packageVersion) throw new Error(`incomplete Debian package provenance for ${canonical}`);
  return { binaryPackage, sourcePackage, packageVersion };
}

async function resolvePrivateLibraries(executable: string, policy: PackagePolicy): Promise<Map<string, string>> {
  const privateNames = Object.keys(policy.privateLibraries);
  const queue = [executable];
  const found = new Map<string, string>();
  const visited = new Set<string>();
  while (queue.length > 0) {
    const file = queue.shift()!;
    const needed = neededLibraries(await readDynamic(file));
    validateDependencyPolicy(privateNames, policy.hostLibraries, needed);
    const paths = parseLdd((await runCaptured("ldd", [file])).stdout);
    for (const name of needed) {
      if (!(name in policy.privateLibraries) || found.has(name)) continue;
      const resolved = paths.get(name);
      if (!resolved) throw new Error(`cannot resolve private dependency ${name} from ${file}`);
      found.set(name, resolved);
      if (!visited.has(resolved)) { visited.add(resolved); queue.push(resolved); }
    }
  }
  return found;
}

async function copyNotices(bundle: string, policy: PackagePolicy): Promise<void> {
  const destination = join(bundle, "share/licenses/huterm");
  await mkdir(destination, { recursive: true });
  for (const notice of policy.notices) {
    if (notice.target.includes("/") || notice.target === "." || notice.target === "..") throw new Error(`unsafe notice target ${notice.target}`);
    await copyFile(join(repoRoot, notice.source), join(destination, notice.target));
  }
}

async function copyMetadata(bundle: string): Promise<void> {
  const applications = join(bundle, "share/applications");
  const icons = join(bundle, "share/icons/hicolor/512x512/apps");
  const metainfo = join(bundle, "share/metainfo");
  await Promise.all([mkdir(applications, { recursive: true }), mkdir(icons, { recursive: true }), mkdir(metainfo, { recursive: true })]);
  await copyFile(join(linuxAssets, "app.huterm.dev.desktop"), join(applications, "app.huterm.dev.desktop"));
  await copyFile(join(repoRoot, "assets/Huterm-512.png"), join(icons, "app.huterm.dev.png"));
  await copyFile(join(linuxAssets, "app.huterm.dev.metainfo.xml"), join(metainfo, "app.huterm.dev.metainfo.xml"));
  await copyFile(join(linuxAssets, "README.md"), join(bundle, "README.md"));
}

export async function normalizeTreeMetadata(root: string, sourceDateEpoch: number): Promise<void> {
  const timestamp = new Date(sourceDateEpoch * 1_000);
  const directories: string[] = [];
  async function visit(directory: string): Promise<void> {
    directories.push(directory);
    await chmod(directory, 0o755);
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const file = join(directory, entry.name);
      if (entry.isDirectory()) await visit(file);
      else if (entry.isFile()) {
        await chmod(file, relative(root, file) === "bin/huterm" ? 0o755 : 0o644);
        await utimes(file, timestamp, timestamp);
      }
    }
  }
  await visit(root);
  for (const directory of directories.reverse()) await utimes(directory, timestamp, timestamp);
}

async function stageBundle(bundle: string, executable: string, architecture: LinuxArchitecture, sourceDateEpoch: number, releaseCommit: string, tools: ToolManifest, policy: PackagePolicy): Promise<void> {
  const binary = join(bundle, "bin/huterm");
  const libraryDirectory = join(bundle, "lib/huterm");
  await Promise.all([mkdir(dirname(binary), { recursive: true }), mkdir(libraryDirectory, { recursive: true })]);
  await copyFile(executable, binary);
  await chmod(binary, 0o755);
  assertElfArchitecture(await readFile(binary), architecture);
  await runInherited("patchelf", ["--set-rpath", "$ORIGIN/../lib/huterm", binary]);
  const resolved = await resolvePrivateLibraries(binary, policy);
  const privateLibraries: PrivateLibraryRecord[] = [];
  for (const [name, source] of [...resolved].sort(([left], [right]) => left.localeCompare(right))) {
    const destination = join(libraryDirectory, name);
    await copyFile(source, destination);
    await chmod(destination, 0o644);
    await runInherited("patchelf", ["--set-rpath", "$ORIGIN", destination]);
    const soname = dynamicSoname(await readDynamic(destination));
    if (soname !== name) throw new Error(`${source} has SONAME ${soname}, expected ${name}`);
    const provenance = await packageForFile(source);
    const libraryPolicy = policy.privateLibraries[name]!;
    const licenseTarget = join(bundle, "share/licenses/huterm", libraryPolicy.licenseFile);
    await mkdir(dirname(licenseTarget), { recursive: true });
    await copyFile(libraryPolicy.licenseSource, licenseTarget);
    privateLibraries.push({ file: relative(bundle, destination), soname, sha256: await sha256(destination), ...provenance, licenseFile: relative(bundle, licenseTarget) });
  }
  await copyMetadata(bundle);
  await copyNotices(bundle, policy);
  const packageManifest: PackageManifest = { version: 1, architecture, releaseCommit, sourceDateEpoch, tools: { appimagetool: tools.tools.appimagetool.version, runtime: tools.tools.runtime.version }, privateLibraries };
  const manifestPath = join(bundle, "share/huterm/package-manifest.json");
  await mkdir(dirname(manifestPath), { recursive: true });
  await writeFile(manifestPath, `${JSON.stringify(packageManifest, null, 2)}\n`);
  await normalizeTreeMetadata(bundle, sourceDateEpoch);
}

async function validateMetadata(bundle: string): Promise<void> {
  await runInherited("desktop-file-validate", [join(bundle, "share/applications/app.huterm.dev.desktop")]);
  await runInherited("appstreamcli", ["validate", "--no-net", join(bundle, "share/metainfo/app.huterm.dev.metainfo.xml")]);
}

async function validateResolvedLibraries(bundle: string, binary: string, policy: PackagePolicy): Promise<void> {
  const privateNames = Object.keys(policy.privateLibraries);
  const files = [binary, ...(await readdir(join(bundle, "lib/huterm"))).map(name => join(bundle, "lib/huterm", name))];
  for (const file of files) {
    const output = (await runCaptured("ldd", [file])).stdout;
    if (output.includes("not found")) throw new Error(`unresolved dependency for ${relative(bundle, file)}: ${output}`);
    validateResolvedLibraryEntries(bundle, privateNames, policy.hostLibraries, parseLdd(output));
  }
}

async function walkFiles(root: string): Promise<string[]> {
  const files: string[] = [];
  async function visit(directory: string): Promise<void> {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const file = join(directory, entry.name);
      if (entry.isDirectory()) await visit(file); else files.push(file);
    }
  }
  await visit(root);
  return files.sort();
}

type TreeEntry = { file: string; kind: "file" | "symlink" };

async function walkTreeEntries(root: string): Promise<TreeEntry[]> {
  const entries: TreeEntry[] = [];
  async function visit(directory: string): Promise<void> {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const file = join(directory, entry.name);
      if (entry.isDirectory()) await visit(file);
      else if (entry.isFile()) entries.push({ file, kind: "file" });
      else if (entry.isSymbolicLink()) entries.push({ file, kind: "symlink" });
      else throw new Error(`${file} is not a regular file or symlink`);
    }
  }
  await visit(root);
  return entries.sort((left, right) => left.file.localeCompare(right.file));
}

async function verifyBundle(bundle: string, expectedVersion: string, expectedArchitecture: LinuxArchitecture): Promise<PackageManifest> {
  const policy = await readPackagePolicy();
  const tools = await readToolManifest();
  const binary = join(bundle, "bin/huterm");
  const binaryHandle = await open(binary, constants.O_RDONLY | constants.O_NOFOLLOW);
  let binaryBytes: Buffer;
  try {
    const executable = await binaryHandle.stat();
    if (!executable.isFile() || (executable.mode & 0o111) === 0) throw new Error("bin/huterm is missing or not executable");
    binaryBytes = await binaryHandle.readFile();
  } finally { await binaryHandle.close(); }
  assertElfArchitecture(binaryBytes, expectedArchitecture);
  const dynamic = await readDynamic(binary);
  validateRunpath(dynamicRunpath(dynamic), "bin/huterm");
  const needed = neededLibraries(dynamic);
  validateDependencyPolicy(Object.keys(policy.privateLibraries), policy.hostLibraries, needed);
  if (needed.includes("libfreetype.so.6")) throw new Error("libfreetype.so.6 must not be a dynamic dependency");
  const glibc = highestRequiredGlibc((await runCaptured("objdump", ["-T", binary])).stdout);
  validateGlibcVersionInfo((await runCaptured("readelf", ["-VW", binary])).stdout, glibc.weak);
  if (glibc.weak.length > 0) console.log(`weak GLIBC imports: ${glibc.weak.join(", ")}`);
  const manifest = validatePackageManifest(JSON.parse(await readFile(join(bundle, "share/huterm/package-manifest.json"), "utf8")));
  if (manifest.architecture !== expectedArchitecture) throw new Error(`package manifest architecture ${manifest.architecture}, expected ${expectedArchitecture}`);
  if (manifest.tools.appimagetool !== tools.tools.appimagetool.version || manifest.tools.runtime !== tools.tools.runtime.version) throw new Error("package manifest tool versions do not match the pinned tool manifest");
  const normalizedTimestamp = manifest.sourceDateEpoch * 1_000;
  for (const file of await walkFiles(bundle)) {
    const handle = await open(file, constants.O_RDONLY | constants.O_NOFOLLOW);
    let metadata;
    let bytes;
    try {
      metadata = await handle.stat();
      if (!metadata.isFile()) throw new Error(`neutral payload contains a non-regular file: ${relative(bundle, file)}`);
      bytes = await handle.readFile();
    } finally { await handle.close(); }
    const expectedMode = relative(bundle, file) === "bin/huterm" ? 0o755 : 0o644;
    if ((metadata.mode & 0o777) !== expectedMode) throw new Error(`${relative(bundle, file)} mode is not ${expectedMode.toString(8)}`);
    if (metadata.mtimeMs !== normalizedTimestamp) throw new Error(`${relative(bundle, file)} timestamp is not SOURCE_DATE_EPOCH`);
    if (bytes.includes(Buffer.from(repoRoot))) throw new Error(`${relative(bundle, file)} contains the build path`);
  }
  if (!/^\d+\.\d+\.\d+$/.test(expectedVersion)) throw new Error(`invalid expected version ${expectedVersion}`);
  const recordedFiles = new Set(manifest.privateLibraries.map(library => library.file));
  const actualFiles = new Set((await readdir(join(bundle, "lib/huterm"))).map(name => `lib/huterm/${name}`));
  if ([...recordedFiles].sort().join("\n") !== [...actualFiles].sort().join("\n")) throw new Error("private library files do not match package manifest");
  for (const library of manifest.privateLibraries) {
    const file = join(bundle, library.file);
    if (await sha256(file) !== library.sha256) throw new Error(`${library.file} digest does not match package manifest`);
    const libraryDynamic = await readDynamic(file);
    if (dynamicSoname(libraryDynamic) !== library.soname) throw new Error(`${library.file} SONAME does not match package manifest`);
    validateRunpath(dynamicRunpath(libraryDynamic), library.file, true);
    await stat(join(bundle, library.licenseFile));
    validateDependencyPolicy(Object.keys(policy.privateLibraries), policy.hostLibraries, neededLibraries(libraryDynamic));
    const libraryGlibc = highestRequiredGlibc((await runCaptured("objdump", ["-T", file])).stdout);
    validateGlibcVersionInfo((await runCaptured("readelf", ["-VW", file])).stdout, libraryGlibc.weak);
  }
  for (const notice of policy.notices) {
    if (!(await readRegularFile(join(bundle, "share/licenses/huterm", notice.target))).equals(await readRegularFile(join(repoRoot, notice.source)))) throw new Error(`packaged notice differs: ${notice.target}`);
  }
  for (const banned of [...Object.keys(policy.appImageEnvelope.symlinks), ...Object.keys(policy.appImageEnvelope.files), policy.appImageEnvelope.noticeDirectory]) if (await Bun.file(join(bundle, banned)).exists()) throw new Error(`neutral bundle contains AppImage-only file: ${banned}`);
  await validateMetadata(bundle);
  await validateResolvedLibraries(bundle, binary, policy);
  console.log(`verified neutral Linux bundle ${basename(bundle)} (${expectedVersion}, ${expectedArchitecture}, GLIBC_${glibc.required})`);
  return manifest;
}

async function validateTarEntries(tarball: string, expectedRoot: string, forbiddenRootEntries: string[]): Promise<void> {
  const entries = (await runCaptured("tar", ["-tzf", tarball])).stdout.split(/\r?\n/).filter(Boolean);
  if (entries.length === 0) throw new Error("tarball is empty");
  for (const entry of entries) {
    const parts = entry.replace(/\/$/, "").split("/");
    if (entry.startsWith("/") || parts.includes("..") || parts[0] !== expectedRoot) throw new Error(`unsafe tarball entry: ${entry}`);
    if (parts.length > 1 && forbiddenRootEntries.includes(parts[1]!)) throw new Error(`tarball contains AppImage-only entry: ${entry}`);
  }
}

async function createTarball(parent: string, bundleName: string, sourceDateEpoch: number, tarball: string): Promise<void> {
  const plainTar = join(dirname(tarball), `${basename(tarball)}.tmp.tar`);
  try {
    await runInherited("tar", ["--sort=name", `--mtime=@${sourceDateEpoch}`, "--owner=0", "--group=0", "--numeric-owner", "--format=gnu", "-C", parent, "-cf", plainTar, bundleName]);
    await runToFile("gzip", ["-n", "-9", "-c", plainTar], tarball);
  } finally { await rm(plainTar, { force: true }); }
}

async function cachedTool(name: "appimagetool" | "runtime", architecture: LinuxArchitecture, manifest: ToolManifest): Promise<string> {
  const definition = manifest.tools[name];
  const asset = definition[architecture];
  const cacheRoot = process.env.HUTERM_APPIMAGE_CACHE_DIR ?? join(process.env.XDG_CACHE_HOME ?? join(homedir(), ".cache"), "huterm/appimage-tools");
  const destination = join(cacheRoot, `${name}-${definition.version}-${architecture}-${asset.sha256}`);
  await mkdir(cacheRoot, { recursive: true });
  try {
    verifyToolBytes(await readRegularFile(destination), asset.sha256, `${name} ${architecture}`);
    return destination;
  } catch { await rm(destination, { force: true }); }
  const quarantine = await mkdtemp(join(cacheRoot, ".download-"));
  const downloaded = join(quarantine, "download");
  try {
    await runInherited("curl", ["--fail", "--location", "--proto", "=https", "--proto-redir", "=https", "--output", downloaded, asset.url]);
    const handle = await open(downloaded, constants.O_RDONLY | constants.O_NOFOLLOW);
    try {
      const metadata = await handle.stat();
      if (!metadata.isFile()) throw new Error(`${name} ${architecture} download is not a regular file`);
      verifyToolBytes(await handle.readFile(), asset.sha256, `${name} ${architecture}`);
    } finally { await handle.close(); }
    await chmod(downloaded, 0o644);
    await rename(downloaded, destination);
    return destination;
  } finally { await rm(quarantine, { recursive: true, force: true }); }
}

async function stageAppImageEnvelope(appDir: string, sourceDateEpoch: number, policy: AppImageEnvelopePolicy): Promise<void> {
  const noticeDirectory = join(appDir, policy.noticeDirectory);
  await mkdir(noticeDirectory);
  for (const notice of policy.notices) {
    const source = join(repoRoot, notice.source);
    const bytes = await readRegularFile(source);
    verifyToolBytes(bytes, notice.sha256, notice.target);
    await copyFile(source, join(noticeDirectory, notice.target));
  }
  await normalizeTreeMetadata(noticeDirectory, sourceDateEpoch);
  for (const [name, target] of Object.entries(policy.symlinks)) await symlink(target, join(appDir, name));
  for (const [name, source] of Object.entries(policy.files)) {
    const destination = join(appDir, name);
    await copyFile(join(appDir, source), destination);
    await chmod(destination, 0o644);
    const timestamp = new Date(sourceDateEpoch * 1_000);
    await utimes(destination, timestamp, timestamp);
  }
}

function expectedAppImageDesktop(source: Buffer, version: string): Buffer {
  const text = source.toString("utf8");
  if (/^X-AppImage-Version=/m.test(text)) throw new Error("neutral desktop entry must not contain X-AppImage-Version");
  return Buffer.from(`${text}${text.endsWith("\n") ? "" : "\n"}X-AppImage-Version=${version}\n`);
}

async function validateAppImageEnvelope(appDir: string, version: string, policy: AppImageEnvelopePolicy): Promise<void> {
  const rootEntries = (await readdir(appDir)).sort();
  const expectedEntries = [...Object.keys(policy.symlinks), ...Object.keys(policy.files), policy.noticeDirectory, "usr"].sort();
  if (rootEntries.join("\n") !== expectedEntries.join("\n")) throw new Error(`unexpected AppImage root entries: ${rootEntries.join(", ")}`);
  for (const [name, target] of Object.entries(policy.symlinks)) {
    if ((await readlink(join(appDir, name))) !== target) throw new Error(`${name} must be a relative symlink to ${target}`);
  }
  for (const [name, source] of Object.entries(policy.files)) {
    const [actual, neutral] = await Promise.all([readRegularFile(join(appDir, name)), readRegularFile(join(appDir, source))]);
    const expected = name.endsWith(".desktop") ? expectedAppImageDesktop(neutral, version) : neutral;
    if (!actual.equals(expected)) throw new Error(`${name} does not have the exact declared envelope contents`);
  }
  const noticeDirectory = join(appDir, policy.noticeDirectory);
  const actualNotices = (await readdir(noticeDirectory)).sort();
  const expectedNotices = policy.notices.map(notice => notice.target).sort();
  if (actualNotices.join("\n") !== expectedNotices.join("\n")) throw new Error(`unexpected AppImage notice entries: ${actualNotices.join(", ")}`);
  for (const notice of policy.notices) {
    const source = await readRegularFile(join(repoRoot, notice.source));
    verifyToolBytes(source, notice.sha256, notice.target);
    if (!(await readRegularFile(join(noticeDirectory, notice.target))).equals(source)) throw new Error(`AppImage notice differs: ${notice.target}`);
  }
}

async function createAppImage(bundle: string, version: string, architecture: LinuxArchitecture, sourceDateEpoch: number, output: string, manifest: ToolManifest, policy: PackagePolicy): Promise<void> {
  const appDirRoot = await mkdtemp(join(tmpdir(), "huterm-appdir-"));
  const appDir = join(appDirRoot, "Huterm.AppDir");
  try {
    await mkdir(appDir);
    await cp(join(bundle, "bin"), join(appDir, "usr/bin"), { recursive: true, preserveTimestamps: true });
    await cp(join(bundle, "lib"), join(appDir, "usr/lib"), { recursive: true, preserveTimestamps: true });
    await cp(join(bundle, "share"), join(appDir, "usr/share"), { recursive: true, preserveTimestamps: true });
    await copyFile(join(bundle, "README.md"), join(appDir, "usr/README.md"));
    await normalizeTreeMetadata(join(appDir, "usr"), sourceDateEpoch);
    await stageAppImageEnvelope(appDir, sourceDateEpoch, policy.appImageEnvelope);
    const appimagetool = await cachedTool("appimagetool", architecture, manifest);
    const runtime = await cachedTool("runtime", architecture, manifest);
    await withPrivateExecutableCopy(appimagetool, executable =>
      runInherited(executable, ["--runtime-file", runtime, appDir, output], { env: {
        APPIMAGE_EXTRACT_AND_RUN: "1", ARCH: architecture, SOURCE_DATE_EPOCH: String(sourceDateEpoch), VERSION: version,
      } }));
  } finally { await rm(appDirRoot, { recursive: true, force: true }); }
}

async function extractTarball(tarball: string, expectedRoot: string, forbiddenRootEntries: string[]): Promise<{ root: string; bundle: string }> {
  await validateTarEntries(tarball, expectedRoot, forbiddenRootEntries);
  const root = await mkdtemp(join(tmpdir(), "huterm-tarball-"));
  await runInherited("tar", ["-xzf", tarball, "-C", root]);
  return { root, bundle: join(root, expectedRoot) };
}

async function extractAppImage(appImage: string): Promise<{ root: string; appDir: string }> {
  const root = await mkdtemp(join(tmpdir(), "huterm-appimage-"));
  try {
    await withPrivateExecutableCopy(appImage, executable => runInherited(executable, ["--appimage-extract"], { cwd: root }));
    return { root, appDir: join(root, "squashfs-root") };
  } catch (error) {
    await rm(root, { recursive: true, force: true });
    throw error;
  }
}

export async function compareTrees(left: string, right: string): Promise<void> {
  const leftFiles = await walkTreeEntries(left);
  const rightFiles = await walkTreeEntries(right);
  const leftNames = leftFiles.map(entry => relative(left, entry.file));
  const rightNames = rightFiles.map(entry => relative(right, entry.file));
  if (leftNames.join("\n") !== rightNames.join("\n")) throw new Error("AppImage and tarball neutral payload file lists differ");
  for (let index = 0; index < leftFiles.length; index++) {
    const leftEntry = leftFiles[index]!;
    const rightEntry = rightFiles[index]!;
    if (leftEntry.kind !== rightEntry.kind) throw new Error(`${leftNames[index]} types differ between AppImage and tarball`);
    if (leftEntry.kind === "symlink") {
      const [leftTarget, rightTarget] = await Promise.all([readlink(leftEntry.file), readlink(rightEntry.file)]);
      if (leftTarget !== rightTarget) throw new Error(`${leftNames[index]} symlink targets differ between AppImage and tarball`);
    } else {
      const [leftBytes, rightBytes] = await Promise.all([readRegularFile(leftEntry.file), readRegularFile(rightEntry.file)]);
      if (!leftBytes.equals(rightBytes)) throw new Error(`${leftNames[index]} bytes differ between AppImage and tarball`);
    }
  }
}

async function runPackageSmoke(executable: string, evidenceDirectory?: string, env: Record<string, string> = {}): Promise<void> {
  await requireCommands(["xvfb-run", "xdotool", "setxkbmap"]);
  if (evidenceDirectory) await mkdir(evidenceDirectory, { recursive: true });
  await runInherited("xvfb-run", ["-a", "-s", "-screen 0 1280x800x24 -noreset", "bun", "scripts/check-linux-input.ts", executable], { env: { ...env, HUTERM_PACKAGE_MAPS_DIR: evidenceDirectory } });
  if (evidenceDirectory) {
    for (const engine of ["alacritty", "ghostty"]) {
      const maps = await readFile(join(evidenceDirectory, `${engine}.maps`), "utf8");
      for (const library of ["libxkbcommon.so.0", "libxkbcommon-x11.so.0", "libxcb-xkb.so.1"]) {
        const line = maps.split(/\r?\n/).find(item => item.includes(`/${library}`));
        if (!line?.includes("/lib/huterm/")) throw new Error(`${engine} loaded ${library} outside the private bundle`);
      }
      if (/\/lib(?:fontconfig|freetype)\.so/.test(maps)) throw new Error(`${engine} loaded a dynamic Fontconfig or FreeType library`);
      const vulkan = maps.split(/\r?\n/).filter(line => /\/libvulkan[^/]*\.so/.test(line));
      if (vulkan.length === 0 || vulkan.some(line => line.includes("/lib/huterm/"))) throw new Error(`${engine} did not load Vulkan from the host`);
    }
  }
}

async function verifyArtifacts(appImage: string, tarball: string, version: string, architecture: LinuxArchitecture, smoke = false): Promise<void> {
  const expectedRoot = `Huterm-${version}-Linux-${architecture}`;
  const policy = await readPackagePolicy();
  assertElfArchitecture(await readRegularFile(appImage), architecture);
  const forbiddenRootEntries = [...Object.keys(policy.appImageEnvelope.symlinks), ...Object.keys(policy.appImageEnvelope.files), policy.appImageEnvelope.noticeDirectory];
  const tar = await extractTarball(tarball, expectedRoot, forbiddenRootEntries);
  const image = await extractAppImage(appImage);
  try {
    await verifyBundle(tar.bundle, version, architecture);
    await verifyBundle(join(image.appDir, "usr"), version, architecture);
    await validateAppImageEnvelope(image.appDir, version, policy.appImageEnvelope);
    await compareTrees(tar.bundle, join(image.appDir, "usr"));
    if (smoke) {
      const evidence = process.env.HUTERM_PACKAGE_EVIDENCE_DIR;
      await runPackageSmoke(join(tar.bundle, "bin/huterm"), evidence ? join(evidence, "tarball") : undefined);
      await withPrivateExecutableCopy(appImage, executable =>
        runPackageSmoke(executable, evidence ? join(evidence, "appimage") : undefined, { APPIMAGE_EXTRACT_AND_RUN: "1" }));
    }
  } finally { await Promise.all([rm(tar.root, { recursive: true, force: true }), rm(image.root, { recursive: true, force: true })]); }
  console.log(`verified ${basename(appImage)} and ${basename(tarball)} share the same neutral payload`);
}

async function currentVersion(): Promise<string> {
  const metadata = JSON.parse((await runCaptured("cargo", ["metadata", "--locked", "--no-deps", "--format-version", "1"])).stdout) as { packages?: { name?: string; version?: string }[] };
  const version = metadata.packages?.find(item => item.name === "huterm")?.version;
  if (!version || !/^\d+\.\d+\.\d+$/.test(version)) throw new Error("cannot derive Huterm version from Cargo metadata");
  return version;
}

async function currentArchitecture(): Promise<LinuxArchitecture> { return normalizeLinuxArchitecture((await runCaptured("uname", ["-m"])).stdout.trim()); }

async function sourceIdentity(): Promise<{ releaseCommit: string; sourceDateEpoch: number }> {
  const releaseCommit = process.env.RELEASE_SHA ?? process.env.HUTERM_SOURCE_REVISION ?? (await runCaptured("git", ["rev-parse", "HEAD"], { allowFailure: true })).stdout.trim();
  if (!/^[a-f0-9]{40}$/.test(releaseCommit)) throw new Error("cannot derive a full release commit; set RELEASE_SHA or HUTERM_SOURCE_REVISION");
  const rawEpoch = process.env.SOURCE_DATE_EPOCH ?? process.env.HUTERM_SOURCE_DATE_EPOCH ?? (await runCaptured("git", ["show", "-s", "--format=%ct", "HEAD"], { allowFailure: true })).stdout.trim();
  const sourceDateEpoch = Number(rawEpoch);
  if (!Number.isSafeInteger(sourceDateEpoch) || sourceDateEpoch <= 0) throw new Error("SOURCE_DATE_EPOCH is required when the source checkout has no Git metadata");
  return { releaseCommit, sourceDateEpoch };
}

async function build(version: string, architecture: LinuxArchitecture): Promise<void> {
  if (process.platform !== "linux") throw new Error("Linux packaging requires a Linux host");
  await requireCommands(["appstreamcli", "curl", "desktop-file-validate", "dpkg-query", "gzip", "ldd", "objdump", "patchelf", "readelf", "tar"]);
  const tools = await readToolManifest();
  const policy = await readPackagePolicy();
  const identity = await sourceIdentity();
  await runInherited("bash", ["scripts/build-exec.sh", "cargo", "build", "--release", "--locked", "-p", "huterm"], {
    env: { SOURCE_DATE_EPOCH: String(identity.sourceDateEpoch) },
  });
  const executable = join(repoRoot, "target/release/huterm");
  assertElfArchitecture(await readFile(executable), architecture);
  const outputDirectory = join(repoRoot, "dist/linux", architecture);
  const staging = join(outputDirectory, `.staging-${process.pid}-${crypto.randomUUID()}`);
  const bundleName = `Huterm-${version}-Linux-${architecture}`;
  const appImageName = `${bundleName}.AppImage`;
  const tarballName = `${bundleName}.tar.gz`;
  const stagedAppImage = join(staging, appImageName);
  const stagedTarball = join(staging, tarballName);
  const finalAppImage = join(outputDirectory, appImageName);
  const finalTarball = join(outputDirectory, tarballName);
  await mkdir(staging, { recursive: true });
  await Promise.all([rm(finalAppImage, { force: true }), rm(finalTarball, { force: true })]);
  try {
    const bundle = join(staging, bundleName);
    await stageBundle(bundle, executable, architecture, identity.sourceDateEpoch, identity.releaseCommit, tools, policy);
    await verifyBundle(bundle, version, architecture);
    await createTarball(staging, bundleName, identity.sourceDateEpoch, stagedTarball);
    await createAppImage(bundle, version, architecture, identity.sourceDateEpoch, stagedAppImage, tools, policy);
    const evidence = join(outputDirectory, "package-evidence");
    await rm(evidence, { recursive: true, force: true });
    await mkdir(evidence, { recursive: true });
    process.env.HUTERM_PACKAGE_EVIDENCE_DIR = evidence;
    await verifyArtifacts(stagedAppImage, stagedTarball, version, architecture, true);
    await Promise.all([rename(stagedAppImage, finalAppImage), rename(stagedTarball, finalTarball)]);
    console.log(`created ${relative(repoRoot, finalAppImage)} and ${relative(repoRoot, finalTarball)}`);
  } finally { await rm(staging, { recursive: true, force: true }); }
}

function cliArguments(args: string[]): { command: string; paths: string[]; version?: string; architecture?: LinuxArchitecture } {
  const [command, ...rest] = args;
  if (!command) throw new Error("expected build, verify-bundle, or verify-artifacts");
  const paths: string[] = [];
  let version: string | undefined;
  let architecture: LinuxArchitecture | undefined;
  for (let index = 0; index < rest.length; index++) {
    const argument = rest[index]!;
    if (argument === "--version") { version = rest[++index]; if (!version) throw new Error("--version requires a value"); }
    else if (argument === "--arch") { const value = rest[++index]; if (!value) throw new Error("--arch requires a value"); architecture = normalizeLinuxArchitecture(value); }
    else if (argument.startsWith("-")) throw new Error(`unknown option ${argument}`); else paths.push(argument);
  }
  return { command, paths, version, architecture };
}

async function main(): Promise<void> {
  const options = cliArguments(Bun.argv.slice(2));
  const version = options.version ?? await currentVersion();
  const architecture = options.architecture ?? await currentArchitecture();
  switch (options.command) {
    case "build": if (options.paths.length !== 0) throw new Error("build takes no paths"); await build(version, architecture); break;
    case "verify-bundle": if (options.paths.length !== 1) throw new Error("verify-bundle requires one bundle path"); await verifyBundle(resolve(options.paths[0]!), version, architecture); break;
    case "verify-artifacts": if (options.paths.length !== 2) throw new Error("verify-artifacts requires an AppImage and tarball path"); await verifyArtifacts(resolve(options.paths[0]!), resolve(options.paths[1]!), version, architecture, process.env.HUTERM_PACKAGE_SMOKE === "1"); break;
    default: throw new Error("expected build, verify-bundle, or verify-artifacts");
  }
}

if (import.meta.main) {
  try { await main(); } catch (error) { console.error(`Linux package failed: ${error instanceof Error ? error.message : String(error)}`); process.exitCode = 1; }
}
