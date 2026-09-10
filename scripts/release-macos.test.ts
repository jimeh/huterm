import { expect, test } from "bun:test";
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
  parseDeveloperIdentity,
  parseSimplePlist,
  privacyUsageDescriptions,
  releaseEntitlements,
  runMacReleasePipeline,
  validateEntitlements,
  validatePrivacyDescriptions,
  validateSignatureDetails,
} from "./release-macos.ts";

const repoRoot = resolve(import.meta.dir, "..");
const inputs = { sha: "a".repeat(40), tag: "v0.1.0", version: "0.1.0" };

test("package verification checks static linkage without rg and rejects failed inspection", async () => {
  const directory = await mkdtemp(join(tmpdir(), "huterm-linkage-"));
  try {
    const plutil = join(directory, "plutil");
    await writeFile(plutil, '#!/bin/bash\ncase "${@: -1}" in\n  *.entitlements) printf "%s" "$TEST_ENTITLEMENTS" ;;\n  *) printf "%s" "$TEST_PRIVACY" ;;\nesac\n');
    await chmod(plutil, 0o755);
    const otool = join(directory, "otool");
    await writeFile(otool, '#!/bin/bash\nprintf "%s" "$TEST_LINKAGE"\nexit "$TEST_OTOOL_EXIT"\n');
    await chmod(otool, 0o755);
    for (const [linkage, code, expected] of [
      ["huterm:\n /usr/lib/libSystem.B.dylib\n", "0", 0],
      ["huterm:\n @rpath/libghostty-vt.dylib\n", "0", 1],
      ["", "1", 1],
    ] as const) {
      const result = Bun.spawnSync([process.execPath, join(import.meta.dir, "release-macos.ts"), "verify-package-config", "fixture.app"], { env: {
        ...process.env, PATH: directory, TEST_LINKAGE: linkage, TEST_OTOOL_EXIT: code,
        TEST_PRIVACY: JSON.stringify(privacyUsageDescriptions),
        TEST_ENTITLEMENTS: JSON.stringify(Object.fromEntries(releaseEntitlements.map(key => [key, true]))),
      } });
      expect(result.exitCode, result.stderr.toString()).toBe(expected);
      if (linkage.includes("libghostty")) expect(result.stderr.toString()).toContain("Ghostty must be statically linked");
      if (code === "1") expect(result.stderr.toString()).toContain("otool exited with status 1");
    }
    await rm(otool);
    const result = Bun.spawnSync([process.execPath, join(import.meta.dir, "release-macos.ts"), "verify-package-config", "fixture.app"], { env: {
      ...process.env, PATH: directory, TEST_PRIVACY: JSON.stringify(privacyUsageDescriptions),
      TEST_ENTITLEMENTS: JSON.stringify(Object.fromEntries(releaseEntitlements.map(key => [key, true]))),
    } });
    expect(result.exitCode).toBe(1);
    expect(result.stderr.toString()).toContain("otool");
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("pre-checkout guard permits exact branch verification but keeps publishing on main", async () => {
  const workflow = Bun.YAML.parse(await readFile(resolve(repoRoot, ".github/workflows/release.yml"), "utf8")) as {
    jobs: { preflight: { steps: { name?: string; run?: string }[] } };
  };
  const script = workflow.jobs.preflight.steps.find(step => step.name === "Validate source selection")!.run!;
  const directory = await mkdtemp(join(tmpdir(), "huterm-release-guard-"));
  try {
    const gh = join(directory, "gh");
    await writeFile(gh, '#!/bin/bash\nprintf "%s\\n" "$TEST_MAIN_STATUS"\n');
    await chmod(gh, 0o755);
    for (const [publish, event, ref, sha, status, expected] of [
      ["false", "workflow_dispatch", "refs/heads/fix", inputs.sha, "behind", 0],
      ["true", "workflow_dispatch", "refs/heads/fix", inputs.sha, "behind", 1],
      ["false", "push", "refs/heads/fix", inputs.sha, "behind", 1],
      ["false", "workflow_dispatch", "refs/tags/v0.1.0", inputs.sha, "behind", 1],
      ["false", "workflow_dispatch", "refs/heads/fix", "b".repeat(40), "behind", 1],
      ["true", "workflow_dispatch", "refs/heads/main", inputs.sha, "identical", 0],
      ["false", "workflow_dispatch", "refs/heads/main", "b".repeat(40), "ahead", 0],
      ["false", "workflow_dispatch", "refs/heads/fix", "invalid", "ahead", 1],
    ] as const) {
      const result = Bun.spawnSync(["bash", "-c", script], { env: {
        ...process.env, PATH: `${directory}:${process.env.PATH}`, RELEASE_PUBLISH: publish,
        GITHUB_EVENT_NAME: event, GITHUB_REF: ref, GITHUB_SHA: inputs.sha,
        RELEASE_SHA: sha, GITHUB_REPOSITORY: "fixture/huterm", TEST_MAIN_STATUS: status,
        GITHUB_OUTPUT: join(directory, "source-output"),
      } });
      expect(result.exitCode, `${publish} ${event} ${ref} ${sha}: ${result.stderr}`).toBe(expected);
    }
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("source bundle metadata and entitlements match the release contract", async () => {
  const info = parseSimplePlist(await readFile(resolve(repoRoot, "assets/macos/Info.plist"), "utf8"));
  const entitlements = parseSimplePlist(await readFile(resolve(repoRoot, "assets/macos/Huterm.entitlements"), "utf8"));
  expect(info).toEqual({ CFBundleIconName: "Huterm", ...privacyUsageDescriptions });
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
  for (const dependency of ["huterm-config", "huterm-core", "huterm-gpui", "huterm-protocol"]) {
    expect(rootManifest).toContain(`${dependency} = { path = "crates/${dependency}", version = "=${rootVersion}" } # x-release-please-version`);
  }
  for (const manifestPath of ["crates/huterm-config/Cargo.toml", "crates/huterm-core/Cargo.toml", "crates/huterm-gpui/Cargo.toml", "crates/huterm-protocol/Cargo.toml"]) {
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
  expect(releaseWorkflow).toContain("run: bun scripts/release.ts validate-build");
  expect(releaseWorkflow).toContain("if: ${{ !inputs.publish }}");
  expect(releaseWorkflow).toContain("uses: actions/upload-artifact@");
  expect(releaseWorkflow).toContain("retention-days: 7");
  expect(releaseWorkflow).toContain("if: inputs.publish");
});
