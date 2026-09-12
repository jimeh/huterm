import { createPrivateKey, createPublicKey, randomBytes, verify as verifySignature } from "node:crypto";
import { copyFile, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "..");
const sparkleTools = join(repoRoot, ".native/sparkle/distribution/bin");
const ed25519SpkiPrefix = Buffer.from("302a300506032b6570032100", "hex");
const ed25519Pkcs8Prefix = Buffer.from("302e020100300506032b657004220420", "hex");

export interface ArtifactInputs {
  version: string;
  tag: string;
}

export interface AppcastUrls {
  archive: string;
  release: string;
}

interface CommandResult {
  stdout: string;
  stderr: string;
}

function mergedEnvironment(additions: Record<string, string> = {}): Record<string, string> {
  const environment: Record<string, string> = { ...additions };
  for (const [key, value] of Object.entries(process.env)) {
    if (value !== undefined && environment[key] === undefined) environment[key] = value;
  }
  return environment;
}

async function runCaptured(command: string, args: string[], cwd = repoRoot): Promise<CommandResult> {
  const child = Bun.spawn([command, ...args], {
    cwd,
    env: mergedEnvironment(),
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (exitCode !== 0) throw new Error(`${command} exited with status ${exitCode}: ${stderr || stdout}`);
  return { stdout, stderr };
}

async function runWithSecretStdin(command: string, args: string[], secret: string, cwd: string): Promise<void> {
  const child = Bun.spawn([command, ...args], {
    cwd,
    env: mergedEnvironment(),
    stdin: "pipe",
    stdout: "inherit",
    stderr: "inherit",
  });
  child.stdin.write(secret);
  child.stdin.end();
  const exitCode = await child.exited;
  if (exitCode !== 0) throw new Error(`${command} exited with status ${exitCode}`);
}

async function runSecretCaptured(command: string, args: string[], secret: string, cwd: string): Promise<CommandResult> {
  const child = Bun.spawn([command, ...args], {
    cwd,
    env: mergedEnvironment(),
    stdin: "pipe",
    stdout: "pipe",
    stderr: "pipe",
  });
  child.stdin.write(secret);
  child.stdin.end();
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (exitCode !== 0) throw new Error(`${command} exited with status ${exitCode}: ${stderr || stdout}`);
  return { stdout, stderr };
}

function decodeXml(value: string): string {
  return value
    .replaceAll("&apos;", "'")
    .replaceAll("&quot;", '"')
    .replaceAll("&gt;", ">")
    .replaceAll("&lt;", "<")
    .replaceAll("&amp;", "&");
}

function elementText(xml: string, name: string): string[] {
  const pattern = new RegExp(`<${name}>([\\s\\S]*?)</${name}>`, "g");
  return [...xml.matchAll(pattern)].map(match => decodeXml(match[1]!.trim()));
}

function enclosureAttributes(xml: string): Record<string, string> {
  const enclosures = [...xml.matchAll(/<enclosure\s+([^>]*?)\/?\s*>/g)];
  if (enclosures.length !== 1) throw new Error(`appcast must contain exactly one enclosure, found ${enclosures.length}`);
  const attributes: Record<string, string> = {};
  for (const match of enclosures[0]![1]!.matchAll(/([:\w-]+)="([^"]*)"/g)) {
    attributes[match[1]!] = decodeXml(match[2]!);
  }
  return attributes;
}

function publicKeyObject(publicKey: string) {
  const raw = Buffer.from(publicKey, "base64");
  if (raw.length !== 32 || raw.toString("base64") !== publicKey) {
    throw new Error("Sparkle public key must be canonical base64 for 32 Ed25519 bytes");
  }
  return createPublicKey({ key: Buffer.concat([ed25519SpkiPrefix, raw]), format: "der", type: "spki" });
}

export function sparklePublicKeyFromPrivateKey(privateKey: string): string {
  const seed = Buffer.from(privateKey, "base64");
  if (seed.length !== 32 || seed.toString("base64") !== privateKey) {
    throw new Error("Sparkle private key must be canonical base64 for a 32-byte Ed25519 seed");
  }
  const privateKeyObject = createPrivateKey({
    key: Buffer.concat([ed25519Pkcs8Prefix, seed]),
    format: "der",
    type: "pkcs8",
  });
  const publicKeyDer = createPublicKey(privateKeyObject).export({ format: "der", type: "spki" });
  return publicKeyDer.subarray(-32).toString("base64");
}

export function validateAppcast(
  bytes: Uint8Array,
  archiveBytes: Uint8Array,
  archiveName: string,
  inputs: ArtifactInputs,
  publicKey: string,
  expectedUrls: AppcastUrls = {
    archive: `https://github.com/jimeh/huterm/releases/download/${inputs.tag}/${archiveName}`,
    release: `https://github.com/jimeh/huterm/releases/tag/${inputs.tag}`,
  },
): void {
  const appcast = Buffer.from(bytes);
  const xml = appcast.toString("utf8");
  if ([...xml.matchAll(/<item(?:\s|>)/g)].length !== 1) throw new Error("appcast must contain exactly one update item");
  const versions = elementText(xml, "sparkle:version");
  if (versions.length !== 1 || versions[0] !== inputs.version) {
    throw new Error(`appcast version does not match ${inputs.version}`);
  }
  const enclosure = enclosureAttributes(xml);
  if (enclosure.url !== expectedUrls.archive) throw new Error("appcast enclosure URL does not match the immutable release asset");
  if (enclosure.length !== String(archiveBytes.byteLength)) throw new Error("appcast enclosure length does not match the archive");
  const item = /<item(?:\s[^>]*)?>([\s\S]*?)<\/item>/.exec(xml);
  if (!item) throw new Error("appcast update item is malformed");
  const itemLinks = elementText(item[1]!, "link");
  if (itemLinks.length !== 1 || itemLinks[0] !== expectedUrls.release) {
    throw new Error("appcast release link does not match the expected release URL");
  }
  const archiveSignature = enclosure["sparkle:edSignature"];
  if (!archiveSignature) throw new Error("appcast enclosure lacks an EdDSA signature");
  const archiveSignatureBytes = Buffer.from(archiveSignature, "base64");
  if (archiveSignatureBytes.length !== 64 || archiveSignatureBytes.toString("base64") !== archiveSignature) {
    throw new Error("appcast enclosure has a malformed EdDSA signature");
  }
  const key = publicKeyObject(publicKey);
  if (!verifySignature(null, Buffer.from(archiveBytes), key, archiveSignatureBytes)) {
    throw new Error("appcast enclosure signature does not verify with the embedded public key");
  }

  const feedSignature = /<!-- sparkle-signatures:\s*edSignature:\s*([A-Za-z0-9+/=]+)\s*length:\s*(\d+)\s*-->/m.exec(xml);
  if (!feedSignature) throw new Error("appcast lacks a signed-feed trailer");
  const signedLength = Number(feedSignature[2]);
  const feedSignatureBytes = Buffer.from(feedSignature[1]!, "base64");
  if (!Number.isSafeInteger(signedLength) || signedLength <= 0 || signedLength > appcast.length) {
    throw new Error("appcast signed-feed length is invalid");
  }
  if (feedSignatureBytes.length !== 64 || !verifySignature(null, appcast.subarray(0, signedLength), key, feedSignatureBytes)) {
    throw new Error("appcast feed signature does not verify with the embedded public key");
  }
}

export async function generateAppcast(
  dist: string,
  archiveName: string,
  inputs: ArtifactInputs,
  publicKey: string,
  privateKey: string,
): Promise<void> {
  if (sparklePublicKeyFromPrivateKey(privateKey) !== publicKey) {
    throw new Error("Sparkle private key does not match the embedded public key");
  }
  const temporary = await mkdtemp(join(tmpdir(), "huterm-appcast-"));
  try {
    const archive = join(dist, archiveName);
    await copyFile(archive, join(temporary, archiveName));
    const output = join(temporary, "appcast.xml");
    await runWithSecretStdin(join(sparkleTools, "generate_appcast"), [
      "--ed-key-file", "-",
      "--download-url-prefix", `https://github.com/jimeh/huterm/releases/download/${inputs.tag}/`,
      "--link", `https://github.com/jimeh/huterm/releases/tag/${inputs.tag}`,
      "--versions", inputs.version,
      "--maximum-versions", "1",
      "--maximum-deltas", "0",
      "--disable-signing-warning",
      "-o", output,
      temporary,
    ], privateKey, temporary);
    const [appcastBytes, archiveBytes] = await Promise.all([readFile(output), readFile(archive)]);
    validateAppcast(appcastBytes, archiveBytes, archiveName, inputs, publicKey);
    await copyFile(output, join(dist, "appcast.xml"));
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}

export async function generateFixtureAppcast(
  dist: string,
  archiveName: string,
  inputs: ArtifactInputs,
): Promise<void> {
  const seed = randomBytes(32);
  const privateKey = seed.toString("base64");
  const publicKey = sparklePublicKeyFromPrivateKey(privateKey);
  const archive = join(dist, archiveName);
  const archiveBytes = await readFile(archive);
  const signature = (await runSecretCaptured(
    join(sparkleTools, "sign_update"),
    ["--ed-key-file", "-", "-p", archive],
    privateKey,
    dist,
  )).stdout.trim();
  if (!/^[A-Za-z0-9+/]{86}==$/.test(signature)) throw new Error("Sparkle returned a malformed fixture signature");
  const urls = {
    archive: `https://updates.huterm.invalid/${inputs.tag}/${archiveName}`,
    release: `https://updates.huterm.invalid/${inputs.tag}/notes`,
  };
  const appcastPath = join(dist, "appcast.xml");
  await writeFile(appcastPath, `<?xml version="1.0" encoding="utf-8"?>
<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" version="2.0">
  <channel>
    <title>Huterm manual verification fixture</title>
    <link>https://updates.huterm.invalid/</link>
    <description>Non-public Sparkle release fixture</description>
    <item>
      <title>Huterm ${inputs.version}</title>
      <link>${urls.release}</link>
      <sparkle:version>${inputs.version}</sparkle:version>
      <sparkle:shortVersionString>${inputs.version}</sparkle:shortVersionString>
      <enclosure url="${urls.archive}" length="${archiveBytes.length}" type="application/octet-stream" sparkle:edSignature="${signature}" />
    </item>
  </channel>
</rss>`);
  await runWithSecretStdin(
    join(sparkleTools, "sign_update"),
    ["--ed-key-file", "-", "--disable-signing-warning", appcastPath],
    privateKey,
    dist,
  );
  validateAppcast(await readFile(appcastPath), archiveBytes, archiveName, inputs, publicKey, urls);
}

type SpdxObject = Record<string, unknown>;

function objectValue(value: unknown, label: string): SpdxObject {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(`${label} must be an object`);
  return value as SpdxObject;
}

function stringValue(value: unknown, label: string): string {
  if (typeof value !== "string" || value.length === 0) throw new Error(`${label} must be a non-empty string`);
  return value;
}

function packageKey(value: unknown): string {
  const pkg = objectValue(value, "SPDX package");
  return `${stringValue(pkg.name, "SPDX package name")}\0${stringValue(pkg.versionInfo, "SPDX package version")}`;
}

function isCargoPackage(value: unknown): boolean {
  const pkg = objectValue(value, "SPDX package");
  return Array.isArray(pkg.externalRefs) && pkg.externalRefs.some(reference => {
    const candidate = objectValue(reference, "SPDX external reference");
    return typeof candidate.referenceLocator === "string" && candidate.referenceLocator.startsWith("pkg:cargo/");
  });
}

function annotation(created: string, architectures: string[]): SpdxObject[] {
  return [{
    annotationDate: created,
    annotationType: "OTHER",
    annotator: "Tool: huterm-release",
    comment: `Recovered from cargo-auditable metadata in ${architectures.join(" and ")} Mach-O slices.`,
  }];
}

function nativePackage(
  name: string,
  version: string,
  sourceUrl: string,
  digest: string,
  license: string,
  sourceInfo: string,
): SpdxObject {
  const idPart = name.replaceAll(/[^A-Za-z0-9.-]/g, "-");
  return {
    SPDXID: `SPDXRef-Package-${idPart}`,
    name,
    versionInfo: version,
    downloadLocation: sourceUrl,
    filesAnalyzed: false,
    checksums: [{ algorithm: "SHA256", checksumValue: digest }],
    licenseConcluded: license,
    licenseDeclared: license,
    copyrightText: "NOASSERTION",
    sourceInfo,
  };
}

export function ghosttyNativePackage(packageValue: unknown): SpdxObject {
  const pkg = objectValue(packageValue, "Ghostty native package");
  const name = stringValue(pkg.name, "Ghostty native package name");
  return nativePackage(
    name,
    stringValue(pkg.version, `${name} version`),
    stringValue(pkg.url, `${name} URL`),
    stringValue(pkg.sha256, `${name} digest`),
    stringValue(pkg.license, `${name} license`),
    "Statically linked through the pinned Ghostty Zig dependency graph.",
  );
}

export async function augmentSpdx(
  applicationDocument: unknown,
  slices: Record<"arm64" | "x86_64", unknown>,
): Promise<SpdxObject> {
  const document = structuredClone(objectValue(applicationDocument, "application SPDX document"));
  const creationInfo = objectValue(document.creationInfo, "SPDX creation info");
  const created = stringValue(creationInfo.created, "SPDX creation time");
  const packages = Array.isArray(document.packages) ? document.packages as SpdxObject[] : [];
  const byKey = new Map(packages.map(pkg => [packageKey(pkg), pkg]));
  const architectureByKey = new Map<string, string[]>();
  for (const architecture of ["arm64", "x86_64"] as const) {
    const slice = objectValue(slices[architecture], `${architecture} SPDX document`);
    if (!Array.isArray(slice.packages)) throw new Error(`${architecture} SPDX document has no packages`);
    for (const packageValue of slice.packages) {
      if (!isCargoPackage(packageValue)) continue;
      const key = packageKey(packageValue);
      architectureByKey.set(key, [...(architectureByKey.get(key) ?? []), architecture]);
      if (!byKey.has(key)) {
        const copied = structuredClone(objectValue(packageValue, "Cargo SPDX package"));
        packages.push(copied);
        byKey.set(key, copied);
      }
    }
  }
  for (const [key, architectures] of architectureByKey) {
    byKey.get(key)!.annotations = annotation(created, architectures);
  }

  const sparkle = objectValue(JSON.parse(await readFile(join(repoRoot, "scripts/sparkle-source.json"), "utf8")), "Sparkle source manifest");
  const sparkleSource = objectValue(sparkle.source, "Sparkle source");
  const ghostty = objectValue(JSON.parse(await readFile(join(repoRoot, "scripts/ghostty-source.json"), "utf8")), "Ghostty source manifest");
  const ghosttySource = objectValue(ghostty.source, "Ghostty source");
  const nativePackages = [
    nativePackage("Sparkle", stringValue(sparkle.version, "Sparkle version"), stringValue(sparkleSource.url, "Sparkle URL"), stringValue(sparkleSource.sha256, "Sparkle digest"), "MIT", "Pinned official Sparkle binary distribution."),
    nativePackage("ghostty", stringValue(ghostty.revision, "Ghostty revision"), stringValue(ghosttySource.url, "Ghostty URL"), stringValue(ghosttySource.sha256, "Ghostty digest"), "MIT", "Statically linked source at the reviewed native revision."),
  ];
  if (!Array.isArray(ghostty.packages)) throw new Error("Ghostty manifest has no native packages");
  for (const packageValue of ghostty.packages) {
    nativePackages.push(ghosttyNativePackage(packageValue));
  }
  const vendor = objectValue(JSON.parse(await readFile(join(repoRoot, "third-party/vendor/sources.json"), "utf8")), "vendor provenance");
  if (!Array.isArray(vendor.sources)) throw new Error("vendor provenance has no sources");
  for (const sourceValue of vendor.sources) {
    const source = objectValue(sourceValue, "vendored source");
    const patches = Array.isArray(source.patches) ? source.patches.map(patch => stringValue(objectValue(patch, "vendor patch").file, "vendor patch file")) : [];
    nativePackages.push(nativePackage(
      stringValue(source.name, "vendor name"),
      stringValue(source.version, "vendor version"),
      stringValue(source.url, "vendor URL"),
      stringValue(source.sha256, "vendor digest"),
      "NOASSERTION",
      `Locally patched runtime crate; reviewed patches: ${patches.join(", ")}.`,
    ));
  }
  for (const pkg of nativePackages) {
    const key = packageKey(pkg);
    if (!byKey.has(key)) {
      packages.push(pkg);
      byKey.set(key, pkg);
    } else if (typeof pkg.sourceInfo === "string" && pkg.sourceInfo.startsWith("Locally patched")) {
      byKey.get(key)!.sourceInfo = pkg.sourceInfo;
    }
  }
  packages.sort((left, right) => packageKey(left).localeCompare(packageKey(right)));
  document.packages = packages;
  const described = Array.isArray(document.documentDescribes) ? document.documentDescribes as string[] : [];
  for (const pkg of nativePackages) {
    const retained = byKey.get(packageKey(pkg))!;
    const id = stringValue(retained.SPDXID, "retained package SPDX ID");
    if (!described.includes(id)) described.push(id);
  }
  document.documentDescribes = described.sort();
  return document;
}

export function validateRuntimeSpdx(value: unknown, version: string): void {
  const document = objectValue(value, "SPDX document");
  if (document.spdxVersion !== "SPDX-2.3") throw new Error("SBOM must use SPDX 2.3");
  if (!Array.isArray(document.packages)) throw new Error("SBOM has no packages");
  const packages = document.packages.map(pkg => objectValue(pkg, "SPDX package"));
  const packageIds = new Set(packages.map(pkg => stringValue(pkg.SPDXID, "SPDX package ID")));
  const described = document.documentDescribes ?? [];
  if (!Array.isArray(described)) throw new Error("SBOM documentDescribes must be an array");
  for (const value of described) {
    const id = stringValue(value, "SBOM documentDescribes ID");
    if (!packageIds.has(id)) throw new Error(`SBOM documentDescribes references missing package ${id}`);
  }
  for (const [name, expectedVersion] of [
    ["huterm", version],
    ["Sparkle", "2.9.6"],
    ["ghostty", "a887df42c56f6de86c0fe6da9c4eeca37931e083"],
    ["uucode", "0.2.0"],
    ["highway", "66486a10623fa0d72fe91260f96c892e41aceb06"],
    ["gpui", "0.2.2"],
    ["libghostty-vt-sys", "0.2.1"],
  ] as const) {
    if (!packages.some(pkg => pkg.name === name && pkg.versionInfo === expectedVersion)) {
      throw new Error(`SBOM lacks required runtime component ${name}@${expectedVersion}`);
    }
  }
  if (!packages.some(isCargoPackage)) throw new Error("SBOM lacks cargo-auditable Rust package evidence");
  for (const patched of ["gpui", "libghostty-vt-sys"]) {
    const pkg = packages.find(candidate => candidate.name === patched);
    if (typeof pkg?.sourceInfo !== "string" || !pkg.sourceInfo.startsWith("Locally patched runtime crate")) {
      throw new Error(`SBOM lacks patched-crate provenance for ${patched}`);
    }
  }
  for (const forbidden of ["cargo-packager", "cargo-auditable", "syft", "zig", "bun"]) {
    if (packages.some(pkg => pkg.name === forbidden)) throw new Error(`SBOM includes build-only tool ${forbidden}`);
  }
}

async function syftDocument(source: string, output: string): Promise<SpdxObject> {
  await runCaptured("syft", [source, "--select-catalogers", "+cargo-auditable-binary-cataloger", "-o", `spdx-json=${output}`]);
  return objectValue(JSON.parse(await readFile(output, "utf8")), output);
}

export async function generateSbom(dist: string, appPath: string, version: string): Promise<string> {
  const temporary = await mkdtemp(join(tmpdir(), "huterm-sbom-"));
  try {
    const application = await syftDocument(`dir:${appPath}`, join(temporary, "application.json"));
    const arm64 = await syftDocument(join(repoRoot, "target/aarch64-apple-darwin/release/huterm"), join(temporary, "arm64.json"));
    const x86_64 = await syftDocument(join(repoRoot, "target/x86_64-apple-darwin/release/huterm"), join(temporary, "x86_64.json"));
    const document = await augmentSpdx(application, { arm64, x86_64 });
    validateRuntimeSpdx(document, version);
    const destination = join(dist, `Huterm-${version}-macOS-universal.spdx.json`);
    await writeFile(destination, `${JSON.stringify(document, null, 2)}\n`);
    const size = (await stat(destination)).size;
    if (size > 10 * 1024 * 1024) throw new Error(`SBOM is ${size} bytes, exceeding the 10 MiB release limit`);
    await runCaptured("pyspdxtools", ["-i", destination]);
    return basename(destination);
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}
