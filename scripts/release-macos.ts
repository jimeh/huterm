import { chmod, copyFile, lstat, mkdir, mkdtemp, readFile, readdir, readlink, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import {
  generateAppcast,
  generateFixtureAppcast,
  generateSbom,
  validateAppcast,
  validateRuntimeSpdx,
} from "./release-artifacts.ts";
import {
  releaseAssetNames,
  validateBuildInputs,
  validateReleaseInputs,
  verifyLocalAssets,
  writePlatformManifest,
  type BuildInputs,
  type ReleaseInputs,
} from "./release.ts";

const repoRoot = resolve(import.meta.dir, "..");
const appPath = join(repoRoot, "target/release/bundle/Huterm.app");
const entitlementPath = join(repoRoot, "assets/macos/Huterm.entitlements");
const sparklePublicKeyPath = join(repoRoot, "assets/macos/SparklePublicKey");

export const sparkleFeedUrl = "https://github.com/jimeh/huterm/releases/latest/download/appcast.xml";
export const minimumMacosVersion = "10.15.7";
const sparkleFrameworkRelative = "Contents/Frameworks/Sparkle.framework";
const sparkleVersionRelative = `${sparkleFrameworkRelative}/Versions/B`;

export interface SigningTarget {
  path: string;
  entitlements: "huterm" | "none";
}

export function signingPlan(bundlePath: string): SigningTarget[] {
  return [
    { path: join(bundlePath, `${sparkleVersionRelative}/Autoupdate`), entitlements: "none" },
    { path: join(bundlePath, `${sparkleVersionRelative}/Updater.app/Contents/MacOS/Updater`), entitlements: "none" },
    { path: join(bundlePath, `${sparkleVersionRelative}/Updater.app`), entitlements: "none" },
    { path: join(bundlePath, `${sparkleVersionRelative}/Sparkle`), entitlements: "none" },
    { path: join(bundlePath, sparkleFrameworkRelative), entitlements: "none" },
    { path: join(bundlePath, "Contents/MacOS/huterm"), entitlements: "huterm" },
    { path: bundlePath, entitlements: "huterm" },
  ];
}

export const releaseEntitlements = [
  "com.apple.security.automation.apple-events",
  "com.apple.security.device.audio-input",
  "com.apple.security.device.camera",
  "com.apple.security.personal-information.addressbook",
  "com.apple.security.personal-information.calendars",
  "com.apple.security.personal-information.location",
  "com.apple.security.personal-information.photos-library",
] as const;

const hostedRequest = "Huterm or a program running within it would like to";
export const privacyUsageDescriptions = {
  NSAppBundlesUsageDescription: `${hostedRequest} access files inside other applications.`,
  NSAppDataUsageDescription: `${hostedRequest} access files in other applications' data containers.`,
  NSAppleEventsUsageDescription: `${hostedRequest} control other applications using Apple events.`,
  NSAppleMusicUsageDescription: `${hostedRequest} access your media library.`,
  NSAudioCaptureUsageDescription: `${hostedRequest} capture system audio.`,
  NSBluetoothAlwaysUsageDescription: `${hostedRequest} use Bluetooth.`,
  NSCalendarsFullAccessUsageDescription: `${hostedRequest} read and modify your calendars.`,
  NSCalendarsUsageDescription: `${hostedRequest} access your calendars.`,
  NSCalendarsWriteOnlyAccessUsageDescription: `${hostedRequest} add events to your calendars.`,
  NSCameraUsageDescription: `${hostedRequest} use the camera.`,
  NSContactsUsageDescription: `${hostedRequest} access your contacts.`,
  NSDesktopFolderUsageDescription: `${hostedRequest} access files in your Desktop folder.`,
  NSDocumentsFolderUsageDescription: `${hostedRequest} access files in your Documents folder.`,
  NSDownloadsFolderUsageDescription: `${hostedRequest} access files in your Downloads folder.`,
  NSFileProviderDomainUsageDescription: `${hostedRequest} access files managed by file providers.`,
  NSLocalNetworkUsageDescription: `${hostedRequest} access devices on your local network.`,
  NSLocationAlwaysAndWhenInUseUsageDescription: `${hostedRequest} access your location when Huterm is not active.`,
  NSLocationUsageDescription: `${hostedRequest} access your location.`,
  NSLocationWhenInUseUsageDescription: `${hostedRequest} access your location while Huterm is in use.`,
  NSMicrophoneUsageDescription: `${hostedRequest} use your microphone.`,
  NSMotionUsageDescription: `${hostedRequest} access motion data.`,
  NSNetworkVolumesUsageDescription: `${hostedRequest} access files on network volumes.`,
  NSPhotoLibraryAddUsageDescription: `${hostedRequest} add items to your photo library.`,
  NSPhotoLibraryUsageDescription: `${hostedRequest} access your photo library.`,
  NSRemindersFullAccessUsageDescription: `${hostedRequest} read and modify your reminders.`,
  NSRemindersUsageDescription: `${hostedRequest} access your reminders.`,
  NSRemovableVolumesUsageDescription: `${hostedRequest} access files on removable volumes.`,
  NSSpeechRecognitionUsageDescription: `${hostedRequest} use speech recognition.`,
  NSSystemAdministrationUsageDescription: `${hostedRequest} modify system configuration.`,
} as const;

type JsonObject = Record<string, unknown>;

interface KeychainState {
  defaultKeychain?: string;
  searchList: string[];
}

interface CommandResult {
  stdout: string;
  stderr: string;
}

export interface MacReleasePipeline {
  signAndVerify(): Promise<void>;
  createNotarizationArchive(): Promise<void>;
  submitNotarization(): Promise<void>;
  staple(): Promise<void>;
  validateStaple(): Promise<void>;
  verifyStapledSignatures(): Promise<void>;
  assessGatekeeper(): Promise<void>;
  createFinalArchive(): Promise<void>;
}

export async function runMacReleasePipeline(pipeline: MacReleasePipeline): Promise<void> {
  await pipeline.signAndVerify();
  await pipeline.createNotarizationArchive();
  await pipeline.submitNotarization();
  await pipeline.staple();
  await pipeline.validateStaple();
  await pipeline.verifyStapledSignatures();
  await pipeline.assessGatekeeper();
  await pipeline.createFinalArchive();
}

function objectValue(value: unknown, label: string): JsonObject {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${label} must be an object`);
  return value as JsonObject;
}

function requiredEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function equalKeys(actual: JsonObject, expected: JsonObject, label: string): void {
  const actualKeys = Object.keys(actual).sort();
  const expectedKeys = Object.keys(expected).sort();
  if (actualKeys.join("\n") !== expectedKeys.join("\n")) {
    throw new Error(`${label} keys do not match: got [${actualKeys.join(", ")}]`);
  }
}

export function validatePrivacyDescriptions(value: unknown): void {
  const plist = objectValue(value, "Info.plist");
  const actual: JsonObject = {};
  for (const [key, description] of Object.entries(plist)) {
    if (key.endsWith("UsageDescription")) actual[key] = description;
  }
  equalKeys(actual, privacyUsageDescriptions, "privacy usage description");
  for (const [key, expected] of Object.entries(privacyUsageDescriptions)) {
    if (actual[key] !== expected) throw new Error(`${key} does not match the approved privacy description`);
  }
}

export function validateEntitlements(value: unknown): void {
  const actual = objectValue(value, "entitlements");
  const expected = Object.fromEntries(releaseEntitlements.map(key => [key, true]));
  equalKeys(actual, expected, "entitlement");
  for (const key of releaseEntitlements) {
    if (actual[key] !== true) throw new Error(`${key} entitlement must be true`);
  }
}

function decodeXml(value: string): string {
  return value
    .replaceAll("&apos;", "'")
    .replaceAll("&quot;", '"')
    .replaceAll("&gt;", ">")
    .replaceAll("&lt;", "<")
    .replaceAll("&amp;", "&");
}

export function parseSimplePlist(xml: string): JsonObject {
  const result: JsonObject = {};
  const itemPattern = /<key>([\s\S]*?)<\/key>\s*(?:<string>([\s\S]*?)<\/string>|<(true|false)\s*\/>)/g;
  for (const match of xml.matchAll(itemPattern)) {
    const key = decodeXml(match[1]!.trim());
    result[key] = match[3] ? match[3] === "true" : decodeXml(match[2]!.trim());
  }
  if (Object.keys(result).length === 0) throw new Error("plist contains no supported values");
  return result;
}

export function validateSignatureDetails(details: string, teamId: string, label: string): void {
  if (/^Signature=adhoc$/m.test(details)) throw new Error(`${label} is ad hoc signed`);
  if (!/^Authority=Developer ID Application:/m.test(details)) throw new Error(`${label} lacks a Developer ID Application authority`);
  if (!new RegExp(`^TeamIdentifier=${teamId}$`, "m").test(details)) throw new Error(`${label} is not signed by team ${teamId}`);
  if (!/^CodeDirectory .* flags=0x[0-9a-f]+\([^)]*runtime[^)]*\)/mi.test(details)) throw new Error(`${label} lacks the hardened runtime flag`);
  const timestamp = /^Timestamp=(.+)$/m.exec(details)?.[1]?.trim();
  if (!timestamp || timestamp === "none") throw new Error(`${label} lacks a secure signing timestamp`);
}

export function parseDeveloperIdentity(output: string, teamId: string): string {
  const matches = output.split(/\r?\n/).filter(line => line.includes("Developer ID Application") && line.includes(`(${teamId})`));
  if (matches.length !== 1) throw new Error(`expected one Developer ID Application identity for team ${teamId}, found ${matches.length}`);
  const identity = /\b([0-9A-F]{40})\b/.exec(matches[0]!)?.[1];
  if (!identity) throw new Error(`Developer ID Application identity for team ${teamId} has no certificate hash`);
  return identity;
}

function mergedEnvironment(additions: Record<string, string> = {}): Record<string, string> {
  const environment: Record<string, string> = { ...additions };
  for (const [key, value] of Object.entries(process.env)) {
    if (value !== undefined && environment[key] === undefined) environment[key] = value;
  }
  return environment;
}

async function runCaptured(command: string, args: string[], options: { allowFailure?: boolean; cwd?: string; env?: Record<string, string> } = {}): Promise<CommandResult> {
  const processHandle = Bun.spawn([command, ...args], {
    cwd: options.cwd ?? repoRoot,
    env: mergedEnvironment(options.env),
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(processHandle.stdout).text(),
    new Response(processHandle.stderr).text(),
    processHandle.exited,
  ]);
  if (exitCode !== 0 && !options.allowFailure) {
    if (stdout) process.stdout.write(stdout);
    if (stderr) process.stderr.write(stderr);
    throw new Error(`${command} exited with status ${exitCode}`);
  }
  return { stdout, stderr };
}

async function runInherited(command: string, args: string[], env?: Record<string, string>): Promise<void> {
  const processHandle = Bun.spawn([command, ...args], {
    cwd: repoRoot,
    env: mergedEnvironment(env),
    stdin: "ignore",
    stdout: "inherit",
    stderr: "inherit",
  });
  const exitCode = await processHandle.exited;
  if (exitCode !== 0) throw new Error(`${command} exited with status ${exitCode}`);
}

async function readPlist(plistPath: string): Promise<JsonObject> {
  const { stdout } = await runCaptured("plutil", ["-convert", "json", "-o", "-", plistPath]);
  return objectValue(JSON.parse(stdout), plistPath);
}

async function expectedSparklePublicKey(): Promise<string> {
  let value: string;
  try {
    value = (await readFile(process.env.SPARKLE_PUBLIC_KEY_FILE ?? sparklePublicKeyPath, "utf8")).trim();
  } catch (error) {
    if (error instanceof Error && "code" in error && error.code === "ENOENT") {
      throw new Error(
        "the production Sparkle public key is missing; add assets/macos/SparklePublicKey before enabling the production feed",
      );
    }
    throw error;
  }
  if (!/^[A-Za-z0-9+/]{43}=$/.test(value)) {
    throw new Error("the Sparkle public key must be a canonical 32-byte base64 EdDSA key");
  }
  return value;
}

type ArchiveExtractor = (archive: string, destination: string) => Promise<void>;
type PlistReader = (plistPath: string) => Promise<JsonObject>;

async function extractZip(archive: string, destination: string): Promise<void> {
  await runInherited("ditto", ["-x", "-k", archive, destination]);
}

export async function sparklePublicKeyFromArchive(
  archive: string,
  expectedPublicKey: string,
  extract: ArchiveExtractor = extractZip,
  plistReader: PlistReader = readPlist,
): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), "huterm-key-verification-"));
  try {
    await extract(archive, directory);
    const entries = await readdir(directory);
    if (entries.length !== 1 || entries[0] !== "Huterm.app") {
      throw new Error(`candidate archive must contain only Huterm.app, found: ${entries.join(", ")}`);
    }
    const bundle = join(directory, "Huterm.app");
    const bundleDetails = await lstat(bundle);
    if (!bundleDetails.isDirectory() || bundleDetails.isSymbolicLink()) {
      throw new Error("candidate archive Huterm.app is not a directory");
    }
    const info = await plistReader(join(bundle, "Contents/Info.plist"));
    const embedded = info.SUPublicEDKey;
    if (typeof embedded !== "string" || !/^[A-Za-z0-9+/]{43}=$/.test(embedded)) {
      throw new Error("candidate archive embeds a malformed Sparkle public key");
    }
    if (embedded !== expectedPublicKey) {
      throw new Error("candidate archive Sparkle public key does not match the committed canonical key");
    }
    return embedded;
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

export function validateUpdatePlist(value: unknown, expectedVersion: string, publicKey: string): void {
  const plist = objectValue(value, "Info.plist");
  validatePackagePlist(plist, expectedVersion);
  if (plist.SUFeedURL !== sparkleFeedUrl) throw new Error("SUFeedURL does not match the production feed");
  if (plist.SUPublicEDKey !== publicKey) throw new Error("SUPublicEDKey does not match the pinned production key");
  if (plist.SUVerifyUpdateBeforeExtraction !== true) throw new Error("SUVerifyUpdateBeforeExtraction must be true");
  if (plist.SURequireSignedFeed !== true) throw new Error("SURequireSignedFeed must be true");
  for (const forbidden of ["SUAutomaticallyUpdate", "SUEnableAutomaticChecks", "SUScheduledCheckInterval"]) {
    if (forbidden in plist) throw new Error(`${forbidden} must remain unset`);
  }
}

function validatePackagePlist(plist: JsonObject, expectedVersion: string): void {
  if (plist.CFBundleVersion !== expectedVersion) {
    throw new Error(`CFBundleVersion ${String(plist.CFBundleVersion)} does not match ${expectedVersion}`);
  }
  if (plist.CFBundleShortVersionString !== expectedVersion) {
    throw new Error(
      `CFBundleShortVersionString ${String(plist.CFBundleShortVersionString)} does not match ${expectedVersion}`,
    );
  }
  if (plist.LSMinimumSystemVersion !== minimumMacosVersion) {
    throw new Error(
      `LSMinimumSystemVersion ${String(plist.LSMinimumSystemVersion)} does not match ${minimumMacosVersion}`,
    );
  }
}

const sparklePlistKeys = [
  "SUFeedURL",
  "SUPublicEDKey",
  "SURequireSignedFeed",
  "SUVerifyUpdateBeforeExtraction",
  "SUAutomaticallyUpdate",
  "SUEnableAutomaticChecks",
  "SUScheduledCheckInterval",
] as const;

export function updaterPlistValues(publicKey: string): JsonObject {
  return {
    SUFeedURL: sparkleFeedUrl,
    SUPublicEDKey: publicKey,
    SURequireSignedFeed: true,
    SUVerifyUpdateBeforeExtraction: true,
  };
}

export function validateLocalPackagePlist(value: unknown, expectedVersion: string): void {
  const plist = objectValue(value, "Info.plist");
  validatePackagePlist(plist, expectedVersion);
  for (const key of sparklePlistKeys) {
    if (key in plist) throw new Error(`${key} must remain absent from Sparkle-free packages`);
  }
}

async function requireSymlink(filePath: string, expectedTarget: string): Promise<void> {
  const details = await lstat(filePath);
  if (!details.isSymbolicLink()) throw new Error(`${filePath} must remain a symbolic link`);
  const target = await readlink(filePath);
  if (target !== expectedTarget) throw new Error(`${filePath} points at ${target}, expected ${expectedTarget}`);
}

async function assertAbsent(filePath: string, label: string): Promise<void> {
  try {
    await lstat(filePath);
  } catch (error) {
    if (error instanceof Error && "code" in error && error.code === "ENOENT") return;
    throw error;
  }
  throw new Error(`${label} must not remain in the packaged framework`);
}

async function verifyUniversalBinary(binary: string): Promise<void> {
  await runCaptured("lipo", [binary, "-verify_arch", "arm64"]);
  await runCaptured("lipo", [binary, "-verify_arch", "x86_64"]);
}

async function verifySparkleFramework(bundlePath: string): Promise<void> {
  const framework = join(bundlePath, sparkleFrameworkRelative);
  await requireSymlink(join(framework, "Versions/Current"), "B");
  await requireSymlink(join(framework, "Sparkle"), "Versions/Current/Sparkle");
  await requireSymlink(join(framework, "Autoupdate"), "Versions/Current/Autoupdate");
  await requireSymlink(join(framework, "Updater.app"), "Versions/Current/Updater.app");
  await requireSymlink(join(framework, "Resources"), "Versions/Current/Resources");
  await assertAbsent(join(framework, "XPCServices"), "Sparkle XPCServices symlink");
  await assertAbsent(join(framework, "Versions/B/XPCServices"), "Sparkle XPCServices directory");
  const license = await readFile(join(bundlePath, "Contents/Resources/Sparkle-LICENSE"));
  const reviewedLicense = await readFile(join(repoRoot, "third-party/sparkle/LICENSE"));
  if (!license.equals(reviewedLicense)) throw new Error("packaged Sparkle license does not match the reviewed notice");
  for (const binary of [
    join(framework, "Versions/B/Sparkle"),
    join(framework, "Versions/B/Autoupdate"),
    join(framework, "Versions/B/Updater.app/Contents/MacOS/Updater"),
  ]) {
    await verifyUniversalBinary(binary);
  }
}

async function prepareUpdaterPackage(bundlePath: string): Promise<void> {
  const publicKey = await expectedSparklePublicKey();
  const framework = join(bundlePath, sparkleFrameworkRelative);
  const sourceFramework = join(repoRoot, ".native/sparkle/distribution/Sparkle.framework");
  const resources = join(bundlePath, "Contents/Resources");
  await rm(framework, { force: true, recursive: true });
  await mkdir(dirname(framework), { recursive: true });
  await mkdir(resources, { recursive: true });
  await runInherited("ditto", [sourceFramework, framework]);
  await copyFile(join(repoRoot, "third-party/sparkle/LICENSE"), join(resources, "Sparkle-LICENSE"));
  await rm(join(framework, "XPCServices"), { force: true });
  await rm(join(framework, "Versions/B/XPCServices"), { force: true, recursive: true });
  const plist = join(bundlePath, "Contents/Info.plist");
  for (const [key, value] of Object.entries(updaterPlistValues(publicKey))) {
    const type = typeof value === "boolean" ? "bool" : "string";
    await runInherited("plutil", ["-insert", key, `-${type}`, String(value), plist]);
  }
}

async function verifyPackageConfiguration(bundlePath: string, expectedVersion?: string): Promise<void> {
  const info = await readPlist(join(bundlePath, "Contents/Info.plist"));
  validatePrivacyDescriptions(info);
  validateEntitlements(await readPlist(entitlementPath));
  const version = expectedVersion ?? String(info.CFBundleShortVersionString);
  validateUpdatePlist(info, version, await expectedSparklePublicKey());
  await verifySparkleFramework(bundlePath);
  const linkage = await runCaptured("otool", ["-L", join(bundlePath, "Contents/MacOS/huterm")]);
  const dependencies = linkage.stdout.split(/\r?\n/).filter(line => /^\s+/.test(line)).join("\n");
  if (dependencies.includes("libghostty")) throw new Error("package verification failed: Ghostty must be statically linked");
  if (!dependencies.includes("@rpath/Sparkle.framework/Versions/B/Sparkle")) {
    throw new Error("package verification failed: Huterm does not resolve the packaged Sparkle framework");
  }
  if (dependencies.includes(".native/sparkle") || dependencies.includes(repoRoot)) {
    throw new Error("package verification failed: Sparkle linkage contains a build-machine path");
  }
}

async function verifyLocalPackageConfiguration(bundlePath: string, expectedVersion?: string): Promise<void> {
  const info = await readPlist(join(bundlePath, "Contents/Info.plist"));
  validatePrivacyDescriptions(info);
  validateEntitlements(await readPlist(entitlementPath));
  const version = expectedVersion ?? String(info.CFBundleShortVersionString);
  validateLocalPackagePlist(info, version);
  await assertAbsent(join(bundlePath, sparkleFrameworkRelative), "Sparkle framework");
  await assertAbsent(join(bundlePath, "Contents/Resources/Sparkle-LICENSE"), "Sparkle license");
  const linkage = await runCaptured("otool", ["-L", join(bundlePath, "Contents/MacOS/huterm")]);
  if (linkage.stdout.includes("libghostty")) throw new Error("package verification failed: Ghostty must be statically linked");
  if (linkage.stdout.includes("Sparkle.framework")) {
    throw new Error("package verification failed: Sparkle-free Huterm links Sparkle");
  }
}

function currentBuildInputs(): BuildInputs {
  return validateBuildInputs(requiredEnv("RELEASE_SHA"), requiredEnv("RELEASE_VERSION"));
}

function strictBase64(value: string, name: string): Buffer {
  const compact = value.replace(/\s/g, "");
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(compact) || compact.length % 4 !== 0) throw new Error(`${name} is not valid base64`);
  const decoded = Buffer.from(compact, "base64");
  if (decoded.length === 0 || decoded.toString("base64") !== compact) throw new Error(`${name} is not canonical base64`);
  return decoded;
}

function keychainPaths(): { keychain: string; state: string; tempRoot: string } {
  const runnerTemp = process.env.RUNNER_TEMP;
  const tempRoot = process.env.RELEASE_TEMP_DIR ?? (runnerTemp ? join(runnerTemp, "huterm-release") : join(tmpdir(), `huterm-release-${process.pid}`));
  return {
    keychain: process.env.RELEASE_KEYCHAIN_PATH ?? join(runnerTemp ?? tempRoot, "huterm-release.keychain-db"),
    state: join(tempRoot, "keychain-state.json"),
    tempRoot,
  };
}

function parseKeychainList(output: string): string[] {
  return [...output.matchAll(/"([^"]+)"/g)].map(match => match[1]!);
}

async function prepareKeychain(teamId: string, p12: Buffer, password: string, notaryKey: Buffer): Promise<{ identity: string; notaryKeyPath: string }> {
  const paths = keychainPaths();
  await rm(paths.tempRoot, { force: true, recursive: true });
  await mkdir(paths.tempRoot, { recursive: true });
  const defaultKeychain = parseKeychainList((await runCaptured("security", ["default-keychain", "-d", "user"])).stdout)[0];
  const searchList = parseKeychainList((await runCaptured("security", ["list-keychains", "-d", "user"])).stdout);
  await writeFile(paths.state, JSON.stringify({ defaultKeychain, searchList } satisfies KeychainState), { mode: 0o600 });

  const p12Path = join(paths.tempRoot, "developer-id.p12");
  const notaryKeyPath = join(paths.tempRoot, "notary-key.p8");
  await writeFile(p12Path, p12, { mode: 0o600 });
  await writeFile(notaryKeyPath, notaryKey, { mode: 0o600 });
  await chmod(p12Path, 0o600);
  await chmod(notaryKeyPath, 0o600);

  const keychainPassword = `huterm-${crypto.randomUUID()}`;
  await runCaptured("security", ["create-keychain", "-p", keychainPassword, paths.keychain]);
  await runCaptured("security", ["set-keychain-settings", "-lut", "21600", paths.keychain]);
  await runCaptured("security", ["unlock-keychain", "-p", keychainPassword, paths.keychain]);
  await runCaptured("security", ["default-keychain", "-d", "user", "-s", paths.keychain]);
  await runCaptured("security", ["list-keychains", "-d", "user", "-s", paths.keychain]);
  await runCaptured("security", ["import", p12Path, "-k", paths.keychain, "-P", password, "-T", "/usr/bin/codesign"]);
  await runCaptured("security", ["set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", keychainPassword, paths.keychain]);
  const identities = (await runCaptured("security", ["find-identity", "-v", "-p", "codesigning", paths.keychain])).stdout;
  return { identity: parseDeveloperIdentity(identities, teamId), notaryKeyPath };
}

async function cleanupSigning(): Promise<void> {
  const paths = keychainPaths();
  const errors: unknown[] = [];
  let state: KeychainState | undefined;
  try {
    const value = objectValue(JSON.parse(await readFile(paths.state, "utf8")), "saved keychain state");
    if (value.defaultKeychain !== undefined && typeof value.defaultKeychain !== "string") {
      throw new Error("saved default keychain must be a string");
    }
    if (!Array.isArray(value.searchList) || value.searchList.some(item => typeof item !== "string")) {
      throw new Error("saved keychain search list must contain only strings");
    }
    state = { defaultKeychain: value.defaultKeychain as string | undefined, searchList: value.searchList as string[] };
  } catch (error) {
    if (!(error instanceof Error && "code" in error && error.code === "ENOENT")) errors.push(error);
  }
  if (state?.defaultKeychain) {
    try {
      await runCaptured("security", ["default-keychain", "-d", "user", "-s", state.defaultKeychain]);
    } catch (error) {
      errors.push(error);
    }
  }
  if (state) {
    try {
      await runCaptured("security", ["list-keychains", "-d", "user", "-s", ...state.searchList]);
    } catch (error) {
      errors.push(error);
    }
  }
  let keychainExists = false;
  try {
    await stat(paths.keychain);
    keychainExists = true;
  } catch (error) {
    if (!(error instanceof Error && "code" in error && error.code === "ENOENT")) errors.push(error);
  }
  if (keychainExists) {
    try {
      await runCaptured("security", ["delete-keychain", paths.keychain]);
    } catch (error) {
      errors.push(error);
    }
  }
  if (errors.length === 0) {
    await rm(paths.tempRoot, { force: true, recursive: true });
  } else {
    throw new AggregateError(errors, "failed to fully clean temporary signing state");
  }
}

const machoMagics = new Set([0xfeedface, 0xcefaedfe, 0xfeedfacf, 0xcffaedfe, 0xcafebabe, 0xbebafeca, 0xcafebabf, 0xbfbafeca]);

async function isMachO(filePath: string): Promise<boolean> {
  const bytes = Buffer.from(await Bun.file(filePath).slice(0, 4).arrayBuffer());
  return bytes.length === 4 && machoMagics.has(bytes.readUInt32BE(0));
}

async function findMachOBinaries(root: string): Promise<string[]> {
  const found: string[] = [];
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const entryPath = join(root, entry.name);
    if (entry.isDirectory()) found.push(...await findMachOBinaries(entryPath));
    else if (entry.isFile() && await isMachO(entryPath)) found.push(entryPath);
  }
  return found.sort((left, right) => right.split("/").length - left.split("/").length || left.localeCompare(right));
}

async function codesignTarget(target: string, identity: string, keychain: string, entitlements: boolean): Promise<void> {
  const args = ["--force", "--sign", identity, "--keychain", keychain, "--options", "runtime", "--timestamp"];
  if (entitlements) args.push("--entitlements", entitlementPath);
  args.push(target);
  await runInherited("codesign", args);
}

async function extractedEntitlements(target: string): Promise<JsonObject> {
  const result = await runCaptured("codesign", ["-d", "--entitlements", ":-", target]);
  const combined = `${result.stdout}\n${result.stderr}`;
  const start = combined.indexOf("<?xml");
  const end = combined.indexOf("</plist>", start);
  if (start < 0 && end < 0) return {};
  if (start < 0 || end < 0) throw new Error(`codesign reported malformed entitlements for ${target}`);
  return parseSimplePlist(combined.slice(start, end + "</plist>".length));
}

async function signAndVerifyApp(identity: string, teamId: string): Promise<void> {
  const mainExecutable = join(appPath, "Contents/MacOS/huterm");
  const keychain = keychainPaths().keychain;
  const binaries = await findMachOBinaries(appPath);
  if (!binaries.includes(mainExecutable)) throw new Error("packaged app does not contain the Huterm Mach-O executable");
  const plan = signingPlan(appPath);
  const plannedBinaries = new Set(plan.filter(target => target.path !== appPath && !target.path.endsWith(".app") && !target.path.endsWith(".framework")).map(target => target.path));
  const unplannedBinaries = binaries.filter(binary => !plannedBinaries.has(binary));
  if (unplannedBinaries.length > 0) {
    throw new Error(`packaged app contains unplanned Mach-O files: ${unplannedBinaries.map(file => relative(appPath, file)).join(", ")}`);
  }
  for (const target of plan) {
    await codesignTarget(target.path, identity, keychain, target.entitlements === "huterm");
  }
  await verifySignedApp(teamId);
}

async function verifySignedApp(teamId: string): Promise<void> {
  const mainExecutable = join(appPath, "Contents/MacOS/huterm");
  const binaries = await findMachOBinaries(appPath);
  if (!binaries.includes(mainExecutable)) throw new Error("packaged app does not contain the Huterm Mach-O executable");
  await runInherited("codesign", ["--verify", "--deep", "--strict", "--verbose=4", appPath]);
  const plan = signingPlan(appPath);
  const plannedBinaries = new Set(plan.filter(target => target.path !== appPath && !target.path.endsWith(".app") && !target.path.endsWith(".framework")).map(target => target.path));
  const unplannedBinaries = binaries.filter(binary => !plannedBinaries.has(binary));
  if (unplannedBinaries.length > 0) {
    throw new Error(`packaged app contains unplanned Mach-O files: ${unplannedBinaries.map(file => relative(appPath, file)).join(", ")}`);
  }
  for (const target of plan) {
    await runInherited("codesign", ["--verify", "--strict", "--verbose=2", target.path]);
    const details = await runCaptured("codesign", ["-dvvv", target.path]);
    validateSignatureDetails(`${details.stdout}\n${details.stderr}`, teamId, relative(repoRoot, target.path));
    const entitlements = await extractedEntitlements(target.path);
    if (target.entitlements === "huterm") validateEntitlements(entitlements);
    else if (Object.keys(entitlements).length > 0) {
      throw new Error(`${relative(repoRoot, target.path)} unexpectedly inherits Huterm entitlements`);
    }
  }
  console.log(`verified ${binaries.length} signed Mach-O file${binaries.length === 1 ? "" : "s"}`);
}

async function zipApp(destination: string): Promise<void> {
  await rm(destination, { force: true });
  await mkdir(dirname(destination), { recursive: true });
  await runInherited("ditto", ["-c", "-k", "--sequesterRsrc", "--keepParent", appPath, destination]);
}

async function buildRelease(): Promise<void> {
  if (process.platform !== "darwin") throw new Error("macOS releases require a macOS host");
  const inputs = currentBuildInputs();
  const teamId = requiredEnv("MACOS_TEAM_ID");
  const issuerId = requiredEnv("MACOS_NOTARY_ISSUER_ID");
  const keyId = requiredEnv("MACOS_NOTARY_KEY_ID");
  if (!/^[A-Z0-9]{10}$/.test(teamId)) throw new Error("MACOS_TEAM_ID must be a 10-character Apple team ID");
  if (!/^[A-Z0-9]{10}$/.test(keyId)) throw new Error("MACOS_NOTARY_KEY_ID must be a 10-character App Store Connect key ID");
  if (!/^[0-9a-fA-F-]{36}$/.test(issuerId)) throw new Error("MACOS_NOTARY_ISSUER_ID must be an App Store Connect issuer UUID");
  const signPassword = requiredEnv("MACOS_SIGN_PASSWORD");
  const p12 = strictBase64(requiredEnv("MACOS_SIGN_P12"), "MACOS_SIGN_P12");
  const notaryKey = strictBase64(requiredEnv("MACOS_NOTARY_KEY"), "MACOS_NOTARY_KEY");
  const dist = requiredEnv("RELEASE_DIST_DIR");
  const names = releaseAssetNames(inputs.version);
  const tempRoot = keychainPaths().tempRoot;
  const notarizationZip = join(tempRoot, "notarization.zip");

  await rm(dist, { force: true, recursive: true });
  await mkdir(dist, { recursive: true });
  await runInherited("mise", ["run", "package:macos-release"]);
  await verifyPackageConfiguration(appPath, inputs.version);

  try {
    const signing = await prepareKeychain(teamId, p12, signPassword, notaryKey);
    const finalArchive = join(dist, names.macos);
    await runMacReleasePipeline({
      signAndVerify: () => signAndVerifyApp(signing.identity, teamId),
      createNotarizationArchive: () => zipApp(notarizationZip),
      submitNotarization: async () => {
        const notarization = await runCaptured("xcrun", [
          "notarytool", "submit", notarizationZip,
          "--wait", "--output-format", "json",
          "--key", signing.notaryKeyPath,
          "--key-id", keyId,
          "--issuer", issuerId,
        ]);
        const result = objectValue(JSON.parse(notarization.stdout), "notarization result");
        if (result.status !== "Accepted") throw new Error(`notarization finished with status ${String(result.status)}`);
        console.log(`notarization accepted: ${String(result.id)}`);
      },
      staple: () => runInherited("xcrun", ["stapler", "staple", appPath]),
      validateStaple: () => runInherited("xcrun", ["stapler", "validate", appPath]),
      verifyStapledSignatures: () => verifySignedApp(teamId),
      assessGatekeeper: () => runInherited("spctl", ["--assess", "--type", "execute", "--verbose=4", appPath]),
      createFinalArchive: () => zipApp(finalArchive),
    });
    await generateSbom(dist, appPath, inputs.version);
    await generateFixtureAppcast(dist, names.macos, { ...inputs, tag: `v${inputs.version}` });
    for (const name of names.platforms[0].payloads) {
      if ((await stat(join(dist, name))).size <= 0) throw new Error(`${name} is empty`);
    }
    console.log(`prepared signed macOS payloads ${names.platforms[0].payloads.join(", ")}`);
  } finally {
    await cleanupSigning();
  }
}

async function validateReleaseMetadata(inputs: ReleaseInputs, dist: string): Promise<void> {
  await verifyLocalAssets(inputs, dist);
  const names = releaseAssetNames(inputs.version);
  const sbomPath = join(dist, names.macosSbom);
  validateRuntimeSpdx(JSON.parse(await readFile(sbomPath, "utf8")), inputs.version);
  await runInherited("pyspdxtools", ["-i", sbomPath]);
  const publicKey = await sparklePublicKeyFromArchive(
    join(dist, names.macos),
    await expectedSparklePublicKey(),
  );
  validateAppcast(
    await readFile(join(dist, names.appcast)),
    await readFile(join(dist, names.macos)),
    names.macos,
    inputs,
    publicKey,
  );
}

async function finalizeReleaseMetadata(): Promise<void> {
  const inputs = validateReleaseInputs(
    requiredEnv("RELEASE_SHA"),
    requiredEnv("RELEASE_TAG"),
    requiredEnv("RELEASE_VERSION"),
  );
  const dist = requiredEnv("RELEASE_DIST_DIR");
  await verifyLocalAssets(inputs, dist);
  const privateKey = (await new Response(Bun.stdin.stream()).text()).trim();
  if (privateKey.length === 0) throw new Error("a Sparkle EdDSA private key must be supplied on standard input");
  const names = releaseAssetNames(inputs.version);
  const publicKey = await sparklePublicKeyFromArchive(
    join(dist, names.macos),
    await expectedSparklePublicKey(),
  );
  await generateAppcast(dist, names.macos, inputs, publicKey, privateKey);
  await writePlatformManifest(join(dist, names.checksums), names.payloads.map(name => join(dist, name)));
  await validateReleaseMetadata(inputs, dist);
  console.log(`prepared signed release metadata ${names.appcast} and ${names.checksums}`);
}

async function verifyReleaseMetadata(): Promise<void> {
  const inputs = validateReleaseInputs(
    requiredEnv("RELEASE_SHA"),
    requiredEnv("RELEASE_TAG"),
    requiredEnv("RELEASE_VERSION"),
  );
  await validateReleaseMetadata(inputs, requiredEnv("RELEASE_DIST_DIR"));
  console.log(`verified signed updater metadata for ${inputs.tag}`);
}

async function fetchPublicAsset(url: string): Promise<Buffer> {
  const response = await fetch(url, {
    headers: { "User-Agent": "huterm-release-probe" },
    redirect: "follow",
    signal: AbortSignal.timeout(30_000),
  });
  if (!response.ok) throw new Error(`public release probe failed for ${url}: HTTP ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
}

export function assertPublicAssetMatches(publicBytes: Buffer, expectedBytes: Buffer, mismatchMessage: string): void {
  if (!publicBytes.equals(expectedBytes)) throw new Error(mismatchMessage);
}

function repository(): string {
  const value = requiredEnv("GITHUB_REPOSITORY");
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(value)) throw new Error(`invalid GITHUB_REPOSITORY: ${value}`);
  return value;
}

