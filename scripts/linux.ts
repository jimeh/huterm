/** Run Linux development workflows in a disposable Docker container. */
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, renameSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve } from "node:path";

const repository = resolve(import.meta.dir, "..");
type Mode = "test" | "smoke" | "package" | "exec" | "clean";
type Architecture = "arm64" | "amd64";

export function architecture(value: string): Architecture {
  switch (value.toLowerCase()) {
    case "arm64": case "aarch64": return "arm64";
    case "amd64": case "x86_64": return "amd64";
    default: throw new Error(`unsupported architecture ${JSON.stringify(value)}; use native, amd64, or arm64`);
  }
}

export function argumentsFor(args: string[]): { mode: Mode; arch: string; command: string[] } {
  const [mode, ...rest] = args;
  if (mode !== "test" && mode !== "smoke" && mode !== "package" && mode !== "exec" && mode !== "clean") {
    throw new Error("expected test, smoke, package, exec, or clean");
  }
  let arch = "native";
  let index = 0;
  while (index < rest.length) {
    const arg = rest[index]!;
    if (arg === "--") { index++; break; }
    if (arg === "--arch" || arg.startsWith("--arch=")) {
      const value = arg === "--arch" ? rest[++index] : arg.slice(7);
      if (!value) throw new Error("--arch requires native, amd64, or arm64");
      arch = value.toLowerCase() === "native" ? "native" : architecture(value);
      index++;
    } else if (arg.startsWith("-")) {
      throw new Error(`unknown option ${arg}`);
    } else break;
  }
  const command = rest.slice(index);
  if (mode === "exec" && command.length === 0) throw new Error("linux:exec requires a command");
  if (mode !== "exec" && command.length > 0) throw new Error(`linux:${mode} takes only --arch; use linux:exec for a custom command`);
  const selected = mode === "test" ? ["mise", "run", "test:rust"]
    : mode === "smoke" ? ["mise", "run", "ci:smoke"]
    : mode === "package" ? ["mise", "run", "package:linux"]
    : command;
  return { mode, arch, command: selected };
}

export function cacheNames(root: string, arch: Architecture) {
  const worktree = createHash("sha256").update(root).digest("hex").slice(0, 16);
  const name = `huterm-linux-${worktree}-${arch}`;
  return { worktree, name, volumes: [`${name}-workspace`, `${name}-cache`] };
}

