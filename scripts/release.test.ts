import { expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
  assembleReleaseArtifacts,
  isDispatchedBranchBuild,
  releaseAssetNames,
  validateBuildInputs,
  validateDraftRelease,
  validateReleaseAssets,
  validateReleaseInputs,
  validateWorkspaceVersions,
  verifyLocalAssets,
  writePlatformManifest,
} from "./release.ts";

const repository = resolve(import.meta.dir, "..");
const inputs = { sha: "a".repeat(40), tag: "v0.4.0", version: "0.4.0" };

test("release inputs and workspace packages bind to one exact version", () => {
  expect(validateBuildInputs(inputs.sha, inputs.version)).toEqual({ sha: inputs.sha, version: inputs.version });
  expect(() => validateBuildInputs("abc", inputs.version)).toThrow("invalid release SHA");
  expect(() => validateBuildInputs(inputs.sha, "0.4.0-beta.1")).toThrow("invalid release version");
  expect(validateReleaseInputs(inputs.sha, inputs.tag, inputs.version)).toEqual(inputs);
  expect(() => validateReleaseInputs(inputs.sha, "v0.5.0", inputs.version)).toThrow("does not match");
  expect(() => validateReleaseInputs(inputs.sha, "v0.4.0-beta.1", "0.4.0-beta.1")).toThrow("invalid release version");
  const packages = ["huterm", "huterm-config", "huterm-core", "huterm-gpui", "huterm-protocol"].map(name => ({ name, version: inputs.version }));
  expect(() => validateWorkspaceVersions({ packages }, inputs.version)).not.toThrow();
  expect(() => validateWorkspaceVersions({ packages: packages.slice(1) }, inputs.version)).toThrow("huterm");
  expect(() => validateWorkspaceVersions({ packages: packages.map(item => item.name === "huterm-core" ? { ...item, version: "0.5.0" } : item) }, inputs.version)).toThrow("huterm-core version");
});

test("branch verification is limited to the exact non-publishing dispatch commit", () => {
  expect(isDispatchedBranchBuild(inputs.sha, "workflow_dispatch", "refs/heads/fix-release", inputs.sha)).toBe(true);
  expect(isDispatchedBranchBuild(inputs.sha, "workflow_dispatch", "refs/heads/fix-release", "b".repeat(40))).toBe(false);
  expect(isDispatchedBranchBuild(inputs.sha, "push", "refs/heads/fix-release", inputs.sha)).toBe(false);
  expect(isDispatchedBranchBuild(inputs.sha, "workflow_dispatch", "refs/tags/v0.4.0", inputs.sha)).toBe(false);
});

test("draft and remote asset validation reject widened release state", () => {
  const release = { id: 9, draft: true, prerelease: false, tag_name: inputs.tag, target_commitish: inputs.sha };
  expect(validateDraftRelease([[release]], inputs, 9)).toEqual(release);
  expect(() => validateDraftRelease([{ ...release, draft: false }], inputs)).toThrow("not a draft");
  expect(() => validateDraftRelease([{ ...release, prerelease: true }], inputs)).toThrow("prerelease");
  expect(() => validateDraftRelease([{ ...release, target_commitish: "b".repeat(40) }], inputs)).toThrow("targets");
  expect(() => validateDraftRelease([release, { ...release, id: 10 }], inputs)).toThrow("found 2");
  expect(() => validateDraftRelease([release], inputs, 10)).toThrow("does not match 10");
  const local = releaseAssetNames(inputs.version).all.map(name => ({ name, path: name, size: 10, digest: "a".repeat(64) }));
  const remote = local.map(asset => ({ name: asset.name, size: asset.size, state: "uploaded", digest: `sha256:${asset.digest}` }));
  expect(() => validateReleaseAssets(remote, local)).not.toThrow();
  expect(() => validateReleaseAssets(remote.slice(1), local)).toThrow("names do not match");
  expect(() => validateReleaseAssets(remote.map(asset => asset.name === "SHA256SUMS" ? { ...asset, state: "new" } : asset), local)).toThrow("complete upload");
  expect(() => validateReleaseAssets(remote.map(asset => asset.name === "SHA256SUMS" ? { ...asset, size: 9 } : asset), local)).toThrow("size");
  expect(() => validateReleaseAssets(remote.map(asset => asset.name === "SHA256SUMS" ? { ...asset, digest: `sha256:${"b".repeat(64)}` } : asset), local)).toThrow("digest");
  expect(() => validateReleaseAssets(remote.map(asset => asset.name === "SHA256SUMS" ? { ...asset, digest: null } : asset), local)).toThrow("digest");
});

