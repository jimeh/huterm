import { afterEach, describe, expect, test } from "bun:test";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { architecture, argumentsFor, cacheNames } from "./linux";

const directories: string[] = [];
afterEach(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });

function fixture(overrides: Record<string, string> = {}) {
  const directory = mkdtempSync(join(tmpdir(), "huterm-linux-test-"));
  directories.push(directory);
  const log = join(directory, "docker.jsonl");
  writeFileSync(log, "");
  const docker = join(directory, "docker");
  writeFileSync(docker, `#!/usr/bin/env bun
import { appendFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
const args = process.argv.slice(2);
appendFileSync(process.env.DOCKER_TEST_LOG, JSON.stringify(args) + '\\n');
if (args[0] === 'info') console.log(process.env.DOCKER_TEST_PLATFORM || 'linux/aarch64');
if (args[0] === 'image' && args[1] === 'inspect') process.exit(Number(process.env.DOCKER_TEST_IMAGE_MISSING || 0));
if (args[0] === 'build') process.exit(Number(process.env.DOCKER_TEST_BUILD_EXIT || 0));
if (args[0] === 'create') {
  if (process.env.DOCKER_TEST_CONFLICT) { console.error('container name already in use'); process.exit(125); }
  console.log('owned-container-id');
}
if (args[0] === 'start') {
  if (process.env.DOCKER_TEST_HOLD) { console.log('STARTED'); await new Promise(() => setInterval(() => {}, 1000)); }
  process.exit(Number(process.env.DOCKER_TEST_EXIT || 0));
}
if (args[0] === 'cp') {
  if (process.env.DOCKER_TEST_EXPORT_EXIT) process.exit(Number(process.env.DOCKER_TEST_EXPORT_EXIT));
  const source = args[1];
  const output = args[2];
  if (!source || !output) { console.error('missing docker cp path'); process.exit(2); }
  const packageArch = source.includes('/aarch64/') ? 'aarch64' : 'x86_64';
  mkdirSync(join(output, 'package-evidence'), { recursive: true });
  writeFileSync(join(output, \`Huterm-0.4.0-Linux-\${packageArch}.AppImage\`), 'appimage');
  writeFileSync(join(output, \`Huterm-0.4.0-Linux-\${packageArch}.tar.gz\`), 'tarball');
}
`);
  chmodSync(docker, 0o755);
  const git = join(directory, "git");
  writeFileSync(git, `#!/usr/bin/env bun
if (process.env.GIT_TEST_FAIL) { console.error("missing repository"); process.exit(128); }
if (process.argv.includes("--format=%ct")) console.log("1700000000");
else console.log("0123456789abcdef0123456789abcdef01234567");
`);
  chmodSync(git, 0o755);
  const output = join(directory, "host-dist");
  const env = { ...process.env, PATH: `${directory}:${process.env.PATH}`, DOCKER_TEST_LOG: log, HUTERM_LINUX_DIST_DIR: output, ...overrides };
  return {
    env,
    output,
    calls: () => readFileSync(log, "utf8").trim().split("\n").filter(Boolean).map((line) => JSON.parse(line) as string[]),
    run: (...args: string[]) => Bun.spawnSync([process.execPath, join(import.meta.dir, "linux.ts"), ...args], { env, stdout: "pipe", stderr: "pipe" }),
  };
}

