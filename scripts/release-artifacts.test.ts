import { expect, test } from "bun:test";
import { createPrivateKey, createPublicKey, generateKeyPairSync, sign } from "node:crypto";
import {
  augmentSpdx,
  generateAppcast,
  ghosttyNativePackage,
  sparklePublicKeyFromPrivateKey,
  validateAppcast,
  validateRuntimeSpdx,
} from "./release-artifacts.ts";

function signingFixture() {
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  const spki = publicKey.export({ format: "der", type: "spki" });
  return { privateKey, publicKey: spki.subarray(-32).toString("base64") };
}

function signedAppcast(
  archive: Buffer,
  signatureKey: ReturnType<typeof signingFixture>,
  version = "1.2.3",
  urls = {
    archive: `https://github.com/jimeh/huterm/releases/download/v${version}/Huterm-${version}-macOS-universal.zip`,
    release: `https://github.com/jimeh/huterm/releases/tag/v${version}`,
  },
) {
  const archiveSignature = sign(null, archive, signatureKey.privateKey).toString("base64");
  const content = Buffer.from(`<?xml version="1.0" encoding="utf-8"?>
<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" version="2.0"><channel>
<link>https://github.com/jimeh/huterm</link><item>
<link>${urls.release}</link>
<sparkle:version>${version}</sparkle:version>
<enclosure url="${urls.archive}" length="${archive.length}" sparkle:edSignature="${archiveSignature}" />
</item></channel></rss>`);
  const feedSignature = sign(null, content, signatureKey.privateKey).toString("base64");
  return Buffer.concat([content, Buffer.from(`<!-- sparkle-signatures:
edSignature: ${feedSignature}
length: ${content.length}
-->`)]);
}

test("appcast validation binds identity, immutable URLs, sizes, archive bytes, and feed bytes", () => {
  const archive = Buffer.from("signed universal application fixture");
  const key = signingFixture();
  const appcast = signedAppcast(archive, key);
  expect(() => validateAppcast(appcast, archive, "Huterm-1.2.3-macOS-universal.zip", { version: "1.2.3", tag: "v1.2.3" }, key.publicKey)).not.toThrow();
  expect(() => validateAppcast(appcast, Buffer.from("tampered"), "Huterm-1.2.3-macOS-universal.zip", { version: "1.2.3", tag: "v1.2.3" }, key.publicKey)).toThrow();
  const otherKey = signingFixture();
  expect(() => validateAppcast(appcast, archive, "Huterm-1.2.3-macOS-universal.zip", { version: "1.2.3", tag: "v1.2.3" }, otherKey.publicKey)).toThrow("embedded public key");
  expect(() => validateAppcast(appcast, archive, "Huterm-1.2.3-macOS-universal.zip", { version: "1.2.4", tag: "v1.2.4" }, key.publicKey)).toThrow("version");
});

test("appcast validation honors explicit fixture URLs", () => {
  const archive = Buffer.from("manual verification archive");
  const key = signingFixture();
  const urls = {
    archive: "https://updates.huterm.invalid/v1.2.3/Huterm-1.2.3-macOS-universal.zip",
    release: "https://updates.huterm.invalid/v1.2.3/notes",
  };
  const appcast = signedAppcast(archive, key, "1.2.3", urls);
  expect(() => validateAppcast(
    appcast,
    archive,
    "Huterm-1.2.3-macOS-universal.zip",
    { version: "1.2.3", tag: "v1.2.3" },
    key.publicKey,
    urls,
  )).not.toThrow();
});

test("Sparkle private keys derive the matching canonical public key", () => {
  const seed = Buffer.alloc(32, 7);
  const privateKey = seed.toString("base64");
  const privateKeyObject = createPrivateKey({
    key: Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), seed]),
    format: "der",
    type: "pkcs8",
  });
  const expected = createPublicKey(privateKeyObject).export({ format: "der", type: "spki" }).subarray(-32).toString("base64");
  expect(sparklePublicKeyFromPrivateKey(privateKey)).toBe(expected);
  expect(() => sparklePublicKeyFromPrivateKey("not-a-key")).toThrow("canonical base64");
});

test("appcast generation rejects a private key that does not match the embedded public key", async () => {
  const privateKey = Buffer.alloc(32, 7).toString("base64");
  const otherPublicKey = sparklePublicKeyFromPrivateKey(Buffer.alloc(32, 8).toString("base64"));
  await expect(generateAppcast(
    "/unused",
    "Huterm-1.2.3-macOS-universal.zip",
    { version: "1.2.3", tag: "v1.2.3" },
    otherPublicKey,
    privateKey,
  )).rejects.toThrow("does not match the embedded public key");
});

