/** Generate reviewed icon assets on macOS; verify them without Apple tools on any host. */
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

const repository = resolve(import.meta.dir, "..");
const source = "assets/Huterm.icon";
const manifestFile = "assets/icons.json";
export const iconFiles = ["assets/Huterm.icns", "assets/Huterm.png", "assets/macos/Assets.car"] as const;
type Digests = Record<string, string>;
type Run = (args: string[]) => string;

function runCommand(args: string[]): string {
  const result = Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0) {
    throw new Error(`${args[0]} exited with status ${result.exitCode}: ${result.stderr.toString().trim()} ${result.stdout.toString().trim()}`);
  }
  if (result.stderr.length) process.stderr.write(result.stderr);
  return result.stdout.toString().trim();
}

function hashes(root: string, files: readonly string[]): Digests {
  return Object.fromEntries([...files].sort().map(file => [file, createHash("sha256").update(readFileSync(join(root, file))).digest("hex")]));
}

function inputs(root: string): Digests {
  const files = ["scripts/icons.ts"];
  function visit(directory: string): void {
    for (const entry of readdirSync(join(root, directory), { withFileTypes: true })) {
      if (entry.name === ".DS_Store") continue;
      const file = `${directory}/${entry.name}`;
      if (entry.isDirectory()) visit(file);
      else if (entry.isFile()) files.push(file);
      else throw new Error(`unexpected icon source entry: ${file}`);
    }
  }
  visit(source);
  if (!files.includes(`${source}/icon.json`)) throw new Error(`missing ${source}/icon.json`);
  return hashes(root, files);
}

function matchingDigests(actual: Digests, expected: unknown, label: string): void {
  if (!expected || typeof expected !== "object" || Array.isArray(expected)) throw new Error(`invalid icon ${label} hashes`);
  const recorded = expected as Record<string, unknown>;
  for (const file of new Set([...Object.keys(actual), ...Object.keys(recorded)])) {
    if (actual[file] !== recorded[file]) throw new Error(`icon ${label} changed: ${file}; run mise run icons:generate on macOS and include all generated assets`);
  }
}

function validateXcodeVersion(version: unknown): void {
  const match = typeof version === "string" && /^Xcode (\d+)(?:\.\d+)*(?:\r?\n|$)/.exec(version);
  if (!match || match[1] !== "27") throw new Error(`icon assets require Xcode 27; found ${String(version)}`);
}

export function validateIconMetadata(value: unknown): void {
  const plist = value as Record<string, unknown> | null;
  if (!plist || plist.CFBundleIconName !== "Huterm" || !["Huterm", "Huterm.icns"].includes(String(plist.CFBundleIconFile))) {
    throw new Error("icon metadata must select Huterm in both CFBundleIconName and CFBundleIconFile");
  }
}

export function validateIconFiles(root: string): void {
  const icns = readFileSync(join(root, iconFiles[0]));
  if (icns.length <= 8 || icns.toString("ascii", 0, 4) !== "icns" || icns.readUInt32BE(4) !== icns.length) {
    throw new Error("invalid generated Huterm.icns");
  }
  const png = readFileSync(join(root, iconFiles[1]));
  if (png.length < 24 || png.subarray(0, 8).toString("hex") !== "89504e470d0a1a0a" || png.toString("ascii", 12, 16) !== "IHDR" || png.readUInt32BE(16) !== 1024 || png.readUInt32BE(20) !== 1024) {
    throw new Error("generated Huterm.png must be a 1024x1024 PNG");
  }
  const catalog = readFileSync(join(root, iconFiles[2]));
  if (catalog.length <= 8 || catalog.toString("ascii", 0, 8) !== "BOMStore") throw new Error("invalid generated Assets.car");
}

export function checkIcons(root = repository): void {
  const manifest = JSON.parse(readFileSync(join(root, manifestFile), "utf8"));
  if (manifest.version !== 1) throw new Error("unsupported icon manifest version");
  validateXcodeVersion(manifest.tools?.xcode);
  matchingDigests(inputs(root), manifest.inputs, "source");
  matchingDigests(hashes(root, iconFiles), manifest.outputs, "output");
  validateIconFiles(root);
}