test("release workflow binds event source, schemas, and producer-qualified artifact names", async () => {
  type Step = { name?: string; id?: string; run?: string; uses?: string; env?: Record<string, unknown>; with?: Record<string, unknown> };
  type Job = { env?: Record<string, unknown>; outputs?: Record<string, unknown>; steps: Step[] };
  const workflow = Bun.YAML.parse(await readFile(join(repository, ".github/workflows/release.yml"), "utf8")) as { env: Record<string, unknown>; jobs: Record<string, Job> };
  const preflight = workflow.jobs.preflight!;
  expect(workflow.env.RELEASE_SHA).toBe("${{ github.sha }}");
  const validationIndex = preflight.steps.findIndex(step => step.run === "bun scripts/release.ts validate-build");
  const schemaIndex = preflight.steps.findIndex(step => step.run === "mise run schema:check");
  expect(schemaIndex).toBeGreaterThan(validationIndex);

  const eventSha = "${{ github.sha }}";
  for (const jobName of ["preflight", "macos", "linux_x86_64", "linux_aarch64", "assemble", "verify_candidate", "publish"]) {
    const job = workflow.jobs[jobName]!;
    expect(job.env?.RELEASE_SHA).toBeUndefined();
    expect(job.steps.find(step => step.uses?.startsWith("actions/checkout@"))?.with?.ref).toBe(eventSha);
  }

  const releaseMutationJobs = Object.entries(workflow.jobs).filter(([, job]) =>
    job.steps.some(step => /scripts\/release\.ts (?:upload-assets|publish)/.test(step.run ?? "")),
  ).map(([name]) => name);
  expect(releaseMutationJobs).toEqual(["publish"]);

  const sha = "${{ github.sha }}";
  const actionName = (job: Job, stepName: string) => job.steps.find(step => step.name === stepName)?.with?.name;
  const producers = [
    ["macos", "release-macos"],
    ["linux_x86_64", "release-linux-x86_64"],
    ["linux_aarch64", "release-linux-aarch64"],
  ] as const;
  for (const [jobName, prefix] of producers) {
    const job = workflow.jobs[jobName]!;
    expect(job.outputs?.artifact_name).toBe("${{ steps.artifact-name.outputs.name }}");
    expect(job.steps.find(step => step.id === "artifact-name")?.env?.ARTIFACT_NAME).toBe(`${prefix}-${sha}-${"${{ github.run_attempt }}"}`);
    expect(actionName(job, `Upload verified ${jobName === "macos" ? "macOS" : "Linux"} payloads`)).toBe("${{ steps.artifact-name.outputs.name }}");
  }
  const downloadExpressions = [
    actionName(workflow.jobs.assemble!, "Download exact macOS payload"),
    actionName(workflow.jobs.assemble!, "Download exact Linux x86_64 payloads"),
    actionName(workflow.jobs.assemble!, "Download exact Linux aarch64 payloads"),
  ];
  expect(downloadExpressions).toEqual([
    "${{ needs.macos.outputs.artifact_name }}",
    "${{ needs.linux_x86_64.outputs.artifact_name }}",
    "${{ needs.linux_aarch64.outputs.artifact_name }}",
  ]);
  for (const jobName of producers.map(([name]) => name)) {
    const upload = workflow.jobs[jobName]!.steps.find(step => step.name?.startsWith("Upload verified"))!;
    expect(upload.with?.overwrite).toBe(false);
    expect(upload.with?.["retention-days"]).toBe(30);
  }

  const mixedAttemptOutputs: Record<string, string> = {
    macos: `release-macos-${inputs.sha}-1`,
    linux_x86_64: `release-linux-x86_64-${inputs.sha}-1`,
    linux_aarch64: `release-linux-aarch64-${inputs.sha}-2`,
  };
  const resolveNeedsOutput = (expression: unknown): string | undefined => {
    const jobName = /^\$\{\{ needs\.([a-z0-9_]+)\.outputs\.artifact_name \}\}$/.exec(String(expression))?.[1];
    return jobName ? mixedAttemptOutputs[jobName] : undefined;
  };
  expect(downloadExpressions.map(resolveNeedsOutput)).toEqual([
    `release-macos-${inputs.sha}-1`,
    `release-linux-x86_64-${inputs.sha}-1`,
    `release-linux-aarch64-${inputs.sha}-2`,
  ]);
});