function cargoPackage(name: string, versionInfo: string) {
  return {
    SPDXID: `SPDXRef-${name}`,
    name,
    versionInfo,
    downloadLocation: "NOASSERTION",
    filesAnalyzed: false,
    licenseConcluded: "NOASSERTION",
    licenseDeclared: "NOASSERTION",
    copyrightText: "NOASSERTION",
    externalRefs: [{ referenceCategory: "PACKAGE-MANAGER", referenceType: "purl", referenceLocator: `pkg:cargo/${name}@${versionInfo}` }],
  };
}

test("SPDX augmentation unions architecture metadata and records pinned native provenance", async () => {
  const application = {
    SPDXID: "SPDXRef-DOCUMENT",
    spdxVersion: "SPDX-2.3",
    creationInfo: { created: "2026-09-10T00:00:00Z", creators: ["Tool: Syft"] },
    packages: [
      cargoPackage("huterm", "0.4.0"),
      cargoPackage("gpui", "0.2.2"),
      cargoPackage("libghostty-vt-sys", "0.2.1"),
    ],
    documentDescribes: ["SPDXRef-huterm"],
  };
  const result = await augmentSpdx(application, {
    arm64: { packages: [cargoPackage("anyhow", "1.0.104"), cargoPackage("arm-only", "1.0.0")] },
    x86_64: { packages: [cargoPackage("anyhow", "1.0.104"), cargoPackage("x86-only", "1.0.0")] },
  });
  const packages = result.packages as Record<string, unknown>[];
  const described = result.documentDescribes as string[];
  for (const name of ["gpui", "libghostty-vt-sys"]) {
    const retained = packages.find(pkg => pkg.name === name)!;
    expect(described).toContain(retained.SPDXID as string);
    expect(described).not.toContain(`SPDXRef-Package-${name}`);
  }
  expect(() => validateRuntimeSpdx(result, "0.4.0")).not.toThrow();
  expect(packages.find(pkg => pkg.name === "anyhow")?.annotations).toEqual([expect.objectContaining({ comment: expect.stringContaining("arm64 and x86_64") })]);
  expect(packages.find(pkg => pkg.name === "arm-only")?.annotations).toEqual([expect.objectContaining({ comment: expect.stringContaining("arm64") })]);
  expect(packages.find(pkg => pkg.name === "gpui")?.sourceInfo).toContain("Locally patched runtime crate");
  for (const name of ["Sparkle", "ghostty", "uucode", "highway", "libghostty-vt-sys"]) {
    expect(packages.some(pkg => pkg.name === name)).toBe(true);
  }
});

test("Ghostty native SPDX metadata is explicit for every package", () => {
  const source = {
    name: "future-native-package",
    version: "1.2.3",
    license: "MIT",
    url: "https://deps.files.ghostty.org/future-native-package.tar.gz",
    sha256: "a".repeat(64),
  };
  expect(ghosttyNativePackage(source)).toEqual(expect.objectContaining({
    name: source.name,
    versionInfo: source.version,
    licenseConcluded: source.license,
    licenseDeclared: source.license,
  }));
  expect(() => ghosttyNativePackage({ ...source, version: undefined })).toThrow(
    "future-native-package version must be a non-empty string",
  );
  expect(() => ghosttyNativePackage({ ...source, license: undefined })).toThrow(
    "future-native-package license must be a non-empty string",
  );
});

test("runtime SPDX validation rejects malformed and dangling document descriptions", async () => {
  const result = await augmentSpdx({
    SPDXID: "SPDXRef-DOCUMENT",
    spdxVersion: "SPDX-2.3",
    creationInfo: { created: "2026-09-10T00:00:00Z", creators: ["Tool: Syft"] },
    packages: [cargoPackage("huterm", "0.4.0")],
    documentDescribes: ["SPDXRef-huterm"],
  }, {
    arm64: { packages: [] },
    x86_64: { packages: [] },
  });
  result.documentDescribes = ["SPDXRef-missing"];
  expect(() => validateRuntimeSpdx(result, "0.4.0")).toThrow(
    "SBOM documentDescribes references missing package SPDXRef-missing",
  );
  result.documentDescribes = "SPDXRef-huterm";
  expect(() => validateRuntimeSpdx(result, "0.4.0")).toThrow(
    "SBOM documentDescribes must be an array",
  );
  result.documentDescribes = [7];
  expect(() => validateRuntimeSpdx(result, "0.4.0")).toThrow(
    "SBOM documentDescribes ID must be a non-empty string",
  );
  result.documentDescribes = ["SPDXRef-huterm"];
  const packages = result.packages as Record<string, unknown>[];
  packages.find(pkg => pkg.name === "huterm")!.SPDXID = 7;
  expect(() => validateRuntimeSpdx(result, "0.4.0")).toThrow(
    "SPDX package ID must be a non-empty string",
  );
});
