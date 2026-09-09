import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import {
  parseDeveloperIdentity,
  parseSimplePlist,
  privacyUsageDescriptions,
  releaseEntitlements,
  runMacReleasePipeline,
  validateBuildInputs,
  validateDraftRelease,
  validateEntitlements,
  validatePrivacyDescriptions,
  validateReleaseAssets,
  validateReleaseInputs,
  validateSignatureDetails,
  validateWorkspaceVersions,
} from "./release-macos.ts";

const repoRoot = resolve(import.meta.dir, "..");
const inputs = { sha: "a".repeat(40), tag: "v0.1.0", version: "0.1.0" };

test("build inputs require a full SHA and stable version without a tag", () => {
  expect(validateBuildInputs(inputs.sha, inputs.version)).toEqual({ sha: inputs.sha, version: inputs.version });
  expect(() => validateBuildInputs("abc", inputs.version)).toThrow("invalid release SHA");
  expect(() => validateBuildInputs(inputs.sha, "0.1.0-beta.1")).toThrow("invalid release version");
});

test("release inputs bind a full SHA, stable version, and matching tag", () => {
  expect(validateReleaseInputs(inputs.sha, inputs.tag, inputs.version)).toEqual(inputs);
  expect(() => validateReleaseInputs("abc", inputs.tag, inputs.version)).toThrow("invalid release SHA");
  expect(() => validateReleaseInputs(inputs.sha, "v0.2.0", inputs.version)).toThrow("does not match version");
  expect(() => validateReleaseInputs(inputs.sha, "v0.1.0-beta.1", "0.1.0-beta.1")).toThrow("invalid release version");
});

test("draft validation requires one exact unpublished release", () => {
  const release = { id: 17, draft: true, prerelease: false, tag_name: inputs.tag, target_commitish: inputs.sha };
  expect(validateDraftRelease([[release]], inputs, 17)).toEqual(release);
  expect(() => validateDraftRelease([{ ...release, draft: false }], inputs)).toThrow("not a draft");
  expect(() => validateDraftRelease([{ ...release, target_commitish: "b".repeat(40) }], inputs)).toThrow("targets");
  expect(() => validateDraftRelease([release, { ...release, id: 18 }], inputs)).toThrow("found 2");
  expect(() => validateDraftRelease([release], inputs, 18)).toThrow("does not match 18");
});

test("all Huterm Cargo packages must match the release version", () => {
  const packages = ["huterm", "huterm-core", "huterm-gpui", "huterm-protocol"].map(name => ({ name, version: inputs.version }));
  expect(() => validateWorkspaceVersions({ packages }, inputs.version)).not.toThrow();
  expect(() => validateWorkspaceVersions({ packages: packages.map(item => item.name === "huterm-core" ? { ...item, version: "0.2.0" } : item) }, inputs.version)).toThrow("huterm-core version");
});

test("source privacy descriptions and entitlements match the release contract", async () => {
  const info = parseSimplePlist(await readFile(resolve(repoRoot, "assets/macos/Info.plist"), "utf8"));
  const entitlements = parseSimplePlist(await readFile(resolve(repoRoot, "assets/macos/Huterm.entitlements"), "utf8"));
  expect(info).toEqual(privacyUsageDescriptions);
  expect(Object.keys(entitlements).sort()).toEqual([...releaseEntitlements].sort());
  expect(() => validatePrivacyDescriptions(info)).not.toThrow();
  expect(() => validateEntitlements(entitlements)).not.toThrow();
  expect(() => validatePrivacyDescriptions({ ...info, NSCameraUsageDescription: "wrong" })).toThrow("NSCameraUsageDescription");
  expect(() => validateEntitlements({ ...entitlements, "com.apple.security.cs.allow-jit": true })).toThrow("keys do not match");
});

test("release signing details require Developer ID, team, runtime, and timestamp", () => {
  const details = [
    "Authority=Developer ID Application: Jim Example (ABCDE12345)",
    "TeamIdentifier=ABCDE12345",
    "CodeDirectory v=20500 size=123 flags=0x10000(runtime) hashes=1+7 location=embedded",
    "Timestamp=8 Sep 2026 at 12:30:00",
  ].join("\n");
  expect(() => validateSignatureDetails(details, "ABCDE12345", "Huterm")).not.toThrow();
  expect(() => validateSignatureDetails(details.replace("(runtime)", "(none)"), "ABCDE12345", "Huterm")).toThrow("hardened runtime");
  expect(() => validateSignatureDetails(details.replace("Timestamp=8 Sep 2026 at 12:30:00", "Timestamp=none"), "ABCDE12345", "Huterm")).toThrow("timestamp");
});

test("Developer ID selection rejects missing or ambiguous identities", () => {
  const identity = "A".repeat(40);
  const output = `  1) ${identity} \"Developer ID Application: Jim Example (ABCDE12345)\"`;
  expect(parseDeveloperIdentity(output, "ABCDE12345")).toBe(identity);
  expect(() => parseDeveloperIdentity("0 valid identities found", "ABCDE12345")).toThrow("found 0");
  expect(() => parseDeveloperIdentity(`${output}\n${output}`, "ABCDE12345")).toThrow("found 2");
});

