/** Build and verify relocatable Linux tarballs and AppImages. */
import { createHash } from "node:crypto";
import { chmod, copyFile, cp, lstat, mkdir, mkdtemp, readFile, readdir, readlink, realpath, rename, rm, stat, symlink, utimes, writeFile } from "node:fs/promises";
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
interface PackagePolicy { version: number; privateLibraries: Record<string, PrivateLibraryPolicy>; hostLibraries: string[]; notices: NoticePolicy[] }
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

async function sha256(file: string): Promise<string> { return createHash("sha256").update(await readFile(file)).digest("hex"); }
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
    if (match) resolved.set(match[1]!, match[2]!);
  }
  return resolved;
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
  validateDependencyPolicy(Object.keys(privateLibraries), value.hostLibraries as string[], []);
  return { version: 1, privateLibraries, hostLibraries: value.hostLibraries as string[], notices };
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
  const bundleRoot = `${resolve(bundle)}${sep}`;
  for (const file of files) {
    const output = (await runCaptured("ldd", [file])).stdout;
    if (output.includes("not found")) throw new Error(`unresolved dependency for ${relative(bundle, file)}: ${output}`);
    for (const [name, filePath] of parseLdd(output)) {
      const inside = resolve(filePath).startsWith(bundleRoot);
      if (privateNames.includes(name) && !inside) throw new Error(`${name} resolved outside the bundle: ${filePath}`);
      if (policy.hostLibraries.includes(name) && inside) throw new Error(`host-owned ${name} resolved inside the bundle`);
    }
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

async function verifyBundle(bundle: string, expectedVersion: string, expectedArchitecture: LinuxArchitecture): Promise<PackageManifest> {
  const policy = await readPackagePolicy();
  const tools = await readToolManifest();
  const binary = join(bundle, "bin/huterm");
  const executable = await stat(binary);
  if (!executable.isFile() || (executable.mode & 0o111) === 0) throw new Error("bin/huterm is missing or not executable");
  assertElfArchitecture(await readFile(binary), expectedArchitecture);
  const dynamic = await readDynamic(binary);
  validateRunpath(dynamicRunpath(dynamic), "bin/huterm");
  const needed = neededLibraries(dynamic);
  validateDependencyPolicy(Object.keys(policy.privateLibraries), policy.hostLibraries, needed);
  if (needed.includes("libfreetype.so.6")) throw new Error("libfreetype.so.6 must not be a dynamic dependency");
  const glibc = highestRequiredGlibc((await runCaptured("objdump", ["-T", binary])).stdout);
  if (glibc.weak.length > 0) console.log(`weak GLIBC imports: ${glibc.weak.join(", ")}`);
  const manifest = validatePackageManifest(JSON.parse(await readFile(join(bundle, "share/huterm/package-manifest.json"), "utf8")));
  if (manifest.architecture !== expectedArchitecture) throw new Error(`package manifest architecture ${manifest.architecture}, expected ${expectedArchitecture}`);
  if (manifest.tools.appimagetool !== tools.tools.appimagetool.version || manifest.tools.runtime !== tools.tools.runtime.version) throw new Error("package manifest tool versions do not match the pinned tool manifest");
  const normalizedTimestamp = manifest.sourceDateEpoch * 1_000;
  for (const file of await walkFiles(bundle)) {
    const metadata = await lstat(file);
    if (metadata.isSymbolicLink()) throw new Error(`neutral payload contains an unexpected symlink: ${relative(bundle, file)}`);
    const expectedMode = relative(bundle, file) === "bin/huterm" ? 0o755 : 0o644;
    if ((metadata.mode & 0o777) !== expectedMode) throw new Error(`${relative(bundle, file)} mode is not ${expectedMode.toString(8)}`);
    if (metadata.mtimeMs !== normalizedTimestamp) throw new Error(`${relative(bundle, file)} timestamp is not SOURCE_DATE_EPOCH`);
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
  }
  for (const notice of policy.notices) {
    if (!(await readFile(join(bundle, "share/licenses/huterm", notice.target))).equals(await readFile(join(repoRoot, notice.source)))) throw new Error(`packaged notice differs: ${notice.target}`);
  }
  for (const banned of ["AppRun", ".DirIcon", "app.huterm.dev.desktop", "app.huterm.dev.png"]) if (await Bun.file(join(bundle, banned)).exists()) throw new Error(`neutral bundle contains AppImage-only file: ${banned}`);
  await validateMetadata(bundle);
  await validateResolvedLibraries(bundle, binary, policy);
  for (const file of await walkFiles(bundle)) if ((await readFile(file)).includes(Buffer.from(repoRoot))) throw new Error(`${relative(bundle, file)} contains the build path`);
  console.log(`verified neutral Linux bundle ${basename(bundle)} (${expectedVersion}, ${expectedArchitecture}, GLIBC_${glibc.required})`);
  return manifest;
}

async function validateTarEntries(tarball: string, expectedRoot: string): Promise<void> {
  const entries = (await runCaptured("tar", ["-tzf", tarball])).stdout.split(/\r?\n/).filter(Boolean);
  if (entries.length === 0) throw new Error("tarball is empty");
  for (const entry of entries) {
    const parts = entry.replace(/\/$/, "").split("/");
    if (entry.startsWith("/") || parts.includes("..") || parts[0] !== expectedRoot) throw new Error(`unsafe tarball entry: ${entry}`);
    if (["AppRun", ".DirIcon"].includes(parts.at(-1)!)) throw new Error(`tarball contains AppImage-only entry: ${entry}`);
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
  const destination = join(cacheRoot, `${name}-${definition.version}-${architecture}`);
  await mkdir(cacheRoot, { recursive: true });
  if (await Bun.file(destination).exists()) {
    try {
      verifyToolBytes(await readFile(destination), asset.sha256, `${name} ${architecture}`);
      if (name === "appimagetool") await chmod(destination, 0o755);
      return destination;
    } catch { await rm(destination, { force: true }); }
  }
  const response = await fetch(asset.url, { redirect: "follow" });
  if (!response.ok) throw new Error(`download ${asset.url} failed with HTTP ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  verifyToolBytes(bytes, asset.sha256, `${name} ${architecture}`);
  const temporary = `${destination}.${process.pid}.tmp`;
  await writeFile(temporary, bytes, { mode: name === "appimagetool" ? 0o755 : 0o644 });
  await rename(temporary, destination);
  return destination;
}

async function createAppImage(bundle: string, version: string, architecture: LinuxArchitecture, sourceDateEpoch: number, output: string, manifest: ToolManifest): Promise<void> {
  const appDirRoot = await mkdtemp(join(tmpdir(), "huterm-appdir-"));
  const appDir = join(appDirRoot, "Huterm.AppDir");
  try {
    await mkdir(appDir);
    await cp(join(bundle, "bin"), join(appDir, "usr/bin"), { recursive: true, preserveTimestamps: true });
    await cp(join(bundle, "lib"), join(appDir, "usr/lib"), { recursive: true, preserveTimestamps: true });
    await cp(join(bundle, "share"), join(appDir, "usr/share"), { recursive: true, preserveTimestamps: true });
    await copyFile(join(bundle, "README.md"), join(appDir, "usr/README.md"));
    await normalizeTreeMetadata(join(appDir, "usr"), sourceDateEpoch);
    await symlink("usr/bin/huterm", join(appDir, "AppRun"));
    await symlink("usr/share/applications/app.huterm.dev.desktop", join(appDir, "app.huterm.dev.desktop"));
    await symlink("usr/share/icons/hicolor/512x512/apps/app.huterm.dev.png", join(appDir, "app.huterm.dev.png"));
    await symlink("app.huterm.dev.png", join(appDir, ".DirIcon"));
    const appimagetool = await cachedTool("appimagetool", architecture, manifest);
    const runtime = await cachedTool("runtime", architecture, manifest);
    await runInherited(appimagetool, ["--runtime-file", runtime, appDir, output], { env: {
      APPIMAGE_EXTRACT_AND_RUN: "1", ARCH: architecture, SOURCE_DATE_EPOCH: String(sourceDateEpoch), VERSION: version,
    } });
  } finally { await rm(appDirRoot, { recursive: true, force: true }); }
}

async function extractTarball(tarball: string, expectedRoot: string): Promise<{ root: string; bundle: string }> {
  await validateTarEntries(tarball, expectedRoot);
  const root = await mkdtemp(join(tmpdir(), "huterm-tarball-"));
  await runInherited("tar", ["-xzf", tarball, "-C", root]);
  return { root, bundle: join(root, expectedRoot) };
}

async function extractAppImage(appImage: string): Promise<{ root: string; appDir: string }> {
  const root = await mkdtemp(join(tmpdir(), "huterm-appimage-"));
  await runInherited(appImage, ["--appimage-extract"], { cwd: root });
  return { root, appDir: join(root, "squashfs-root") };
}

async function compareTrees(left: string, right: string): Promise<void> {
  const leftFiles = await walkFiles(left);
  const rightFiles = await walkFiles(right);
  const leftNames = leftFiles.map(file => relative(left, file));
  const rightNames = rightFiles.map(file => relative(right, file));
  if (leftNames.join("\n") !== rightNames.join("\n")) throw new Error("AppImage and tarball neutral payload file lists differ");
  for (let index = 0; index < leftFiles.length; index++) {
    const leftPath = leftFiles[index]!;
    const rightPath = rightFiles[index]!;
    const [leftEntry, rightEntry] = await Promise.all([lstat(leftPath), lstat(rightPath)]);
    if (leftEntry.isSymbolicLink() !== rightEntry.isSymbolicLink()) throw new Error(`${leftNames[index]} file types differ`);
    if (leftEntry.isSymbolicLink()) {
      if (await readlink(leftPath) !== await readlink(rightPath)) throw new Error(`${leftNames[index]} symlink targets differ`);
    } else if (!(await readFile(leftPath)).equals(await readFile(rightPath))) throw new Error(`${leftNames[index]} bytes differ between AppImage and tarball`);
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
  assertElfArchitecture(await readFile(appImage), architecture);
  const tar = await extractTarball(tarball, expectedRoot);
  const image = await extractAppImage(appImage);
  try {
    await verifyBundle(tar.bundle, version, architecture);
    await verifyBundle(join(image.appDir, "usr"), version, architecture);
    const rootEntries = (await readdir(image.appDir)).sort();
    const expectedEntries = [".DirIcon", "AppRun", "app.huterm.dev.desktop", "app.huterm.dev.png", "usr"].sort();
    if (rootEntries.join("\n") !== expectedEntries.join("\n")) throw new Error(`unexpected AppImage root entries: ${rootEntries.join(", ")}`);
    if ((await readlink(join(image.appDir, "AppRun"))) !== "usr/bin/huterm") throw new Error("AppRun must be a relative symlink to usr/bin/huterm");
    await compareTrees(tar.bundle, join(image.appDir, "usr"));
    if (smoke) {
      const evidence = process.env.HUTERM_PACKAGE_EVIDENCE_DIR;
      await runPackageSmoke(join(tar.bundle, "bin/huterm"), evidence ? join(evidence, "tarball") : undefined);
      await runPackageSmoke(appImage, evidence ? join(evidence, "appimage") : undefined, { APPIMAGE_EXTRACT_AND_RUN: "1" });
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
  await requireCommands(["appstreamcli", "desktop-file-validate", "dpkg-query", "gzip", "ldd", "objdump", "patchelf", "readelf", "tar"]);
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
    await createAppImage(bundle, version, architecture, identity.sourceDateEpoch, stagedAppImage, tools);
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
