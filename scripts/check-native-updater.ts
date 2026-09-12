import { chmod, copyFile, mkdir, mkdtemp, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { checkSmokeProcess, runSmokeProcess } from "./smoke-process.ts";

const repoRoot = resolve(import.meta.dir, "..");
const fixturePublicKey = "6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw=";
const packagedMarkers = ["controller-started", "explicit-false-preference", "packaged-framework", "can-check", "application-command"];
const unpackagedMarkers = ["unpackaged-diagnostic"];

export function checkNativeUpdaterRun(exitCode: number, output: string, unpackaged = false): void {
  const prefix = "NATIVE_UPDATER_SMOKE ";
  const actual = output.split(/\r?\n/)
    .filter(line => line.startsWith(prefix))
    .map(line => line.slice(prefix.length));
  const expected = unpackaged ? unpackagedMarkers : packagedMarkers;
  if (exitCode !== 0 || JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`native updater smoke failed: exit=${exitCode}, markers=${JSON.stringify(actual)}`);
  }
}

async function run(executable: string, env: Record<string, string>, unpackaged: boolean): Promise<void> {
  const result = await runSmokeProcess([executable], {
    env: { ...process.env, ...env },
    timeoutMs: 30_000,
  });
  checkSmokeProcess(result, "native updater smoke");
  checkNativeUpdaterRun(result.exitCode, result.stdout, unpackaged);
}

export function updaterFixturePlist(): string {
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key><string>Huterm Updater Smoke</string>
  <key>CFBundleExecutable</key><string>native_updater_smoke</string>
  <key>CFBundleIdentifier</key><string>app.huterm.dev</string>
  <key>CFBundleName</key><string>Huterm Updater Smoke</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.0.1</string>
  <key>CFBundleVersion</key><string>0.0.1</string>
  <key>LSMinimumSystemVersion</key><string>10.15.7</string>
  <key>SUFeedURL</key><string>https://updates.huterm.invalid/appcast.xml</string>
  <key>SUPublicEDKey</key><string>${fixturePublicKey}</string>
  <key>SURequireSignedFeed</key><true/>
  <key>SUVerifyUpdateBeforeExtraction</key><true/>
</dict>
</plist>
`;
}

async function assembleBundle(executable: string, directory: string): Promise<{ bundleExecutable: string; framework: string }> {
  const bundle = join(directory, "Huterm Updater Smoke.app");
  const contents = join(bundle, "Contents");
  const macos = join(contents, "MacOS");
  const frameworks = join(contents, "Frameworks");
  await Promise.all([mkdir(macos, { recursive: true }), mkdir(frameworks, { recursive: true })]);
  const bundleExecutable = join(macos, "native_updater_smoke");
  await copyFile(executable, bundleExecutable);
  await chmod(bundleExecutable, 0o755);
  await writeFile(join(contents, "Info.plist"), updaterFixturePlist());
  const framework = join(frameworks, "Sparkle.framework");
  const copy = Bun.spawnSync(["ditto", join(repoRoot, ".native/sparkle/distribution/Sparkle.framework"), framework]);
  if (copy.exitCode !== 0) throw new Error(`ditto failed: ${copy.stderr}`);
  await Promise.all([
    rm(join(framework, "XPCServices"), { force: true }),
    rm(join(framework, "Versions/B/XPCServices"), { force: true, recursive: true }),
  ]);
  return {
    bundleExecutable: await realpath(bundleExecutable),
    framework: await realpath(framework),
  };
}

async function main(): Promise<void> {
  if (process.platform !== "darwin") throw new Error("native updater smoke requires macOS");
  const executable = resolve(repoRoot, Bun.argv[2] ?? "target/debug/examples/native_updater_smoke");
  const linkage = Bun.spawnSync(["otool", "-L", executable]);
  if (linkage.exitCode !== 0) throw new Error(`otool failed: ${linkage.stderr}`);
  const linkageText = linkage.stdout.toString();
  if (!linkageText.includes("@rpath/Sparkle.framework/Versions/B/Sparkle")) {
    throw new Error("updater smoke executable does not link the packaged Sparkle framework");
  }
  if (linkageText.includes(".native/sparkle")) {
    throw new Error("updater smoke executable contains a source-tree Sparkle path");
  }

  const directory = await mkdtemp(join(tmpdir(), "huterm-updater-smoke-"));
  try {
    const config = join(directory, "config.toml");
    await writeFile(config, "[updates]\nautomatic_checks = false\n");
    const fixture = updaterFixturePlist();
    if (fixture.includes("github.com/jimeh/huterm")) {
      throw new Error("updater smoke fixture must not use the production feed");
    }
    const { bundleExecutable, framework } = await assembleBundle(executable, directory);
    await run(bundleExecutable, {
      CFFIXED_USER_HOME: directory,
      HUTERM_CONFIG_FILE: config,
      HUTERM_UPDATER_SMOKE_FRAMEWORK: framework,
      SHELL: "/bin/sh",
    }, false);
    await run(executable, {
      CFFIXED_USER_HOME: directory,
      HUTERM_CONFIG_FILE: config,
      HUTERM_UPDATER_SMOKE_UNPACKAGED: "1",
      SHELL: "/bin/sh",
    }, true);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  try {
    await main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
