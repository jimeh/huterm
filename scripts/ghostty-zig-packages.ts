/**
 * Regenerate the vendored Zig packages Ghostty's lib-vt build fetches.
 *
 * Prepares the pinned source and patches in a scratch directory, builds
 * lib-vt with the build script's arguments for every release target and
 * optimize mode into an empty global cache, and copies the tarballs Zig wrote
 * to its `p` directory into `third-party/ghostty-zig-packages`. It builds
 * rather than running `zig build --fetch`, because Zig fetches some lazy
 * packages, such as `pixels`, only while build steps run. Zig compresses those
 * tarballs deterministically, so rerunning on an unchanged pin changes nothing.
 * This needs the upstream package hosts; builds never do.
 */
import { cpSync, existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { createHash } from "node:crypto";
import { type Manifest, type ZigPackage, loadPatches, otherCheckouts, parseManifest, prepareSource, verifyRevision, zigPackageDirectory, zigPackageFile } from "./prepare-ghostty.ts";

const repository = resolve(import.meta.dir, "..");

/**
 * Zig targets the release builds, as `huterm-ghostty`'s build script names
 * them. Linux builds there are native, so their Zig target is the host's.
 */
export const targets = ["aarch64-macos.14.0", "x86_64-macos.14.0", "x86_64-linux-gnu", "aarch64-linux-gnu"];
/** Optimize modes Cargo and `test:ghostty-releasesafe` select. */
export const optimizeModes = ["ReleaseFast", "ReleaseSafe"];

/** The build script's Zig arguments for one target and optimize mode. */
export function buildArguments(target: string, optimize: string, install: string, localCache: string): string[] {
  return [
    "build", "-Demit-lib-vt=true", "-Dapp-runtime=none", "-Demit-xcframework=false", "-Demit-themes=false",
    `-Doptimize=${optimize}`, "-Dcpu=baseline", `-Dtarget=${target}`,
    "--prefix", install, "--cache-dir", localCache,
  ];
}

/**
 * Remote dependencies a `build.zig.zon` declares, by hash: the dependency's
 * name and the URL it is fetched from. Path dependencies have no hash.
 */
export function declaredDependencies(zon: string): Map<string, { name: string; url: string }> {
  const declared = new Map<string, { name: string; url: string }>();
  // Drop `//` comments, but not the `//` inside string literals such as URLs.
  const text = zon.replace(/"(?:[^"\\\n]|\\.)*"|\/\/[^\n]*/g, match => (match.startsWith('"') ? match : ""));
  for (const block of text.matchAll(/\.(@"[^"]+"|[A-Za-z_][A-Za-z0-9_]*)\s*=\s*\.\{([^{}]*)\}/g)) {
    const url = /\.url\s*=\s*"([^"]+)"/.exec(block[2]!)?.[1];
    const hash = /\.hash\s*=\s*"([^"]+)"/.exec(block[2]!)?.[1];
    if (url && hash) declared.set(hash, { name: block[1]!.replace(/^@"|"$/g, ""), url });
  }
  return declared;
}

/** Every `build.zig.zon` under `root`, including fetched packages in `zig-pkg`. */
function manifests(root: string): string[] {
  const found: string[] = [];
  const visit = (directory: string): void => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory() && !entry.isSymbolicLink()) visit(path);
      else if (entry.isFile() && entry.name === "build.zig.zon") found.push(path);
    }
  };
  visit(root);
  return found;
}

function run(command: string[], cwd: string, env: Record<string, string | undefined> = process.env): void {
  const result = Bun.spawnSync(command, { cwd, env, stdout: "inherit", stderr: "inherit" });
  if (result.exitCode !== 0) throw new Error(`${command.slice(0, 3).join(" ")} failed with exit code ${result.exitCode}`);
}

