import { expect, test } from "bun:test";
import { chmod, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
  assertPublicAssetMatches,
  parseDeveloperIdentity,
  parseSimplePlist,
  privacyUsageDescriptions,
  releaseEntitlements,
  runMacReleasePipeline,
  signingPlan,
  sparklePublicKeyFromArchive,
  updaterPlistValues,
  validateEntitlements,
  validateLocalPackagePlist,
  validatePrivacyDescriptions,
  validateSignatureDetails,
  validateUpdatePlist,
} from "./release-macos.ts";

const repoRoot = resolve(import.meta.dir, "..");
const inputs = { sha: "a".repeat(40), tag: "v0.1.0", version: "0.1.0" };

async function rootPackageVersion(): Promise<string> {
  const manifest = await readFile(resolve(repoRoot, "Cargo.toml"), "utf8");
  const version = /^version = "([^"]+)"$/m.exec(manifest)?.[1];
  if (!version) throw new Error("root Cargo package version is missing");
  return version;
}

test("public release comparison accepts exact bytes and rejects mismatches", () => {
  const expected = Buffer.from("verified local candidate");
  expect(() => assertPublicAssetMatches(Buffer.from(expected), expected, "mismatch")).not.toThrow();
  expect(() => assertPublicAssetMatches(Buffer.from("network mismatch"), expected, "mismatch")).toThrow("mismatch");
});

