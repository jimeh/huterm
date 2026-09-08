/** Run existing Linux checks in a disposable Docker container. */
import { createHash } from "node:crypto";
import { copyFileSync, mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const repository = resolve(import.meta.dir, "..");
type Mode = "test" | "smoke" | "exec" | "clean";
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
  if (mode !== "test" && mode !== "smoke" && mode !== "exec" && mode !== "clean") {
    throw new Error("expected test, smoke, exec, or clean");
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
  return { mode, arch, command: mode === "test" ? ["mise", "run", "test:rust"] : mode === "smoke" ? ["mise", "run", "ci:smoke"] : command };
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

async function main(args: string[]): Promise<number> {
  if (args[1] === "--help") {
    console.log("Usage: mise run linux:{test,smoke,exec,clean} -- [--arch native|amd64|arm64] [command ...]\nlinux:exec requires a command. linux:clean removes this worktree's selected-architecture cache volumes.");
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
      "--mount", `type=bind,source=${root},target=/source,readonly`,
      "--mount", `type=volume,source=${names.volumes[0]},target=/workspace`,
      "--mount", `type=volume,source=${names.volumes[1]},target=/cache`,
      image, ...options.command,
    ]);
    if (interrupted) return interrupted;
    return await run(["docker", "start", "--attach", "--interactive", container!]);
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