test("remote assets must exactly match local names, sizes, states, and digests", () => {
  const local = [
    { name: "Huterm-0.1.0-macOS-universal.zip", path: "/dist/app.zip", size: 120, digest: "a".repeat(64) },
    { name: "SHA256SUMS", path: "/dist/SHA256SUMS", size: 100, digest: "b".repeat(64) },
  ];
  const remote = local.map(asset => ({ name: asset.name, size: asset.size, state: "uploaded", digest: `sha256:${asset.digest}` }));
  expect(() => validateReleaseAssets([remote], local)).not.toThrow();
  expect(() => validateReleaseAssets([...remote, { name: "extra", size: 1, state: "uploaded", digest: `sha256:${"c".repeat(64)}` }], local)).toThrow("names do not match");
  expect(() => validateReleaseAssets(remote.map(asset => asset.name === "SHA256SUMS" ? { ...asset, digest: null } : asset), local)).toThrow("digest");
});

test("the final archive is created only after notarization and stapled-app verification", async () => {
  const calls: string[] = [];
  const step = (name: string) => async () => { calls.push(name); };
  await runMacReleasePipeline({
    signAndVerify: step("sign"),
    createNotarizationArchive: step("notarization archive"),
    submitNotarization: step("notarize"),
    staple: step("staple"),
    validateStaple: step("validate staple"),
    verifyStapledSignatures: step("verify signatures"),
    assessGatekeeper: step("spctl"),
    createFinalArchive: step("final archive"),
  });
  expect(calls).toEqual([
    "sign",
    "notarization archive",
    "notarize",
    "staple",
    "validate staple",
    "verify signatures",
    "spctl",
    "final archive",
  ]);
  expect(calls.filter(call => call === "sign")).toHaveLength(1);
});

test("release-please can update explicit package versions and centralized exact pins", async () => {
  const rootManifest = await readFile(resolve(repoRoot, "Cargo.toml"), "utf8");
  const rootVersion = /^version = "([^"]+)"$/m.exec(rootManifest)?.[1];
  expect(rootVersion).toMatch(/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/);
  expect(rootManifest).toContain('[package]\nname = "huterm"');
  expect(rootManifest).not.toMatch(/^version\.workspace = true$/m);
  for (const dependency of ["huterm-core", "huterm-gpui", "huterm-protocol"]) {
    expect(rootManifest).toContain(`${dependency} = { path = "crates/${dependency}", version = "=${rootVersion}" } # x-release-please-version`);
  }
  for (const manifestPath of ["crates/huterm-core/Cargo.toml", "crates/huterm-gpui/Cargo.toml", "crates/huterm-protocol/Cargo.toml"]) {
    const manifest = await readFile(resolve(repoRoot, manifestPath), "utf8");
    expect(manifest).toContain(`version = "${rootVersion}"`);
    expect(manifest).not.toMatch(/^version\.workspace = true$/m);
  }
});

test("release workflows use the documented repository credential names", async () => {
  const releasePleaseWorkflow = await readFile(resolve(repoRoot, ".github/workflows/release-please.yml"), "utf8");
  const releaseWorkflow = await readFile(resolve(repoRoot, ".github/workflows/release.yml"), "utf8");
  const releaseGuide = await readFile(resolve(repoRoot, "docs/agents/releases.md"), "utf8");
  const variables = [
    "RELEASE_BOT_CLIENT_ID",
    "APPLE_TEAM_ID",
    "APPLE_NOTARIZATION_KEY_ID",
    "APPLE_NOTARIZATION_ISSUER_ID",
  ];
  const secrets = [
    "RELEASE_BOT_PRIVATE_KEY",
    "MACOS_DEVELOPER_ID_APPLICATION_P12_BASE64",
    "MACOS_DEVELOPER_ID_APPLICATION_P12_PASSWORD",
    "APPLE_NOTARIZATION_KEY_P8_BASE64",
  ];

  for (const variable of variables) {
    expect(releaseGuide).toContain(`\`${variable}\``);
    expect(releaseWorkflow).toContain(`vars.${variable}`);
  }
  for (const secret of secrets) {
    expect(releaseGuide).toContain(`\`${secret}\``);
    expect(releaseWorkflow).toContain(`secrets.${secret}`);
    expect(releasePleaseWorkflow).toContain(`secrets.${secret}`);
  }
});

test("manual verification signs without requiring or publishing a GitHub release", async () => {
  const releaseWorkflow = await readFile(resolve(repoRoot, ".github/workflows/release.yml"), "utf8");
  const sourceValidation = releaseWorkflow.indexOf("name: Validate source selection");
  const checkout = releaseWorkflow.indexOf("name: Check out exact release commit");

  expect(releaseWorkflow).toContain("publish:");
  expect(releaseWorkflow).toContain("default: false");
  expect(sourceValidation).toBeGreaterThan(0);
  expect(checkout).toBeGreaterThan(sourceValidation);
  expect(releaseWorkflow).toContain('compare/${RELEASE_SHA}...main');
  expect(releaseWorkflow).toContain("run: bun scripts/release-macos.ts validate-build");
  expect(releaseWorkflow).toContain("if: ${{ !inputs.publish }}");
  expect(releaseWorkflow).toContain("uses: actions/upload-artifact@");
  expect(releaseWorkflow).toContain("retention-days: 7");
  expect(releaseWorkflow).toContain("if: inputs.publish");
});
