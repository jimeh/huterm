/** Stage, archive, and verify the standalone `huterm` command-line downloads. */
import { constants } from "node:fs";
import { chmod, copyFile, mkdir, mkdtemp, open, readFile, readdir, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "..");

export type CliPlatform = "macOS-universal" | "Linux-x86_64" | "Linux-aarch64";

/** Files beside the executable in every standalone archive, from the repository root. */
export const cliNotices = [
  { source: "LICENSE", target: "LICENSE" },
  { source: "third-party/licenses/licenses.json", target: "licenses.json" },
] as const;

export function cliArchiveRoot(version: string, platform: CliPlatform): string {
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error(`invalid CLI version ${version}`);
  return `Huterm-CLI-${version}-${platform}`;
}

export function cliArchiveName(version: string, platform: CliPlatform): string {
  return `${cliArchiveRoot(version, platform)}.${platform === "macOS-universal" ? "zip" : "tar.gz"}`;
}

/** The archive's exact entries, directories with a trailing slash. */
export function cliArchiveEntries(root: string): string[] {
  return [`${root}/`, `${root}/huterm`, ...cliNotices.map(notice => `${root}/${notice.target}`)].sort();
}

/** Copies the executable and notices into `parent/root` with fixed modes. */
export async function stageCliDirectory(parent: string, root: string, executable: string): Promise<string> {
  const directory = join(parent, root);
  await rm(directory, { recursive: true, force: true });
  await mkdir(directory, { recursive: true });
  await chmod(directory, 0o755);
  await copyFile(executable, join(directory, "huterm"));
  await chmod(join(directory, "huterm"), 0o755);
  for (const notice of cliNotices) {
    await copyFile(join(repoRoot, notice.source), join(directory, notice.target));
    await chmod(join(directory, notice.target), 0o644);
  }
  return directory;
}

interface CommandResult { stdout: string; stderr: string; exitCode: number }

async function capture(command: string[], env?: Record<string, string>): Promise<CommandResult> {
  const child = Bun.spawn(command, { cwd: repoRoot, env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, exitCode] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
  return { stdout, stderr, exitCode };
}

async function required(command: string[]): Promise<string> {
  const result = await capture(command);
  if (result.exitCode !== 0) throw new Error(`${command[0]} exited with status ${result.exitCode}: ${result.stderr || result.stdout}`);
  return result.stdout;
}

/** The environment a headless host gives the CLI: no display variables. */
function headlessEnvironment(): Record<string, string> {
  const environment: Record<string, string> = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (value !== undefined && key !== "DISPLAY" && key !== "WAYLAND_DISPLAY") environment[key] = value;
  }
  return environment;
}

export interface CliRunResults { version: CommandResult; help: CommandResult; usage: CommandResult }

/** Checks `--version`, `--help`, and a usage error against the documented conventions. */
export function validateCliRuns(results: CliRunResults, version: string, label: string): void {
  if (results.version.exitCode !== 0 || results.version.stdout !== `huterm ${version}\n` || results.version.stderr !== "") {
    throw new Error(`${label} --version printed ${JSON.stringify(results.version.stdout)} with status ${results.version.exitCode}; expected "huterm ${version}"`);
  }
  if (results.help.exitCode !== 0 || !results.help.stdout.includes("Usage: huterm") || results.help.stderr !== "") {
    throw new Error(`${label} --help did not print usage to standard output with status 0`);
  }
  if (results.usage.exitCode !== 2 || results.usage.stdout !== "" || !results.usage.stderr.includes("Usage: huterm")) {
    throw new Error(`${label} accepted an unknown argument or reported it without usage and status 2`);
  }
}

/** Runs the executable without a display and checks its conventions. */
export async function verifyCliRuns(executable: string, version: string, label = basename(executable)): Promise<void> {
  const environment = headlessEnvironment();
  validateCliRuns({
    version: await capture([executable, "--version"], environment),
    help: await capture([executable, "--help"], environment),
    usage: await capture([executable, "--huterm-package-check-unknown-argument"], environment),
  }, version, label);
}

/**
 * Reads a regular file, refusing a symlink, with its mode from the same open
 * handle so the checked file is the one read.
 */
async function readArchiveFile(file: string, mode: number, label: string): Promise<Buffer> {
  const handle = await open(file, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const metadata = await handle.stat();
    if (!metadata.isFile() || (metadata.mode & 0o777) !== mode) throw new Error(`CLI archive ${label} is not a mode ${mode.toString(8)} file`);
    return await handle.readFile();
  } finally { await handle.close(); }
}

/**
 * Checks an extracted archive directory's files, modes, and notices, and
 * returns the executable's bytes as checked.
 */