test("package verification checks static linkage without rg and rejects failed inspection", async () => {
  const packageVersion = await rootPackageVersion();
  const directory = await mkdtemp(join(tmpdir(), "huterm-linkage-"));
  try {
    const bundle = join(directory, "fixture.app");
    const framework = join(bundle, "Contents/Frameworks/Sparkle.framework");
    const sparkleVersion = join(framework, "Versions/B");
    await mkdir(join(sparkleVersion, "Updater.app/Contents/MacOS"), { recursive: true });
    await mkdir(join(sparkleVersion, "Resources"), { recursive: true });
    await mkdir(join(bundle, "Contents/MacOS"), { recursive: true });
    await mkdir(join(bundle, "Contents/Resources"), { recursive: true });
    await symlink("B", join(framework, "Versions/Current"));
    for (const [name, target] of [
      ["Sparkle", "Versions/Current/Sparkle"],
      ["Autoupdate", "Versions/Current/Autoupdate"],
      ["Updater.app", "Versions/Current/Updater.app"],
      ["Resources", "Versions/Current/Resources"],
    ] as const) await symlink(target, join(framework, name));
    for (const file of [
      join(sparkleVersion, "Sparkle"),
      join(sparkleVersion, "Autoupdate"),
      join(sparkleVersion, "Updater.app/Contents/MacOS/Updater"),
      join(bundle, "Contents/MacOS/huterm"),
    ]) await writeFile(file, "universal fixture");
    await writeFile(join(bundle, "Contents/Resources/Sparkle-LICENSE"), await readFile(join(repoRoot, "third-party/sparkle/LICENSE")));
    const publicKey = `${"A".repeat(43)}=`;
    const publicKeyFile = join(directory, "SparklePublicKey");
    await writeFile(publicKeyFile, publicKey);
    const updateInfo = {
      ...privacyUsageDescriptions,
      CFBundleShortVersionString: packageVersion,
      CFBundleVersion: packageVersion,
      LSMinimumSystemVersion: "10.15.7",
      SUFeedURL: "https://github.com/jimeh/huterm/releases/latest/download/appcast.xml",
      SUPublicEDKey: publicKey,
      SURequireSignedFeed: true,
      SUVerifyUpdateBeforeExtraction: true,
    };
    const plutil = join(directory, "plutil");
    await writeFile(plutil, '#!/bin/bash\ncase "${@: -1}" in\n  *.entitlements) printf "%s" "$TEST_ENTITLEMENTS" ;;\n  *) printf "%s" "$TEST_PRIVACY" ;;\nesac\n');
    await chmod(plutil, 0o755);
    const otool = join(directory, "otool");
    await writeFile(otool, '#!/bin/bash\nprintf "%s" "$TEST_LINKAGE"\nexit "$TEST_OTOOL_EXIT"\n');
    await chmod(otool, 0o755);
    const lipo = join(directory, "lipo");
    await writeFile(lipo, "#!/bin/bash\nexit 0\n");
    await chmod(lipo, 0o755);
    const absoluteHeader = `${repoRoot}/target/release/bundle/Huterm.app/Contents/MacOS/huterm`;
    for (const [linkage, code, expected] of [
      [`${absoluteHeader} (architecture x86_64):\n @rpath/Sparkle.framework/Versions/B/Sparkle\n /usr/lib/libSystem.B.dylib\n`, "0", 0],
      [`${absoluteHeader}:\n @rpath/Sparkle.framework/Versions/B/Sparkle\n ${repoRoot}/.native/sparkle/Sparkle.framework/Versions/B/Sparkle\n`, "0", 1],
      ["huterm:\n @rpath/libghostty-vt.dylib\n", "0", 1],
      ["", "1", 1],
    ] as const) {
      const result = Bun.spawnSync([process.execPath, join(import.meta.dir, "release-macos.ts"), "verify-package-config", bundle], { env: {
        ...process.env, PATH: directory, SPARKLE_PUBLIC_KEY_FILE: publicKeyFile,
        TEST_LINKAGE: linkage, TEST_OTOOL_EXIT: code,
        TEST_PRIVACY: JSON.stringify(updateInfo),
        TEST_ENTITLEMENTS: JSON.stringify(Object.fromEntries(releaseEntitlements.map(key => [key, true]))),
      } });
      expect(result.exitCode, result.stderr.toString()).toBe(expected);
      if (linkage.includes(".native/sparkle")) expect(result.stderr.toString()).toContain("build-machine path");
      if (linkage.includes("libghostty")) expect(result.stderr.toString()).toContain("Ghostty must be statically linked");
      if (code === "1") expect(result.stderr.toString()).toContain("otool exited with status 1");
    }
    await rm(otool);
    const result = Bun.spawnSync([process.execPath, join(import.meta.dir, "release-macos.ts"), "verify-package-config", bundle], { env: {
      ...process.env, PATH: directory, SPARKLE_PUBLIC_KEY_FILE: publicKeyFile,
      TEST_PRIVACY: JSON.stringify(updateInfo),
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
      ["true", "workflow_dispatch", "refs/heads/main", inputs.sha, "identical", 1],
      ["true", "workflow_dispatch", "refs/tags/v0.1.0", inputs.sha, "identical", 0],
      ["true", "push", "refs/heads/main", inputs.sha, "identical", 0],
      ["true", "push", "refs/heads/main", "b".repeat(40), "ahead", 1],
      ["false", "workflow_dispatch", "refs/heads/main", "b".repeat(40), "ahead", 0],
      ["false", "workflow_dispatch", "refs/heads/fix", "invalid", "ahead", 1],
    ] as const) {
      const result = Bun.spawnSync(["bash", "-c", script], { env: {
        ...process.env, PATH: `${directory}:${process.env.PATH}`, RELEASE_PUBLISH: publish,
        GITHUB_EVENT_NAME: event, GITHUB_REF: ref, GITHUB_SHA: inputs.sha,
        RELEASE_SHA: sha, RELEASE_TAG: inputs.tag,
        GITHUB_REPOSITORY: "fixture/huterm", TEST_MAIN_STATUS: status,
        GITHUB_OUTPUT: join(directory, "source-output"),
      } });
      expect(result.exitCode, `${publish} ${event} ${ref} ${sha}: ${result.stderr}`).toBe(expected);
    }
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("source bundle metadata and entitlements match the release contract", async () => {
  const packageVersion = await rootPackageVersion();
  const info = parseSimplePlist(await readFile(resolve(repoRoot, "assets/macos/Info.plist"), "utf8"));
  const entitlements = parseSimplePlist(await readFile(resolve(repoRoot, "assets/macos/Huterm.entitlements"), "utf8"));
  expect(info).toEqual({ CFBundleIconName: "Huterm", CFBundleVersion: packageVersion, ...privacyUsageDescriptions });
  expect(Object.keys(entitlements).sort()).toEqual([...releaseEntitlements].sort());
  expect(() => validatePrivacyDescriptions(info)).not.toThrow();
  expect(() => validateEntitlements(entitlements)).not.toThrow();
  expect(() => validatePrivacyDescriptions({ ...info, NSCameraUsageDescription: "wrong" })).toThrow("NSCameraUsageDescription");
  expect(() => validateEntitlements({ ...entitlements, "com.apple.security.cs.allow-jit": true })).toThrow("keys do not match");
  const publicKey = `${"A".repeat(43)}=`;
  expect(updaterPlistValues(publicKey)).toEqual({
    SUFeedURL: "https://github.com/jimeh/huterm/releases/latest/download/appcast.xml",
    SUPublicEDKey: publicKey,
    SURequireSignedFeed: true,
    SUVerifyUpdateBeforeExtraction: true,
  });
  const packagedInfo = {
    ...info,
    CFBundleShortVersionString: packageVersion,
    LSMinimumSystemVersion: "10.15.7",
  };
  expect(() => validateLocalPackagePlist(packagedInfo, packageVersion)).not.toThrow();
  expect(() => validateLocalPackagePlist({ ...packagedInfo, SUFeedURL: "https://example.invalid" }, packageVersion)).toThrow(
    "SUFeedURL must remain absent",
  );
  expect(() => validateUpdatePlist({
    ...packagedInfo,
    SUFeedURL: "https://github.com/jimeh/huterm/releases/latest/download/appcast.xml",
    SUPublicEDKey: publicKey,
    SURequireSignedFeed: true,
    SUVerifyUpdateBeforeExtraction: true,
  }, packageVersion, publicKey)).not.toThrow();
  expect(() => validateUpdatePlist(packagedInfo, packageVersion, publicKey)).toThrow("SUFeedURL");
});

test("macOS packaging keeps Sparkle opt-in and release-only", async () => {
  const manifest = await readFile(resolve(repoRoot, "Cargo.toml"), "utf8");
  const tasks = await readFile(resolve(repoRoot, "mise.toml"), "utf8");
  const releaseScript = await readFile(resolve(repoRoot, "scripts/release-macos.ts"), "utf8");
  expect(manifest).toContain('macos-updater = ["huterm-gpui/macos-updater"]');
  expect(manifest).not.toContain('frameworks = [".native/sparkle/distribution/Sparkle.framework"]');
  expect(tasks).toContain('[tasks."package:macos-release"]');
  expect(tasks).toContain('"bun scripts/release-macos.ts validate-updater-inputs"');
  expect(tasks).toContain('"HUTERM_MACOS_UPDATER=1 mise run package:macos:bundle"');
  expect(releaseScript).toContain('await runInherited("mise", ["run", "package:macos-release"]);');
});

test("candidate archive key comes from the exact Huterm app and matches the committed key", async () => {
  const publicKey = `${"A".repeat(43)}=`;
  const archive = "/candidate/Huterm-0.1.0-macOS-universal.zip";
  const extracted = await sparklePublicKeyFromArchive(
    archive,
    publicKey,
    async (selectedArchive, destination) => {
      expect(selectedArchive).toBe(archive);
      await mkdir(join(destination, "Huterm.app"));
    },
    async plistPath => {
      expect(plistPath.endsWith("Huterm.app/Contents/Info.plist")).toBe(true);
      return { SUPublicEDKey: publicKey };
    },
  );
  expect(extracted).toBe(publicKey);
  await expect(sparklePublicKeyFromArchive(
    archive,
    publicKey,
    async (_selectedArchive, destination) => {
      await mkdir(join(destination, "Huterm.app"));
    },
    async () => ({ SUPublicEDKey: `${"B".repeat(43)}=` }),
  )).rejects.toThrow("does not match");
});

test("nested signing plan is explicit, inside-out, and isolates helper entitlements", () => {
  expect(signingPlan("/tmp/Huterm.app")).toEqual([
    { path: "/tmp/Huterm.app/Contents/Frameworks/Sparkle.framework/Versions/B/Autoupdate", entitlements: "none" },
    { path: "/tmp/Huterm.app/Contents/Frameworks/Sparkle.framework/Versions/B/Updater.app/Contents/MacOS/Updater", entitlements: "none" },
    { path: "/tmp/Huterm.app/Contents/Frameworks/Sparkle.framework/Versions/B/Updater.app", entitlements: "none" },
    { path: "/tmp/Huterm.app/Contents/Frameworks/Sparkle.framework/Versions/B/Sparkle", entitlements: "none" },
    { path: "/tmp/Huterm.app/Contents/Frameworks/Sparkle.framework", entitlements: "none" },
    { path: "/tmp/Huterm.app/Contents/MacOS/huterm", entitlements: "huterm" },
    { path: "/tmp/Huterm.app", entitlements: "huterm" },
  ]);
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
  const releasePleaseConfig = JSON.parse(await readFile(resolve(repoRoot, ".github/release-please-config.json"), "utf8")) as {
    packages: { ".": { "extra-files": { path: string; type: string }[] } };
  };
  const macosInfo = await readFile(resolve(repoRoot, "assets/macos/Info.plist"), "utf8");
  const rootVersion = await rootPackageVersion();
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
  expect(releasePleaseConfig.packages["."]["extra-files"]).toContainEqual({
    type: "generic",
    path: "assets/macos/Info.plist",
  });
  expect(macosInfo).toContain(`<string>${rootVersion}</string> <!-- x-release-please-version -->`);
});

test("macOS CI prepares Sparkle before compiling updater smokes", async () => {
  const workflow = Bun.YAML.parse(await readFile(resolve(repoRoot, ".github/workflows/ci.yml"), "utf8")) as {
    jobs: { smoke: { steps: { name?: string; run?: string }[] } };
  };
  const steps = workflow.jobs.smoke.steps;
  const prepare = steps.findIndex(step => step.name === "Prepare desktop smoke dependencies");
  const compile = steps.findIndex(step => step.name === "Compile desktop smoke binaries");
  expect(prepare).toBeGreaterThan(-1);
  expect(compile).toBeGreaterThan(prepare);
  expect(steps[prepare]!.run).toContain("mise run sparkle:prepare");
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
  expect(releaseGuide).toContain("`SPARKLE_EDDSA_PRIVATE_KEY`");
  expect(releaseWorkflow).toContain("secrets.SPARKLE_EDDSA_PRIVATE_KEY");
  expect(releasePleaseWorkflow).not.toContain("secrets.SPARKLE_EDDSA_PRIVATE_KEY");
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
  expect(releaseWorkflow).toContain("uses: actions/upload-artifact@");
  expect(releaseWorkflow).toContain("retention-days: 7");
  expect(releaseWorkflow.slice(releaseWorkflow.indexOf("\n  publish:\n"))).toContain("if: inputs.publish");
});

test("protected publication alone receives updater signing and attestation authority", async () => {
  const workflow = await readFile(resolve(repoRoot, ".github/workflows/release.yml"), "utf8");
  const jobs = workflow.indexOf("\njobs:\n");
  const publishJob = workflow.indexOf("\n  publish:\n");
  expect(jobs).toBeGreaterThan(0);
  expect(publishJob).toBeGreaterThan(0);
  expect(workflow.slice(jobs, publishJob)).not.toContain("SPARKLE_EDDSA_PRIVATE_KEY");
  expect(workflow.slice(publishJob)).toContain("environment: release");
  expect(workflow.slice(publishJob)).toContain("SPARKLE_EDDSA_PRIVATE_KEY: ${{ secrets.SPARKLE_EDDSA_PRIVATE_KEY }}");
  expect(workflow.slice(publishJob)).toContain("artifact-id: ${{ needs.assemble.outputs.artifact_id }}");
  expect(workflow.slice(publishJob)).toContain("artifact-digest: ${{ needs.assemble.outputs.artifact_digest }}");
  for (const permission of ["artifact-metadata: write", "attestations: write", "id-token: write"]) {
    expect(workflow.slice(publishJob)).toContain(permission);
  }
  expect(workflow.slice(publishJob)).toContain("--predicate-type https://spdx.dev/Document/v2.3");
});
