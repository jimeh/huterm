import { expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
  assembleReleaseArtifacts,
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
  expect(validateReleaseInputs(inputs.sha, inputs.tag, inputs.version)).toEqual(inputs);
  expect(() => validateReleaseInputs(inputs.sha, "v0.5.0", inputs.version)).toThrow("does not match");
  const packages = ["huterm", "huterm-config", "huterm-core", "huterm-gpui", "huterm-protocol"].map(name => ({ name, version: inputs.version }));
  expect(() => validateWorkspaceVersions({ packages }, inputs.version)).not.toThrow();
  expect(() => validateWorkspaceVersions({ packages: packages.slice(1) }, inputs.version)).toThrow("huterm");
});

test("draft and remote asset validation reject widened release state", () => {
  const release = { id: 9, draft: true, prerelease: false, tag_name: inputs.tag, target_commitish: inputs.sha };
  expect(validateDraftRelease([[release]], inputs, 9)).toEqual(release);
  expect(() => validateDraftRelease([{ ...release, draft: false }], inputs)).toThrow("not a draft");
  const local = releaseAssetNames(inputs.version).all.map(name => ({ name, path: name, size: 10, digest: "a".repeat(64) }));
  const remote = local.map(asset => ({ name: asset.name, size: asset.size, state: "uploaded", digest: `sha256:${asset.digest}` }));
  expect(() => validateReleaseAssets(remote, local)).not.toThrow();
  expect(() => validateReleaseAssets(remote.slice(1), local)).toThrow("names do not match");
  expect(() => validateReleaseAssets(remote.map(asset => asset.name === "SHA256SUMS" ? { ...asset, digest: null } : asset), local)).toThrow("digest");
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