async function probePublicRelease(): Promise<void> {
  const inputs = validateReleaseInputs(
    requiredEnv("RELEASE_SHA"),
    requiredEnv("RELEASE_TAG"),
    requiredEnv("RELEASE_VERSION"),
  );
  const dist = requiredEnv("RELEASE_DIST_DIR");
  const names = releaseAssetNames(inputs.version);
  const taggedPrefix = `https://github.com/${repository()}/releases/download/${inputs.tag}`;
  const latestAppcast = `https://github.com/${repository()}/releases/latest/download/${names.appcast}`;
  const expectedAppcast = await readFile(join(dist, names.appcast));
  let publicAppcast: Buffer | undefined;
  let lastError: unknown;
  for (let attempt = 1; attempt <= 10; attempt += 1) {
    try {
      const candidate = await fetchPublicAsset(latestAppcast);
      assertPublicAssetMatches(candidate, expectedAppcast, "latest appcast bytes do not match the published asset");
      publicAppcast = expectedAppcast;
      break;
    } catch (error) {
      lastError = error;
      if (attempt < 10) await Bun.sleep(3_000);
    }
  }
  if (!publicAppcast) throw lastError;
  const probeDirectory = requiredEnv("RELEASE_PUBLIC_DIR");
  await rm(probeDirectory, { force: true, recursive: true });
  await mkdir(probeDirectory, { recursive: true });
  for (const name of [names.macos, names.macosSbom]) {
    const downloaded = await fetchPublicAsset(`${taggedPrefix}/${name}`);
    const localPath = join(dist, name);
    const local = await readFile(localPath);
    assertPublicAssetMatches(downloaded, local, `public ${name} bytes do not match the verified release asset`);
    await copyFile(localPath, join(probeDirectory, name));
  }
  await writeFile(join(probeDirectory, names.appcast), publicAppcast);
  validateAppcast(
    publicAppcast,
    await readFile(join(probeDirectory, names.macos)),
    names.macos,
    inputs,
    await sparklePublicKeyFromArchive(
      join(probeDirectory, names.macos),
      await expectedSparklePublicKey(),
    ),
  );
  console.log(`verified unauthenticated appcast, archive, and SBOM for ${inputs.tag}`);
}