test("publication and manual verification share repaired native candidate preparation", async () => {
  type Step = { uses?: string; run?: string; with?: Record<string, unknown> };
  type Job = { if?: string; environment?: string; permissions?: Record<string, string>; steps: Step[] };
  const workflow = Bun.YAML.parse(await readFile(join(repository, ".github/workflows/release.yml"), "utf8")) as { jobs: Record<string, Job> };
  const actionName = "./.github/actions/prepare-release-candidate";
  for (const name of ["publish", "verify_candidate"]) {
    const steps = workflow.jobs[name]?.steps ?? [];
    const preparation = steps.findIndex(step => step.uses === actionName);
    expect(preparation).toBeGreaterThan(steps.findIndex(step => step.uses?.startsWith("actions/checkout@")));
    expect(preparation).toBeGreaterThanOrEqual(0);
    expect(steps[preparation]?.with?.["artifact-id"]).toBe("${{ needs.assemble.outputs.artifact_id }}");
    expect(steps[preparation]?.with?.["artifact-digest"]).toBe("${{ needs.assemble.outputs.artifact_digest }}");
  }
  const verification = workflow.jobs.verify_candidate!;
  expect(verification.if).toBe("${{ !inputs.publish }}");
  expect(verification.environment).toBeUndefined();
  expect(verification.permissions).toEqual({ actions: "read", contents: "read" });
  expect(workflow.jobs.publish?.if).toBe("inputs.publish");
  const action = Bun.YAML.parse(await readFile(join(repository, ".github/actions/prepare-release-candidate/action.yml"), "utf8")) as { runs: { steps: Step[] } };
  const steps = action.runs.steps;
  const install = steps.findIndex(step => step.uses?.startsWith("jdx/mise-action@"));
  const repair = steps.findIndex(step => step.run === "mise run ci:toolchain");
  const validate = steps.findIndex(step => step.run === "bun scripts/release.ts validate-build");
  const download = steps.findIndex(step => step.uses?.startsWith("actions/download-artifact@"));
  const verify = steps.findIndex(step => step.run === "mise run release:verify-candidate");
  expect(install).toBeGreaterThanOrEqual(0);
  expect(repair).toBeGreaterThan(install);
  expect(validate).toBeGreaterThan(repair);
  expect(download).toBeGreaterThan(validate);
  expect(verify).toBeGreaterThan(download);
  const macos = workflow.jobs.macos!.steps;
  expect(macos.findIndex(step => step.run === "mise run ci:toolchain")).toBeGreaterThanOrEqual(0);
  expect(macos.findIndex(step => step.run === "mise run ci:toolchain")).toBeLessThan(macos.findIndex(step => step.run === "bun scripts/release.ts validate-build"));
});