export async function verifyCliDirectory(directory: string): Promise<Buffer> {
  const expected = ["huterm", ...cliNotices.map(notice => notice.target)].sort();
  const actual = (await readdir(directory)).sort();
  if (actual.join("\n") !== expected.join("\n")) throw new Error(`CLI archive contains missing or unexpected files: ${actual.join(", ")}`);
  if (((await stat(directory)).mode & 0o777) !== 0o755) throw new Error("CLI archive directory mode is not 755");
  const executable = await readArchiveFile(join(directory, "huterm"), 0o755, "huterm");
  for (const notice of cliNotices) {
    const bytes = await readArchiveFile(join(directory, notice.target), 0o644, notice.target);
    if (!bytes.equals(await readFile(join(repoRoot, notice.source)))) throw new Error(`CLI archive ${notice.target} differs from ${notice.source}`);
  }
  return executable;
}

async function verifyMacosExecutable(executable: string): Promise<void> {
  for (const architecture of ["arm64", "x86_64"]) await required(["lipo", executable, "-verify_arch", architecture]);
  const linkage = (await required(["otool", "-L", executable])).split(/\r?\n/).filter(line => /^\s+/.test(line)).map(line => line.trim());
  validateMacosCliLinkage(linkage);
}

/** The command line links only the system library; GPUI would bring frameworks. */
export function validateMacosCliLinkage(dependencies: string[]): void {
  const unexpected = dependencies.filter(line => !line.startsWith("/usr/lib/libSystem.B.dylib "));
  if (dependencies.length === 0 || unexpected.length > 0) throw new Error(`the huterm command line links more than libSystem: ${unexpected.join("; ") || "no dependencies found"}`);
}

/** Zips the executable with its notices; ditto keeps modes and drops metadata. */
export async function createMacosCliArchive(executable: string, version: string, destination: string): Promise<void> {
  const root = cliArchiveRoot(version, "macOS-universal");
  if (basename(destination) !== `${root}.zip`) throw new Error(`macOS CLI archive must be named ${root}.zip`);
  const staging = await mkdtemp(join(tmpdir(), "huterm-cli-macos-"));
  try {
    const directory = await stageCliDirectory(staging, root, executable);
    await rm(destination, { force: true });
    await mkdir(resolve(destination, ".."), { recursive: true });
    await required(["ditto", "-c", "-k", "--norsrc", "--noextattr", "--noacl", "--keepParent", directory, destination]);
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
}

/**
 * Verifies a macOS CLI archive's entries, files, executable, and behavior.
 * When `bundled` names the app's `huterm`, the archive must carry the same bytes.
 */
export async function verifyMacosCliArchive(archive: string, version: string, bundled?: string): Promise<void> {
  const root = cliArchiveRoot(version, "macOS-universal");
  const entries = (await required(["zipinfo", "-1", archive])).split(/\r?\n/).filter(Boolean).sort();
  const expected = cliArchiveEntries(root);
  if (entries.join("\n") !== expected.join("\n")) throw new Error(`${basename(archive)} has unexpected entries: ${entries.join(", ")}`);
  const extracted = await mkdtemp(join(tmpdir(), "huterm-cli-verify-"));
  try {
    await required(["ditto", "-x", "-k", archive, extracted]);
    const directory = join(extracted, root);
    const bytes = await verifyCliDirectory(directory);
    const executable = join(directory, "huterm");
    if (bundled && !bytes.equals(await readFile(bundled))) throw new Error(`${basename(archive)} does not carry the app's huterm executable`);
    await verifyMacosExecutable(executable);
    await verifyCliRuns(executable, version, basename(archive));
  } finally {
    await rm(extracted, { recursive: true, force: true });
  }
  console.log(`verified ${basename(archive)}`);
}

async function main(): Promise<void> {
  const [command, executable, version, destination] = Bun.argv.slice(2);
  switch (command) {
    case "macos": {
      // Builds and verifies an archive from `executable`, as packaging does before signing.
      if (!executable || !version || !destination) throw new Error("macos requires an executable, a version, and a destination directory");
      const archive = join(resolve(destination), cliArchiveName(version, "macOS-universal"));
      await createMacosCliArchive(resolve(executable), version, archive);
      await verifyMacosCliArchive(archive, version, resolve(executable));
      break;
    }
    default: throw new Error("expected macos <executable> <version> <destination-directory>");
  }
}

if (import.meta.main) {
  try { await main(); } catch (error) { console.error(`CLI package failed: ${error instanceof Error ? error.message : String(error)}`); process.exitCode = 1; }
}
