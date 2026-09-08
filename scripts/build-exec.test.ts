import { afterEach, expect, test } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const directories: string[] = [];
afterEach(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });

function run(platform: string, selected = "27.0", fallback = "26.2", developer?: string, exit = 0, sdkTargets?: string, autoFallback = false) {
  const directory = mkdtempSync(join(tmpdir(), "huterm-sdk-"));
  directories.push(directory);
  const executable = (name: string, script: string) => {
    const file = join(directory, name);
    writeFileSync(file, `#!/bin/bash\n${script}\n`);
    chmodSync(file, 0o755);
  };
  executable("uname", 'if [ "$1" = -m ]; then echo "$TEST_ARCH"; else echo "$TEST_PLATFORM"; fi');
  executable("xcrun", 'if [ -n "${DEVELOPER_DIR:-}" ]; then\n  [ "$TEST_FALLBACK" != missing ] || exit 1\n  echo "$TEST_FALLBACK"\nelse\n  echo "$TEST_SELECTED"\nfi');
  executable("command", 'printf "%s\\n" "${DEVELOPER_DIR:-unset}" "$@"\nexit "$TEST_EXIT"');
  const env: NodeJS.ProcessEnv = { ...process.env, PATH: `${directory}:${process.env.PATH}`, TEST_PLATFORM: platform, TEST_SELECTED: selected, TEST_FALLBACK: fallback, TEST_EXIT: String(exit) };
  delete env.DEVELOPER_DIR;
  delete env.HUTERM_ZIG_SDKROOT;
  env.TEST_ARCH = "x86_64";
  if (sdkTargets !== undefined) {
    env.TEST_ARCH = "arm64";
    const sdk = join(directory, "SDK with spaces");
    mkdirSync(join(sdk, "usr/lib"), { recursive: true });
    writeFileSync(join(sdk, "usr/lib/libSystem.tbd"), `targets: [ ${sdkTargets} ]\ninstall-name: '/usr/lib/libSystem.B.dylib'\n--- !tapi-tbd\ntargets: [ arm64-macos ]\n`);
    env.HUTERM_ZIG_SDKROOT = sdk;
    if (autoFallback) {
      delete env.HUTERM_ZIG_SDKROOT;
      env.TEST_SDK = sdk;
      const compatible = join(directory, "MacOSX15.4.sdk");
      mkdirSync(join(compatible, "usr/lib"), { recursive: true });
      writeFileSync(join(compatible, "usr/lib/libSystem.tbd"), "targets: [ arm64-macos ]\n");
      executable("xcrun", 'if [ "$3" = --show-sdk-path ]; then echo "$TEST_SDK"; else echo "$TEST_SELECTED"; fi');
    }
    executable("command", 'xcrun --sdk macosx --show-sdk-path');
  }
  if (developer) env.DEVELOPER_DIR = developer;
  const result = Bun.spawnSync(["/bin/bash", join(import.meta.dir, "build-exec.sh"), "command", "argument with spaces", "$(literal)"], { env });
  return { code: result.exitCode, out: result.stdout.toString(), err: result.stderr.toString() };
}

test("Linux does not query the SDK and preserves arguments", () => {
  const result = run("Linux", "invalid");
  expect(result.code).toBe(0);
  expect(result.out).toBe("unset\nargument with spaces\n$(literal)\n");
});
test("explicit developer directory is preserved", () => {
  expect(run("Darwin", "invalid", "missing", "/custom/Xcode").out).toStartWith("/custom/Xcode\n");
});
test("compatible selected SDK is preserved", () => {
  expect(run("Darwin", "26.2").out).toStartWith("unset\n");
});
test("beta SDK selects installed Xcode 26", () => {
  const result = run("Darwin");
  expect(result.code).toBe(0);
  expect(result.out).toStartWith("/Applications/Xcode.app/Contents/Developer\n");
  expect(result.err).toContain("using macOS SDK 26.2");
});
for (const fallback of ["missing", "27.0"]) {
  test(`reports actionable failure for fallback ${fallback}`, () => {
    const result = run("Darwin", "27.0", fallback);
    expect(result.code).toBe(1);
    expect(result.err).toContain("set DEVELOPER_DIR");
    expect(result.out).toBe("");
  });
}
test("preserves invoked command exit code", () => {
  expect(run("Linux", "27.0", "26.2", undefined, 37).code).toBe(37);
});

test("routes the SDK query to explicit compatible stubs with spaces in the path", () => {
  const result = run("Darwin", "26.5", "26.5", undefined, 0, "arm64-macos, arm64e-macos");
  expect(result.code).toBe(0);
  expect(result.out.trim()).toEndWith("/SDK with spaces");
});

test("rejects explicit arm64e-only SDK before invoking the build", () => {
  const result = run("Darwin", "26.5", "26.5", undefined, 0, "arm64e-macos");
  expect(result.code).toBe(1);
  expect(result.err).toContain("lacks arm64-macos system stubs");
  expect(result.out).toBe("");
});

test("falls back from arm64e-only stubs to an installed compatible SDK", () => {
  const result = run("Darwin", "26.5", "26.5", undefined, 0, "arm64e-macos", true);
  expect(result.code).toBe(0);
  expect(result.out.trim()).toEndWith("/MacOSX15.4.sdk");
  expect(result.err).toContain("selected SDK lacks arm64-macos");
});
