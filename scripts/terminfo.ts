/** Compile and inspect Huterm's private terminfo entry. */
import { copyFile, mkdir, readFile } from "node:fs/promises";
import { join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "..");
export const source = join(repoRoot, "assets/terminfo/xterm-huterm.terminfo");
export const defaultOutput = join(repoRoot, "target/terminfo");
const license = join(repoRoot, "third-party/terminfo/LICENSE-ncurses");
const terminalName = "xterm-huterm";

function environment(directory: string): Record<string, string> {
  return { ...process.env, TERMINFO: directory } as Record<string, string>;
}

async function run(command: string, args: string[], env?: Record<string, string>): Promise<string> {
  const child = Bun.spawn([command, ...args], {
    cwd: repoRoot,
    env: env ?? (process.env as Record<string, string>),
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, status] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (status !== 0) throw new Error(`${command} failed (${status}): ${stderr.trim()}`);
  return stdout;
}

export async function compiledEntry(directory: string): Promise<string> {
  for (const bucket of [terminalName[0]!, terminalName.charCodeAt(0).toString(16)]) {
    const candidate = join(directory, bucket, terminalName);
    if (await Bun.file(candidate).exists()) return candidate;
  }
  throw new Error(`compiled ${terminalName} entry is missing from ${directory}`);
}

export async function verifyCompiled(directory: string): Promise<void> {
  const [packagedSource, sourceBytes, packagedLicense, licenseBytes] = await Promise.all([
    readFile(join(directory, "xterm-huterm.terminfo")),
    readFile(source),
    readFile(join(directory, "LICENSE-ncurses")),
    readFile(license),
  ]);
  if (!packagedSource.equals(sourceBytes) || !packagedLicense.equals(licenseBytes)) {
    throw new Error("packaged terminfo source or ncurses notice differs from its reviewed input");
  }
  const bytes = await readFile(await compiledEntry(directory));
  if (bytes.length < 2 || bytes[0] !== 0x1a || bytes[1] !== 0x01) {
    throw new Error("xterm-huterm must use the portable 16-bit terminfo format");
  }
  const description = await run("infocmp", ["-x", terminalName], environment(directory));
  // Numeric values are normalized differently by ncurses releases.
  if (!/(?:^|[,\s])colors#(?:256|0x100)(?:,|\s)/m.test(description)
    || !/(?:^|[,\s])pairs#(?:32767|0x7fff)(?:,|\s)/m.test(description)
    || !/(?:^|[,\s])RGB(?:,|\s)/m.test(description)
    || !/(?:^|[,\s])Tc(?:,|\s)/m.test(description)) {
    throw new Error("compiled xterm-huterm is missing its 256-color or direct-color capabilities");
  }
  const colors = (await run("tput", ["-T", terminalName, "colors"], environment(directory))).trim();
  if (colors !== "256") throw new Error(`tput reported ${JSON.stringify(colors)} colors instead of 256`);
}

export async function prepare(directory = defaultOutput): Promise<void> {
  await mkdir(directory, { recursive: true });
  await Promise.all([
    copyFile(source, join(directory, "xterm-huterm.terminfo")),
    copyFile(license, join(directory, "LICENSE-ncurses")),
  ]);
  await run("tic", ["-x", "-o", directory, source]);
  await verifyCompiled(directory);
}

async function main(args: string[]): Promise<void> {
  const [command = "prepare", output = defaultOutput] = args;
  if (command === "prepare") await prepare(resolve(output));
  else if (command === "check") await verifyCompiled(resolve(output));
  else throw new Error("usage: bun scripts/terminfo.ts [prepare|check] [output-directory]");
  console.log(`${command === "prepare" ? "prepared" : "verified"} ${await compiledEntry(resolve(output))}`);
}

if (import.meta.main) await main(process.argv.slice(2));