/** Applies the manifest's patches to `copy` as the build script does, ignoring Git configuration. */
function applyPatches(manifest: Manifest, copy: string): void {
  const env: Record<string, string | undefined> = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("GIT_")));
  Object.assign(env, { GIT_CEILING_DIRECTORIES: resolve(copy, ".."), GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null" });
  // Loading checks each patch's bytes against the manifest first.
  loadPatches(manifest.patches, repository);
  for (const patch of manifest.patches) run(["git", "apply", join(repository, patch.file)], copy, env);
}

async function regenerate(): Promise<void> {
  const manifestPath = join(import.meta.dir, "ghostty-source.json");
  const manifestBytes = readFileSync(manifestPath);
  const document = JSON.parse(manifestBytes.toString("utf8"));
  // Ignore the current package list: it may be the broken input being repaired.
  const manifest = parseManifest(Buffer.from(JSON.stringify({ ...document, zig_packages: [] })));
  verifyRevision(manifest);
  const version = Bun.spawnSync(["zig", "version"]).stdout.toString().trim();
  if (version !== manifest.zig) throw new Error(`expected Zig ${manifest.zig}, got ${version || "no zig"}`);
  const scratch = mkdtempSync(join(tmpdir(), "huterm-zig-packages-"));
  try {
    // Prepare without the vendored packages, which may be the ones being repaired.
    const reuse = [join(repository, ".native/ghostty"), ...otherCheckouts(repository).map(checkout => join(checkout, ".native/ghostty"))];
    await prepareSource(manifest.source, join(scratch, "prepared"), false, manifestBytes, reuse);
    const copy = join(scratch, "build");
    cpSync(join(scratch, "prepared", "source"), copy, { recursive: true, verbatimSymlinks: true });
    applyPatches(manifest, copy);
    const globalCache = join(scratch, "zig-global-cache");
    const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("GIT_") && name !== "ZIG_LOCAL_PKG_DIR"));
    env.GIT_CEILING_DIRECTORIES = scratch;
    // `zig build` takes the global cache only from the environment.
    env.ZIG_GLOBAL_CACHE_DIR = globalCache;
    for (const target of targets) {
      for (const optimize of optimizeModes) {
        console.log(`building for ${target} ${optimize}`);
        run(["zig", ...buildArguments(target, optimize, join(scratch, "install"), join(scratch, "zig-local-cache"))], copy, env);
      }
    }

    const declared = new Map<string, { name: string; url: string }>();
    for (const file of manifests(copy)) for (const [hash, value] of declaredDependencies(readFileSync(file, "utf8"))) declared.set(hash, value);
    const packages: ZigPackage[] = readdirSync(join(globalCache, "p"))
      .filter(name => name.endsWith(".tar.gz"))
      .map(name => {
        const zigHash = name.slice(0, -".tar.gz".length);
        const declaration = declared.get(zigHash);
        if (!declaration) throw new Error(`no build.zig.zon declares the fetched Zig package ${zigHash}`);
        const sha256 = createHash("sha256").update(readFileSync(join(globalCache, "p", name))).digest("hex");
        return { name: declaration.name, zig_hash: zigHash, url: declaration.url, sha256 };
      })
      // Code-unit order, so the manifest does not depend on the locale.
      .sort((a, b) => (a.name === b.name ? (a.zig_hash < b.zig_hash ? -1 : 1) : a.name < b.name ? -1 : 1));

    const directory = join(repository, zigPackageDirectory);
    const keep = new Set(packages.map(item => `${item.zig_hash}.tar.gz`));
    for (const name of existsSync(directory) ? readdirSync(directory) : []) {
      if (name.endsWith(".tar.gz") && !keep.has(name)) unlinkSync(join(directory, name));
    }
    for (const item of packages) cpSync(join(globalCache, "p", `${item.zig_hash}.tar.gz`), join(repository, zigPackageFile(item.zig_hash)));
    const updated = { ...document, zig_packages: packages };
    writeFileSync(manifestPath, `${JSON.stringify(updated, null, 2)}\n`);
    for (const item of packages) console.log(`${item.zig_hash} ${item.sha256} ${item.url}`);
    console.log(`vendored ${packages.length} Zig packages; run \`mise run ghostty:prepare\` to publish them`);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  const { values } = parseArgs({ args: Bun.argv.slice(2), options: { help: { type: "boolean" } } });
  if (values.help) {
    console.log("Regenerate third-party/ghostty-zig-packages and the manifest's zig_packages from upstream");
  } else {
    try {
      await regenerate();
    } catch (error) {
      console.error(`Zig package regeneration failed: ${error instanceof Error ? error.message : error}`);
      process.exitCode = 1;
    }
  }
}
