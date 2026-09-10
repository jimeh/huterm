import { afterEach, expect, test } from "bun:test";
import { copyFileSync, cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { checkIcons, generateIcons, iconFiles, validateIconFiles, verifyBundleIcons } from "./icons.ts";

const repository = resolve(import.meta.dir, "..");
const directories: string[] = [];
afterEach(() => {
  for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true });
});

function fixture(): string {
  const root = mkdtempSync(join(tmpdir(), "huterm-icon-test-"));
  directories.push(root);
  cpSync(join(repository, "assets"), join(root, "assets"), { recursive: true });
  mkdirSync(join(root, "scripts"));
  copyFileSync(join(repository, "scripts/icons.ts"), join(root, "scripts/icons.ts"));
  return root;
}

test("icon check accepts the committed source and outputs without Apple tools", () => {
  checkIcons(fixture());
});

for (const version of ["Xcode 26.3\nBuild version 17C529", "Xcode 28.0\nBuild version 28A123", "Xcode unknown", undefined]) {
  test(`icon check rejects unsupported recorded Xcode version: ${version}`, () => {
    const root = fixture();
    const file = join(root, "assets/icons.json");
    const manifest = JSON.parse(readFileSync(file, "utf8"));
    manifest.tools.xcode = version;
    writeFileSync(file, JSON.stringify(manifest));
    expect(() => checkIcons(root)).toThrow("icon assets require Xcode 27");
  });
}

for (const version of ["Xcode 26.3\nBuild version 17C529", "Xcode 28.0\nBuild version 28A123"]) {
  test(`generation rejects unsupported Xcode before exporting: ${version}`, () => {
    const root = fixture();
    const original = [...iconFiles, "assets/icons.json"].map(file => readFileSync(join(root, file)));
    const run = (args: string[]): string => {
      if (args[0] === "xcodebuild") return version;
      throw new Error(`unexpected command: ${args.join(" ")}`);
    };
    expect(() => generateIcons(root, run)).toThrow("icon assets require Xcode 27");
    for (const [index, file] of [...iconFiles, "assets/icons.json"].entries()) {
      expect(readFileSync(join(root, file))).toEqual(original[index]!);
    }
  });
}

for (const change of ["edit", "add", "remove", "generator"] as const) {
  test(`icon check detects source ${change}`, () => {
    const root = fixture();
    const layer = join(root, "assets/Huterm.icon/Assets/prompt-large.svg");
    if (change === "edit") writeFileSync(layer, "changed artwork");
    if (change === "add") writeFileSync(join(root, "assets/Huterm.icon/Assets/new.svg"), "new artwork");
    if (change === "remove") rmSync(layer);
    if (change === "generator") writeFileSync(join(root, "scripts/icons.ts"), "changed rendering options");
    expect(() => checkIcons(root)).toThrow("icon source changed");
  });
}

test("icon check rejects changed output bytes and missing files", () => {
  const root = fixture();
  writeFileSync(join(root, "assets/Huterm.png"), "stale image");
  expect(() => checkIcons(root)).toThrow("icon output changed: assets/Huterm.png");
  rmSync(join(root, "assets/Huterm.png"));
  expect(() => checkIcons(root)).toThrow("ENOENT");
});

test("generation rejects a successful Apple export with no outputs and preserves committed assets", () => {
  const root = fixture();
  const original = iconFiles.map(file => readFileSync(join(root, file)));
  const manifest = readFileSync(join(root, "assets/icons.json"));
  const run = (args: string[]): string => {
    if (args[0] === "xcrun" && args[1] === "--find") return "/Applications/Xcode.app/Contents/Developer/usr/bin/actool";
    if (args[0] === "xcodebuild") return "Xcode 27.0\nBuild version 27A5228h";
    if (args[1] === "--version") return "{}";
    if (args[0] === "sw_vers") return "fixture";
    if (args[1] === "actool") return "Icon export exited with status 255, signal 0";
    if (args[0] === "plutil") return JSON.stringify({ CFBundleIconFile: "Huterm", CFBundleIconName: "Huterm" });
    if (args[1] === "assetutil") return JSON.stringify([{ Name: "Huterm", AssetType: "IconImageStack" }]);
    throw new Error(`unexpected command: ${args.join(" ")}`);
  };
  expect(() => generateIcons(root, run)).toThrow("ENOENT");
  for (const [index, file] of iconFiles.entries()) expect(readFileSync(join(root, file))).toEqual(original[index]!);
  expect(readFileSync(join(root, "assets/icons.json"))).toEqual(manifest);
});

test("generated PNG must have the full 1024-pixel resolution", () => {
  const root = fixture();
  const file = join(root, "assets/Huterm.png");
  const png = readFileSync(file);
  png.writeUInt32BE(256, 16);
  png.writeUInt32BE(256, 20);
  writeFileSync(file, png);
  expect(() => validateIconFiles(root)).toThrow("1024x1024 PNG");
});

test("bundle verification rejects wrong icon metadata and stale packaged resources", () => {
  const root = fixture();
  const bundle = join(root, "Huterm.app");
  const resources = join(bundle, "Contents/Resources");
  mkdirSync(resources, { recursive: true });
  copyFileSync(join(root, "assets/Huterm.icns"), join(resources, "Huterm.icns"));
  copyFileSync(join(root, "assets/macos/Assets.car"), join(resources, "Assets.car"));
  let metadata = { CFBundleIconName: "Huterm", CFBundleIconFile: "Huterm.icns" };
  const run = () => JSON.stringify(metadata);
  verifyBundleIcons(bundle, root, run);
  metadata = { ...metadata, CFBundleIconName: "OldIcon" };
  expect(() => verifyBundleIcons(bundle, root, run)).toThrow("icon metadata must select Huterm");
  metadata.CFBundleIconName = "Huterm";
  writeFileSync(join(resources, "Assets.car"), "stale catalog");
  expect(() => verifyBundleIcons(bundle, root, run)).toThrow("packaged Assets.car differs");
});
