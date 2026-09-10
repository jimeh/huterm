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

test("release workflow binds validated source, schemas, and rerun-safe artifact names", async () => {
  type Step = { name?: string; id?: string; run?: string; uses?: string; with?: Record<string, unknown> };
  type Job = { env?: Record<string, unknown>; outputs?: Record<string, unknown>; steps: Step[] };
  const workflow = Bun.YAML.parse(await readFile(join(repository, ".github/workflows/release.yml"), "utf8")) as { jobs: Record<string, Job> };
  const preflight = workflow.jobs.preflight!;
  expect(preflight.outputs?.validated_sha).toBe("${{ steps.source.outputs.validated_sha }}");
  const validationIndex = preflight.steps.findIndex(step => step.run === "bun scripts/release.ts validate-build");
  const sourceIndex = preflight.steps.findIndex(step => step.id === "source");
  const schemaIndex = preflight.steps.findIndex(step => step.run === "mise run schema:check");
  expect(schemaIndex).toBeGreaterThan(validationIndex);
  expect(sourceIndex).toBeGreaterThan(schemaIndex);

  const validatedSha = "${{ needs.preflight.outputs.validated_sha }}";
  for (const jobName of ["macos", "linux", "assemble"]) {
    const job = workflow.jobs[jobName]!;
    expect(job.env?.RELEASE_SHA).toBe(validatedSha);
    expect(job.steps.find(step => step.uses?.startsWith("actions/checkout@"))?.with?.ref).toBe(validatedSha);
  }

  const releaseMutationJobs = Object.entries(workflow.jobs).filter(([, job]) =>
    job.steps.some(step => /scripts\/release\.ts (?:upload-assets|publish)/.test(step.run ?? "")),
  ).map(([name]) => name);
  expect(releaseMutationJobs).toEqual(["assemble"]);

  const sha = "${{ needs.preflight.outputs.validated_sha }}";
  const attempt = "${{ github.run_attempt }}";
  const actionName = (job: Job, stepName: string) => job.steps.find(step => step.name === stepName)?.with?.name;
  expect(actionName(workflow.jobs.macos!, "Upload verified macOS payload")).toBe(`release-macos-${sha}-${attempt}`);
  expect(actionName(workflow.jobs.assemble!, "Download exact macOS payload")).toBe(`release-macos-${sha}-${attempt}`);
  expect(actionName(workflow.jobs.linux!, "Upload verified Linux payloads")).toBe(`release-linux-${"${{ matrix.arch }}"}-${sha}-${attempt}`);
  expect(actionName(workflow.jobs.assemble!, "Download exact Linux x86_64 payloads")).toBe(`release-linux-x86_64-${sha}-${attempt}`);
  expect(actionName(workflow.jobs.assemble!, "Download exact Linux aarch64 payloads")).toBe(`release-linux-aarch64-${sha}-${attempt}`);
  for (const jobName of ["macos", "linux"]) {
    const upload = workflow.jobs[jobName]!.steps.find(step => step.name?.startsWith("Upload verified"))!;
    expect(upload.with?.overwrite).toBe(false);
  }
});

test("assembly verifies platform digests and creates the exact eight-file release", async () => {
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
    await expect(verifyLocalAssets({ sha: inputs.sha, version: inputs.version }, root)).resolves.toHaveLength(8);
    await writeFile(join(root, "extra"), "extra");
    await expect(verifyLocalAssets({ sha: inputs.sha, version: inputs.version }, root)).rejects.toThrow("missing or unexpected");
    await rm(join(root, "extra"));
    await writeFile(join(root, names.payloads[0]!), "");
    await expect(verifyLocalAssets({ sha: inputs.sha, version: inputs.version }, root)).rejects.toThrow("empty");
  } finally { await rm(root, { recursive: true, force: true }); }
});
