import { expect, test } from "bun:test";
import { chmod, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { cliArchiveEntries, cliArchiveName, stageCliDirectory, validateCliRuns, validateMacosCliLinkage, verifyCliDirectory } from "./cli-package.ts";

test("standalone archives are named per platform and hold one directory", () => {
  expect(cliArchiveName("1.2.3", "macOS-universal")).toBe("Huterm-CLI-1.2.3-macOS-universal.zip");
  expect(cliArchiveName("1.2.3", "Linux-aarch64")).toBe("Huterm-CLI-1.2.3-Linux-aarch64.tar.gz");
  expect(cliArchiveEntries("Huterm-CLI-1.2.3-Linux-x86_64")).toEqual([
    "Huterm-CLI-1.2.3-Linux-x86_64/",
    "Huterm-CLI-1.2.3-Linux-x86_64/LICENSE",
    "Huterm-CLI-1.2.3-Linux-x86_64/huterm",
    "Huterm-CLI-1.2.3-Linux-x86_64/licenses.json",
  ]);
  expect(() => cliArchiveName("1.2", "macOS-universal")).toThrow("invalid CLI version");
});

test("runs must follow the help, version, and usage-error conventions", () => {
  const ok = (stdout: string) => ({ stdout, stderr: "", exitCode: 0 });
  const valid = {
    version: ok("huterm 1.2.3\n"),
    help: ok("Usage: huterm\n"),
    usage: { stdout: "", stderr: "error: unexpected argument\n\nUsage: huterm\n", exitCode: 2 },
  };
  expect(() => validateCliRuns(valid, "1.2.3", "huterm")).not.toThrow();
  expect(() => validateCliRuns({ ...valid, version: ok("huterm 1.2.4\n") }, "1.2.3", "huterm")).toThrow("--version");
  expect(() => validateCliRuns({ ...valid, help: { ...valid.help, exitCode: 1 } }, "1.2.3", "huterm")).toThrow("--help");
  // A launcher that ignores unknown arguments would open the app instead.
  expect(() => validateCliRuns({ ...valid, usage: ok("") }, "1.2.3", "huterm")).toThrow("unknown argument");
});

test("the macOS command line links only libSystem", () => {
  expect(() => validateMacosCliLinkage(["/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1359.0.0)"])).not.toThrow();
  expect(() => validateMacosCliLinkage([
    "/usr/lib/libSystem.B.dylib (compatibility version 1.0.0)",
    "/System/Library/Frameworks/AppKit.framework/Versions/C/AppKit (compatibility version 45.0.0)",
  ])).toThrow("AppKit");
  expect(() => validateMacosCliLinkage([])).toThrow("no dependencies");
});

test("archive directories hold regular files with fixed modes and the committed notices", async () => {
  const root = await mkdtemp(join(tmpdir(), "huterm-cli-directory-"));
  try {
    const executable = join(root, "built-huterm");
    await writeFile(executable, "binary");
    const directory = await stageCliDirectory(root, "Huterm-CLI-1.2.3-Linux-x86_64", executable);
    expect((await verifyCliDirectory(directory)).toString()).toBe("binary");

    await chmod(join(directory, "LICENSE"), 0o600);
    await expect(verifyCliDirectory(directory)).rejects.toThrow("LICENSE is not a mode 644 file");
    await chmod(join(directory, "LICENSE"), 0o644);

    // A symlink to an identical notice is still not a regular file.
    await rm(join(directory, "LICENSE"));
    await symlink(resolve(import.meta.dir, "../LICENSE"), join(directory, "LICENSE"));
    await expect(verifyCliDirectory(directory)).rejects.toThrow();
  } finally { await rm(root, { recursive: true, force: true }); }
});
