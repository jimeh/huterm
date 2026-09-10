import { afterEach, expect, test } from "bun:test";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileHash, treeHash } from "./prepare-ghostty.ts";
import { prepareSparkle, type SparkleManifest, verifySparkleDistribution, verifySparklePolicy } from "./prepare-sparkle.ts";

const directories: string[] = [];
function temporary(): string {
  const directory = mkdtempSync(join(tmpdir(), "huterm-sparkle-"));
  directories.push(directory);
  return directory;
}
afterEach(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });

function distribution(): string {
  const root = temporary();
  const version = join(root, "Sparkle.framework/Versions/B");
  mkdirSync(join(version, "Resources"), { recursive: true });
  mkdirSync(join(root, "Sparkle.framework/Versions"), { recursive: true });
  symlinkSync("B", join(root, "Sparkle.framework/Versions/Current"));
  for (const [name, target] of [["Sparkle", "Versions/Current/Sparkle"], ["Resources", "Versions/Current/Resources"]] as const) {
    symlinkSync(target, join(root, "Sparkle.framework", name));
  }
  writeFileSync(join(version, "Sparkle"), "universal fixture");
  writeFileSync(join(version, "Resources/Info.plist"), `
    <key>CFBundleIdentifier</key><string>org.sparkle-project.Sparkle</string>
    <key>CFBundleShortVersionString</key><string>2.9.6</string>
    <key>LSMinimumSystemVersion</key><string>10.13</string>`);
  mkdirSync(join(root, "bin"));
  for (const tool of ["generate_appcast", "generate_keys", "sign_update"]) {
    const toolPath = join(root, "bin", tool);
    writeFileSync(toolPath, tool);
    chmodSync(toolPath, 0o755);
  }
  writeFileSync(join(root, "LICENSE"), "reviewed license");
  return root;
}

function manifest(sourceRoot: string): SparkleManifest {
  return {
    version: "2.9.6",
    published_at: "2026-08-17T01:40:46Z",
    minimum_macos: "10.13",
    source: { name: "Sparkle-2.9.6.tar.xz", url: "https://invalid.example/not-fetched", sha256: "0".repeat(64), tree_sha256: treeHash(sourceRoot) },
    framework: { identifier: "org.sparkle-project.Sparkle", license_sha256: fileHash(join(sourceRoot, "LICENSE")) },
    license_url: "https://github.com/sparkle-project/Sparkle/blob/2.9.6/LICENSE",
  };
}

test("distribution verification binds layout identity minimum OS tools and license", () => {
  const root = distribution();
  const pin = manifest(root);
  expect(() => verifySparkleDistribution(root, pin)).not.toThrow();
  writeFileSync(join(root, "LICENSE"), "tampered");
  expect(() => verifySparkleDistribution(root, pin)).toThrow("contents differ");
});

test("preparation extracts verified bytes and check mode never repairs", async () => {
  const source = distribution();
  const pin = manifest(source);
  const served = temporary();
  const archive = join(served, pin.source.name);
  const packed = Bun.spawnSync(["tar", "-cJf", archive, "-C", source, "."]);
  expect(packed.exitCode, packed.stderr.toString()).toBe(0);
  pin.source.sha256 = fileHash(archive);
  const bytes = await Bun.file(archive).bytes();
  const server = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: () => new Response(bytes) });
  pin.source.url = server.url.toString();
  const root = temporary();
  try {
    await prepareSparkle(pin, root, false);
    expect(existsSync(join(root, "distribution/Sparkle.framework"))).toBe(true);
    await prepareSparkle(pin, root, true);
    writeFileSync(join(root, "distribution/LICENSE"), "tampered");
    await expect(prepareSparkle(pin, root, false)).rejects.toThrow("contents differ");
  } finally {
    server.stop(true);
  }
});

test("check mode rejects a missing distribution without downloading", async () => {
  const source = distribution();
  const root = temporary();
  await expect(prepareSparkle(manifest(source), root, true)).rejects.toThrow("missing Sparkle distribution");
  expect(existsSync(join(root, "archives"))).toBe(false);
});

test("distribution verification rejects escaping symlinks", () => {
  const root = distribution();
  symlinkSync("../../outside", join(root, "escape"));
  const pin = manifest(root);
  expect(() => verifySparkleDistribution(root, pin)).toThrow("symlink escapes");
});

test("policy binds the committed license and three-day release age", () => {
  const source = distribution();
  const pinned = manifest(source);
  expect(() => verifySparklePolicy(pinned, join(source, "LICENSE"), new Date("2026-08-20T01:40:46Z"))).not.toThrow();
  expect(() => verifySparklePolicy(pinned, join(source, "LICENSE"), new Date("2026-08-20T01:40:45Z"))).toThrow("three-day");
  writeFileSync(join(source, "LICENSE"), "changed");
  expect(() => verifySparklePolicy(pinned, join(source, "LICENSE"), new Date("2026-09-10T00:00:00Z"))).toThrow("license");
});