export function generateIcons(root = repository, run: Run = runCommand): void {
  const before = inputs(root);
  const xcode = run(["xcodebuild", "-version"]);
  validateXcodeVersion(xcode);
  const actool = run(["xcrun", "--find", "actool"]);
  // xcrun's ictool is an asset-catalog entry point, not Icon Composer's renderer.
  const ictool = resolve(dirname(actool), "../../../Applications/Icon Composer.app/Contents/Executables/ictool");
  const tools = {
    xcode,
    iconComposer: JSON.parse(run([ictool, "--version"])),
    macOS: run(["sw_vers", "-productVersion"]),
  };
  const stage = mkdtempSync(join(tmpdir(), "huterm-icons-"));
  try {
    const compiled = join(stage, "compiled");
    mkdirSync(compiled);
    const plist = join(compiled, "Info.plist");
    run(["xcrun", "actool", join(root, source), "--compile", compiled,
      "--platform", "macosx", "--target-device", "mac", "--app-icon", "Huterm",
      // Keep flattened renditions for older macOS. This is only the asset deployment target.
      "--minimum-deployment-target", "10.13", "--output-partial-info-plist", plist,
      "--output-format", "human-readable-text", "--errors", "--warnings", "--notices"]);
    // actool can exit zero after an icon export fails. Require fresh outputs from this run.
    const metadata = JSON.parse(run(["plutil", "-convert", "json", "-o", "-", plist]));
    validateIconMetadata(metadata);
    if (Object.keys(metadata).sort().join(",") !== "CFBundleIconFile,CFBundleIconName") {
      throw new Error("actool emitted additional icon metadata; update the bundle integration before generating assets");
    }
    const catalog = JSON.parse(run(["xcrun", "assetutil", "--info", join(compiled, "Assets.car")]));
    if (!Array.isArray(catalog) || !catalog.some(item => item.Name === "Huterm" && item.AssetType === "IconImageStack")) {
      throw new Error("compiled Assets.car is missing the layered Huterm icon");
    }
    mkdirSync(join(stage, "assets/macos"), { recursive: true });
    copyFileSync(join(compiled, "Huterm.icns"), join(stage, iconFiles[0]));
    copyFileSync(join(compiled, "Assets.car"), join(stage, iconFiles[2]));
    run([ictool, join(root, source), "--export-image", "--output-file", join(stage, iconFiles[1]),
      "--platform", "macOS", "--rendition", "Default", "--width", "1024", "--height", "1024", "--scale", "1",
      "--design-generation", "27"]);
    validateIconFiles(stage);
    matchingDigests(inputs(root), before, "source during generation");
    const manifest = { version: 1, tools, inputs: before, outputs: hashes(stage, iconFiles) };
    for (const file of iconFiles) copyFileSync(join(stage, file), join(root, file));
    // Write the manifest last, so interrupted generation cannot pass verification.
    writeFileSync(join(root, manifestFile), `${JSON.stringify(manifest, null, 2)}\n`);
  } finally {
    rmSync(stage, { recursive: true, force: true });
  }
  checkIcons(root);
}

export function verifyBundleIcons(bundle: string, root = repository, run: Run = runCommand): void {
  checkIcons(root);
  validateIconMetadata(JSON.parse(run(["plutil", "-convert", "json", "-o", "-", join(bundle, "Contents/Info.plist")])));
  for (const [file, name] of [[iconFiles[0], "Huterm.icns"], [iconFiles[2], "Assets.car"]]) {
    if (!readFileSync(join(root, file!)).equals(readFileSync(join(bundle, "Contents/Resources", name!)))) {
      throw new Error(`packaged ${name} differs from the verified icon asset`);
    }
  }
}

if (import.meta.main) {
  try {
    const [mode, bundle, ...rest] = Bun.argv.slice(2);
    if (mode === "generate" && bundle === undefined) {
      if (process.platform !== "darwin") throw new Error("icon generation requires macOS and Xcode 27; normal builds use the committed assets");
      generateIcons();
      console.log("Generated Huterm.icns, Huterm.png, Assets.car, and icons.json");
    } else if (mode === "check" && bundle === undefined) {
      checkIcons();
      console.log("Verified Huterm icon source and generated assets");
    } else if (mode === "verify-bundle" && bundle && rest.length === 0) {
      verifyBundleIcons(resolve(bundle));
      console.log("Verified packaged Huterm icon metadata and resources");
    } else throw new Error("expected generate, check, or verify-bundle <Huterm.app>");
  } catch (error) {
    console.error(`Icons failed: ${error instanceof Error ? error.message : error}`);
    process.exitCode = 1;
  }
}