describe("Linux container runner", () => {
  test("architecture aliases and invalid values", () => {
    expect(architecture("aarch64")).toBe("arm64");
    expect(architecture("AMD64")).toBe("amd64");
    expect(architecture("x86_64")).toBe("amd64");
    expect(() => architecture("sparc")).toThrow("unsupported architecture");
  });

  test("arguments default to native and preserve the custom command verbatim", () => {
    expect(argumentsFor(["test"])).toEqual({ mode: "test", arch: "native", command: ["mise", "run", "test:rust"] });
    expect(argumentsFor(["package"])).toEqual({ mode: "package", arch: "native", command: ["mise", "run", "package:linux"] });
    expect(argumentsFor(["exec", "--arch=ARM64", "--", "printf", "%s", "a b", "$(literal)", "--help"]).command).toEqual(["printf", "%s", "a b", "$(literal)", "--help"]);
    for (const args of [["exec"], ["test", "--arch"], ["test", "--bogus"], ["test", "extra"], ["clean", "extra"]]) {
      expect(() => argumentsFor(args)).toThrow();
    }
  });

  test("cache scopes separate worktrees and architectures", () => {
    expect(cacheNames("/checkout/a", "arm64")).toEqual(cacheNames("/checkout/a", "arm64"));
    expect(cacheNames("/checkout/a", "arm64").volumes).not.toEqual(cacheNames("/checkout/a", "amd64").volumes);
    expect(cacheNames("/checkout/a", "arm64").volumes).not.toEqual(cacheNames("/checkout/b", "arm64").volumes);
  });

  test("native uses the engine architecture and forwards literal argv", () => {
    const f = fixture({ DOCKER_TEST_PLATFORM: "linux/x86_64" });
    const result = f.run("exec", "printf", "%s", "a b", "$(literal)", "--help");
    expect(result.exitCode).toBe(0);
    const create = f.calls().find((args) => args[0] === "create")!;
    expect(create[create.indexOf("--platform") + 1]).toBe("linux/amd64");
    expect(create).toContain("HUTERM_SOURCE_REVISION=0123456789abcdef0123456789abcdef01234567");
    expect(create).toContain("HUTERM_SOURCE_DATE_EPOCH=1700000000");
    expect(create.slice(-5)).toEqual(["printf", "%s", "a b", "$(literal)", "--help"]);
    expect(create.some((arg) => arg.endsWith("target=/source,readonly"))).toBe(true);
    expect(f.calls().at(-1)).toEqual(["rm", "--force", "owned-container-id"]);
  });

  test("explicit architecture overrides native and command failures survive cleanup", () => {
    const f = fixture({ DOCKER_TEST_EXIT: "23" });
    expect(f.run("test", "--arch", "amd64").exitCode).toBe(23);
    const create = f.calls().find((args) => args[0] === "create")!;
    expect(create[create.indexOf("--platform") + 1]).toBe("linux/amd64");
    expect(f.calls().at(-1)).toEqual(["rm", "--force", "owned-container-id"]);
  });

  test("package exports verified artifacts from the workspace volume to host dist", () => {
    const f = fixture({ DOCKER_TEST_PLATFORM: "linux/x86_64" });
    const result = f.run("package");
    expect(result.exitCode, result.stderr.toString()).toBe(0);
    expect(readFileSync(join(f.output, "linux/x86_64/Huterm-0.4.0-Linux-x86_64.AppImage"), "utf8")).toBe("appimage");
    expect(readFileSync(join(f.output, "linux/x86_64/Huterm-0.4.0-Linux-x86_64.tar.gz"), "utf8")).toBe("tarball");
    const calls = f.calls();
    const create = calls.find(args => args[0] === "create")!;
    expect(create.slice(-3)).toEqual(["mise", "run", "package:linux"]);
    expect(create).toContain("HUTERM_LINUX_CLEAN_DIST=1");
    const exportCall = calls.find(args => args[0] === "cp")!;
    expect(exportCall[1]).toBe("owned-container-id:/workspace/dist/linux/x86_64/.");
    expect(exportCall[2]?.startsWith(join(f.output, "linux/.container-export-x86_64-"))).toBe(true);
    expect(calls.some(args => args[0] === "run")).toBe(false);
    expect(calls.findIndex(args => args[0] === "start")).toBeLessThan(calls.findIndex(args => args[0] === "cp"));
  });

  test("package exports an explicit arm64 build to the aarch64 package directory", () => {
    const f = fixture({ DOCKER_TEST_PLATFORM: "linux/x86_64" });
    const result = f.run("package", "--arch", "arm64");
    expect(result.exitCode, result.stderr.toString()).toBe(0);
    expect(readFileSync(join(f.output, "linux/aarch64/Huterm-0.4.0-Linux-aarch64.AppImage"), "utf8")).toBe("appimage");
    expect(readFileSync(join(f.output, "linux/aarch64/Huterm-0.4.0-Linux-aarch64.tar.gz"), "utf8")).toBe("tarball");
    const create = f.calls().find(args => args[0] === "create")!;
    expect(create[create.indexOf("--platform") + 1]).toBe("linux/arm64");
    expect(f.calls().find(args => args[0] === "cp")?.[1]).toBe("owned-container-id:/workspace/dist/linux/aarch64/.");
  });

  test("package workspace sync drops host and retained dist artifacts", () => {
    const root = mkdtempSync(join(tmpdir(), "huterm-linux-sync-test-"));
    directories.push(root);
    const source = join(root, "source");
    const workspace = join(root, "workspace");
    mkdirSync(join(source, "dist/linux/x86_64"), { recursive: true });
    mkdirSync(join(workspace, "dist/linux/x86_64"), { recursive: true });
    writeFileSync(join(source, "source.txt"), "current source");
    writeFileSync(join(source, "dist/linux/x86_64/Huterm-0.3.0-Linux-x86_64.AppImage"), "host artifact");
    writeFileSync(join(workspace, "dist/linux/x86_64/Huterm-0.2.0-Linux-x86_64.AppImage"), "retained artifact");
    writeFileSync(join(workspace, "obsolete.txt"), "obsolete source");
    const entrypoint = join(import.meta.dir, "linux/entrypoint.sh");
    const result = Bun.spawnSync([
      "bash", "-c", 'source "$1"; sync_workspace "$2" "$3" 1', "bash", entrypoint, source, workspace,
    ], { stdout: "pipe", stderr: "pipe" });
    expect(result.exitCode, result.stderr.toString()).toBe(0);
    expect(readFileSync(join(workspace, "source.txt"), "utf8")).toBe("current source");
    expect(existsSync(join(workspace, "obsolete.txt"))).toBe(false);
    expect(existsSync(join(workspace, "dist"))).toBe(false);
  });

  test("package export failure preserves existing host artifacts", () => {
    const f = fixture({ DOCKER_TEST_PLATFORM: "linux/x86_64", DOCKER_TEST_EXPORT_EXIT: "23" });
    const existing = join(f.output, "linux/x86_64/existing.txt");
    mkdirSync(join(f.output, "linux/x86_64"), { recursive: true });
    writeFileSync(existing, "keep");
    const result = f.run("package");
    expect(result.exitCode).toBe(1);
    expect(result.stderr.toString()).toContain("docker cp failed");
    expect(readFileSync(existing, "utf8")).toBe("keep");
  });

  test("source revision failure prevents creating a container", () => {
    const f = fixture({ GIT_TEST_FAIL: "1" });
    const result = f.run("test");
    expect(result.exitCode).toBe(1);
    expect(result.stderr.toString()).toContain("cannot resolve source revision: missing repository");
    expect(f.calls().some((args) => args[0] === "create" || args[0] === "volume")).toBe(false);
  });

  test("build failures propagate without creating a container", () => {
    const f = fixture({ DOCKER_TEST_IMAGE_MISSING: "1", DOCKER_TEST_BUILD_EXIT: "17" });
    expect(f.run("test").exitCode).toBe(17);
    expect(f.calls().some((args) => args[0] === "build")).toBe(true);
    expect(f.calls().some((args) => args[0] === "create")).toBe(false);
  });

  test("name conflicts never remove another run's container", () => {
    const f = fixture({ DOCKER_TEST_CONFLICT: "1" });
    expect(f.run("test").exitCode).toBe(1);
    expect(f.calls().some((args) => args[0] === "rm")).toBe(false);
  });

  test("invalid architecture is rejected before calling Docker", () => {
    const f = fixture();
    expect(f.run("test", "--arch", "invalid").exitCode).toBe(1);
    expect(f.calls()).toEqual([]);
  });

  test("cleanup only removes this worktree's selected-architecture volumes", () => {
    const f = fixture();
    expect(f.run("clean", "--arch", "amd64").exitCode).toBe(0);
    const removals = f.calls().filter((args) => args[0] === "volume" && args[1] === "rm");
    expect(removals).toHaveLength(2);
    expect(removals.every((args) => /^huterm-linux-[a-f0-9]{16}-amd64-(workspace|cache)$/.test(args[2]!))).toBe(true);
    expect(f.calls().some((args) => args[0] === "build" || args[0] === "create")).toBe(false);
  });

  test("termination stops and removes only the owned container", async () => {
    const f = fixture({ DOCKER_TEST_HOLD: "1" });
    const child = Bun.spawn([process.execPath, join(import.meta.dir, "linux.ts"), "test"], { env: f.env, stdout: "pipe", stderr: "pipe" });
    try {
      const reader = child.stdout.getReader();
      let output = "";
      while (!output.includes("STARTED")) {
        const result = await reader.read();
        if (result.done) throw new Error("runner exited before starting the container");
        output += new TextDecoder().decode(result.value);
      }
      child.kill("SIGTERM");
      expect(await child.exited).toBe(143);
      expect(f.calls()).toContainEqual(["stop", "--time", "2", "owned-container-id"]);
      expect(f.calls().at(-1)).toEqual(["rm", "--force", "owned-container-id"]);
    } finally { child.kill(); }
  }, 10_000);
});