async function main(): Promise<void> {
  const command = Bun.argv[2];
  switch (command) {
    case "verify-package-config": {
      const bundle = Bun.argv[3];
      if (!bundle) throw new Error("verify-package-config requires an app bundle path");
      await verifyPackageConfiguration(resolve(repoRoot, bundle));
      console.log("verified macOS privacy descriptions, release entitlements, and static Ghostty linkage");
      break;
    }
    case "verify-local-package-config": {
      const bundle = Bun.argv[3];
      if (!bundle) throw new Error("verify-local-package-config requires an app bundle path");
      await verifyLocalPackageConfiguration(resolve(repoRoot, bundle));
      console.log("verified Sparkle-free macOS package configuration and static Ghostty linkage");
      break;
    }
    case "validate-updater-inputs":
      await expectedSparklePublicKey();
      console.log("verified the canonical Sparkle public key");
      break;
    case "prepare-updater-package": {
      const bundle = Bun.argv[3];
      if (!bundle) throw new Error("prepare-updater-package requires an app bundle path");
      await prepareUpdaterPackage(resolve(repoRoot, bundle));
      console.log("installed the verified Sparkle framework and production updater metadata");
      break;
    }
    case "build":
      await buildRelease();
      break;
    case "finalize-metadata":
      await finalizeReleaseMetadata();
      break;
    case "verify-release-metadata":
      await verifyReleaseMetadata();
      break;
    case "probe-public":
      await probePublicRelease();
      break;
    case "cleanup":
      await cleanupSigning();
      break;
    default:
      throw new Error(
        "expected validate-updater-inputs, prepare-updater-package, verify-package-config, verify-local-package-config, build, finalize-metadata, verify-release-metadata, probe-public, or cleanup",
      );
  }
}

if (import.meta.main) {
  try {
    await main();
  } catch (error) {
    console.error(`macOS release failed: ${error instanceof Error ? error.message : error}`);
    process.exitCode = 1;
  }
}