function capture(args: string[], required = true): string | undefined {
  const result = Bun.spawnSync(["docker", ...args], { stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0) {
    if (!required) return undefined;
    throw new Error(`docker ${args[0]} failed: ${result.stderr.toString().trim()}`);
  }
  return result.stdout.toString().trim();
}

function exportPackageArtifacts(root: string, container: string, arch: Architecture): void {
  const packageArch = arch === "amd64" ? "x86_64" : "aarch64";
  const distRoot = resolve(process.env.HUTERM_LINUX_DIST_DIR ?? join(root, "dist"));
  const linuxDist = join(distRoot, "linux");
  const destination = join(linuxDist, packageArch);
  mkdirSync(linuxDist, { recursive: true });
  const staging = mkdtempSync(join(linuxDist, `.container-export-${packageArch}-`));
  if (staging.includes(",")) {
    rmSync(staging, { recursive: true, force: true });
    throw new Error("Docker package output paths containing commas are not supported");
  }
  let previous: string | undefined;
  try {
    // docker cp extracts through the host client, avoiding UID-map assumptions
    // for bind-mounted output under rootless and user-namespace daemons.
    capture(["cp", `${container}:/workspace/dist/linux/${packageArch}/.`, staging]);
    const files = readdirSync(staging, { withFileTypes: true });
    const appImages = files.filter(entry => entry.isFile() && entry.name.endsWith(`-Linux-${packageArch}.AppImage`));
    const tarballs = files.filter(entry => entry.isFile() && entry.name.endsWith(`-Linux-${packageArch}.tar.gz`));
    if (appImages.length !== 1 || tarballs.length !== 1) {
      throw new Error(`package export expected one ${packageArch} AppImage and tarball, found ${appImages.length} and ${tarballs.length}`);
    }
    if (existsSync(destination)) {
      previous = `${destination}.previous-${process.pid}-${crypto.randomUUID()}`;
      renameSync(destination, previous);
    }
    try {
      renameSync(staging, destination);
    } catch (error) {
      if (previous) renameSync(previous, destination);
      throw error;
    }
    if (previous) rmSync(previous, { recursive: true, force: true });
    console.log(`Exported Linux artifacts to ${relative(root, destination)}`);
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
}

async function main(args: string[]): Promise<number> {
  if (args[1] === "--help") {
    console.log("Usage: mise run linux:{test,smoke,package,exec,clean} -- [--arch native|amd64|arm64] [command ...]\nlinux:package exports verified artifacts to host dist/. linux:exec requires a command. linux:clean removes this worktree's selected-architecture cache volumes.");
    return 0;
  }
  const options = argumentsFor(args);
  const info = capture(["info", "--format", "{{.OSType}}/{{.Architecture}}"]);
  const [os, daemonArch] = info!.split("/");
  if (os !== "linux") throw new Error("Linux tests require a Linux Docker engine");
  const arch = options.arch === "native" ? architecture(daemonArch!) : architecture(options.arch);
  const root = realpathSync(repository);
  // Docker's --mount syntax cannot represent a comma in its source field.
  if (root.includes(",")) throw new Error("Docker source paths containing commas are not supported");
  const names = cacheNames(root, arch);
  if (options.mode === "clean") {
    for (const volume of names.volumes) {
      if (capture(["volume", "inspect", volume], false) !== undefined) {
        capture(["volume", "rm", volume]);
        console.log(`Removed ${volume}`);
      }
    }
    return 0;
  }

  const revision = Bun.spawnSync(["git", "rev-parse", "HEAD"], { cwd: root, stdout: "pipe", stderr: "pipe" });
  if (revision.exitCode !== 0) throw new Error(`cannot resolve source revision: ${revision.stderr.toString().trim()}`);
  const sourceDateEpoch = Bun.spawnSync(["git", "show", "-s", "--format=%ct", "HEAD"], { cwd: root, stdout: "pipe", stderr: "pipe" });
  if (sourceDateEpoch.exitCode !== 0 || !/^\d+\s*$/.test(sourceDateEpoch.stdout.toString())) {
    throw new Error(`cannot resolve source commit time: ${sourceDateEpoch.stderr.toString().trim()}`);
  }

  const inputs = [
    ["scripts/linux/Dockerfile", "Dockerfile"],
    ["scripts/linux/entrypoint.sh", "entrypoint.sh"],
    ["mise.toml", "mise.toml"], ["mise.lock", "mise.lock"],
    ["rust-toolchain.toml", "rust-toolchain.toml"],
  ] as const;
  const digest = createHash("sha256");
  for (const [source] of inputs) digest.update(source).update("\0").update(readFileSync(join(root, source)));
  const image = `huterm-linux:${arch}-${digest.digest("hex").slice(0, 16)}`;
  console.log(`Linux ${arch}${arch !== architecture(daemonArch!) ? " (emulated)" : " (native)"}: ${options.command.join(" ")}`);
  let child: ReturnType<typeof Bun.spawn> | undefined;
  let container: string | undefined;
  let interrupted = 0;
  const stop = (signal: "SIGINT" | "SIGTERM") => {
    interrupted = signal === "SIGINT" ? 130 : 143;
    child?.kill(signal);
    if (container) capture(["stop", "--time", "2", container], false);
  };
  const onInterrupt = () => stop("SIGINT");
  const onTerminate = () => stop("SIGTERM");
  process.on("SIGINT", onInterrupt);
  process.on("SIGTERM", onTerminate);
  async function run(command: string[]): Promise<number> {
    child = Bun.spawn(command, { stdin: "inherit", stdout: "inherit", stderr: "inherit" });
    const status = await child.exited;
    child = undefined;
    return interrupted || status;
  }
  try {
    if (capture(["image", "inspect", image], false) === undefined) {
      const context = mkdtempSync(join(tmpdir(), "huterm-linux-build-"));
      try {
        for (const [source, target] of inputs) copyFileSync(join(root, source), join(context, target));
        const status = await run(["docker", "build", "--load", "--platform", `linux/${arch}`, "--tag", image, context]);
        if (status !== 0) return status;
      } finally { rmSync(context, { recursive: true, force: true }); }
    }
    if (interrupted) return interrupted;
    for (const volume of names.volumes) {
      capture(["volume", "create", "--label", `app.huterm.worktree=${names.worktree}`, "--label", `app.huterm.arch=${arch}`, volume]);
    }
    // A fixed name rejects concurrent runs before either can sync the shared
    // workspace. Only remove a container after this invocation created it.
    container = capture([
      "create", "--init", "--interactive", "--name", names.name, "--platform", `linux/${arch}`,
      "--env", `HUTERM_SOURCE_REVISION=${revision.stdout.toString().trim()}`,
      "--env", `HUTERM_SOURCE_DATE_EPOCH=${sourceDateEpoch.stdout.toString().trim()}`,
      ...(options.mode === "package" ? ["--env", "HUTERM_LINUX_CLEAN_DIST=1"] : []),
      "--mount", `type=bind,source=${root},target=/source,readonly`,
      "--mount", `type=volume,source=${names.volumes[0]},target=/workspace`,
      "--mount", `type=volume,source=${names.volumes[1]},target=/cache`,
      image, ...options.command,
    ]);
    if (interrupted) return interrupted;
    const status = await run(["docker", "start", "--attach", "--interactive", container!]);
    if (status === 0 && options.mode === "package") exportPackageArtifacts(root, container!, arch);
    return status;
  } finally {
    if (container) capture(["rm", "--force", container], false);
    process.off("SIGINT", onInterrupt);
    process.off("SIGTERM", onTerminate);
  }
}

if (import.meta.main) {
  try { process.exitCode = await main(process.argv.slice(2)); }
  catch (error) { console.error(`Linux runner: ${error instanceof Error ? error.message : String(error)}`); process.exitCode = 1; }
}