test("candidate CLI rejects altered assets before native signing without requiring a tag", async () => {
  const root = await mkdtemp(join(tmpdir(), "huterm-candidate-"));
  const names = releaseAssetNames(inputs.version);
  try {
    for (const name of names.payloads) {
      await writeFile(join(root, name), name.endsWith(".schema.json")
        ? await readFile(join(repository, "schemas", name)) : name);
    }
    await writePlatformManifest(join(root, names.checksums), names.payloads.map(name => join(root, name)));
    await writeFile(join(root, names.macos), "altered archive");
    const child = Bun.spawn([process.execPath, join(repository, "scripts/release-macos.ts"), "verify-candidate"], {
      env: { ...process.env, RELEASE_SHA: inputs.sha, RELEASE_VERSION: inputs.version, RELEASE_DIST_DIR: root, RELEASE_TAG: "" },
      stdout: "pipe", stderr: "pipe",
    });
    const [status, stderr] = await Promise.all([child.exited, new Response(child.stderr).text()]);
    expect(status).toBe(1);
    expect(stderr).toContain("SHA256SUMS does not match the release payloads");
    expect(await readFile(join(root, names.macos), "utf8")).toBe("altered archive");
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("assembly verifies platform digests and creates the exact ten-file release", async () => {
  const root = await mkdtemp(join(tmpdir(), "huterm-release-assembly-"));
  const incoming = join(root, "incoming");
  const dist = join(root, "dist");
  const names = releaseAssetNames(inputs.version);
  try {
    for (const platform of names.platforms) {
      const directory = join(incoming, platform.id);
      await mkdir(directory, { recursive: true });
      const files: string[] = [];
      for (const name of platform.payloads) {
        const file = join(directory, name);
        await writeFile(file, `${name} fixture`);
        files.push(file);
      }
      await writePlatformManifest(join(directory, platform.manifest), files);
    }
    await assembleReleaseArtifacts({ sha: inputs.sha, version: inputs.version }, incoming, dist);
    const local = await verifyLocalAssets({ sha: inputs.sha, version: inputs.version }, dist);
    expect(local.map(asset => asset.name).sort()).toEqual([...names.all].sort());
    for (const name of names.payloads) expect(await readFile(join(dist, "SHA256SUMS"), "utf8")).toContain(`  ${name}\n`);

    const damaged = join(incoming, "linux-x86_64", names.platforms[1]!.payloads[0]!);
    await writeFile(damaged, "altered");
    await expect(assembleReleaseArtifacts({ sha: inputs.sha, version: inputs.version }, incoming, dist)).rejects.toThrow("digest");
  } finally { await rm(root, { recursive: true, force: true }); }
}, 10_000);

test("local release verification rejects missing, extra, empty, and changed schema assets", async () => {
  const root = await mkdtemp(join(tmpdir(), "huterm-release-assets-"));
  const names = releaseAssetNames(inputs.version);
  try {
    for (const name of names.payloads) {
      if (name.endsWith(".schema.json")) await writeFile(join(root, name), await readFile(join(repository, "schemas", name)));
      else await writeFile(join(root, name), name);
    }
    await writePlatformManifest(join(root, "SHA256SUMS"), names.payloads.map(name => join(root, name)));
    await expect(verifyLocalAssets({ sha: inputs.sha, version: inputs.version }, root)).resolves.toHaveLength(10);
    await writeFile(join(root, "extra"), "extra");
    await expect(verifyLocalAssets({ sha: inputs.sha, version: inputs.version }, root)).rejects.toThrow("missing or unexpected");
    await rm(join(root, "extra"));
    await writeFile(join(root, names.payloads[0]!), "");
    await expect(verifyLocalAssets({ sha: inputs.sha, version: inputs.version }, root)).rejects.toThrow("empty");
  } finally { await rm(root, { recursive: true, force: true }); }
});
