import { chmod, mkdir, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { releaseAssetNames, validateBuildInputs, type BuildInputs } from "./release.ts";

const repoRoot = resolve(import.meta.dir, "..");
const appPath = join(repoRoot, "target/release/bundle/Huterm.app");
const entitlementPath = join(repoRoot, "assets/macos/Huterm.entitlements");

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

async function verifyPackageConfiguration(bundlePath: string): Promise<void> {
  validatePrivacyDescriptions(await readPlist(join(bundlePath, "Contents/Info.plist")));
  validateEntitlements(await readPlist(entitlementPath));
  const linkage = await runCaptured("otool", ["-L", join(bundlePath, "Contents/MacOS/huterm")]);
  if (linkage.stdout.includes("libghostty")) throw new Error("package verification failed: Ghostty must be statically linked");
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
  if (start < 0 || end < 0) throw new Error(`codesign did not report entitlements for ${target}`);
  return parseSimplePlist(combined.slice(start, end + "</plist>".length));
}

async function signAndVerifyApp(identity: string, teamId: string): Promise<void> {
  const mainExecutable = join(appPath, "Contents/MacOS/huterm");
  const keychain = keychainPaths().keychain;
  const binaries = await findMachOBinaries(appPath);
  if (!binaries.includes(mainExecutable)) throw new Error("packaged app does not contain the Huterm Mach-O executable");
  for (const binary of binaries) await codesignTarget(binary, identity, keychain, binary === mainExecutable);
  await codesignTarget(appPath, identity, keychain, true);
  await verifySignedApp(teamId);
}

async function verifySignedApp(teamId: string): Promise<void> {
  const mainExecutable = join(appPath, "Contents/MacOS/huterm");
  const binaries = await findMachOBinaries(appPath);
  if (!binaries.includes(mainExecutable)) throw new Error("packaged app does not contain the Huterm Mach-O executable");
  await runInherited("codesign", ["--verify", "--deep", "--strict", "--verbose=4", appPath]);
  for (const target of [...binaries, appPath]) {
    await runInherited("codesign", ["--verify", "--strict", "--verbose=2", target]);
    const details = await runCaptured("codesign", ["-dvvv", target]);
    validateSignatureDetails(`${details.stdout}\n${details.stderr}`, teamId, relative(repoRoot, target));
  }
  validateEntitlements(await extractedEntitlements(mainExecutable));
  validateEntitlements(await extractedEntitlements(appPath));
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
  const archive = releaseAssetNames(inputs.version).macos;
  const tempRoot = keychainPaths().tempRoot;
  const notarizationZip = join(tempRoot, "notarization.zip");

  await mkdir(dist, { recursive: true });
  await runInherited("mise", ["run", "package:macos"]);
  await verifyPackageConfiguration(appPath);

  try {
    const signing = await prepareKeychain(teamId, p12, signPassword, notaryKey);
    const finalArchive = join(dist, archive);
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
    if ((await stat(finalArchive)).size <= 0) throw new Error("final macOS archive is empty");
    console.log(`prepared ${archive}`);
  } finally {
    await cleanupSigning();
  }
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
    case "build":
      await buildRelease();
      break;
    case "cleanup":
      await cleanupSigning();
      break;
    default:
      throw new Error("expected verify-package-config, build, or cleanup");
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
