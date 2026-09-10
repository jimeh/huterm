import { expect, test } from "bun:test";
import { generateKeyPairSync, sign } from "node:crypto";
import { augmentSpdx, validateAppcast, validateRuntimeSpdx } from "./release-artifacts.ts";

function signingFixture() {
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  const spki = publicKey.export({ format: "der", type: "spki" });
  return { privateKey, publicKey: spki.subarray(-32).toString("base64") };
}

function signedAppcast(archive: Buffer, signatureKey: ReturnType<typeof signingFixture>, version = "1.2.3") {
  const archiveSignature = sign(null, archive, signatureKey.privateKey).toString("base64");
  const content = Buffer.from(`<?xml version="1.0" encoding="utf-8"?>
<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" version="2.0"><channel>
<link>https://github.com/jimeh/huterm</link><item>
<link>https://github.com/jimeh/huterm/releases/tag/v${version}</link>
<sparkle:version>${version}</sparkle:version>
<enclosure url="https://github.com/jimeh/huterm/releases/download/v${version}/Huterm-${version}-macOS-universal.zip" length="${archive.length}" sparkle:edSignature="${archiveSignature}" />
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
    packages: [cargoPackage("huterm", "0.4.0"), cargoPackage("gpui", "0.2.2")],
    documentDescribes: ["SPDXRef-huterm"],
  };
  const result = await augmentSpdx(application, {
    arm64: { packages: [cargoPackage("anyhow", "1.0.104"), cargoPackage("arm-only", "1.0.0")] },
    x86_64: { packages: [cargoPackage("anyhow", "1.0.104"), cargoPackage("x86-only", "1.0.0")] },
  });
  expect(() => validateRuntimeSpdx(result, "0.4.0")).not.toThrow();
  const packages = result.packages as Record<string, unknown>[];
  expect(packages.find(pkg => pkg.name === "anyhow")?.annotations).toEqual([expect.objectContaining({ comment: expect.stringContaining("arm64 and x86_64") })]);
  expect(packages.find(pkg => pkg.name === "arm-only")?.annotations).toEqual([expect.objectContaining({ comment: expect.stringContaining("arm64") })]);
  expect(packages.find(pkg => pkg.name === "gpui")?.sourceInfo).toContain("Locally patched runtime crate");
  for (const name of ["Sparkle", "ghostty", "uucode", "highway", "libghostty-vt-sys"]) {
    expect(packages.some(pkg => pkg.name === name)).toBe(true);
  }
});
