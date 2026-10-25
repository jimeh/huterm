import { afterEach, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, symlinkSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileHash, loadPatches, otherCheckouts, parsePatches, patchPaths, prepareSource, treeHash, verifyTree, withPrepareLock } from "./prepare-ghostty.ts";

const directories: string[] = [];
function temporary(): string {
  const directory = mkdtempSync(join(tmpdir(), "huterm-prepare-"));
  directories.push(directory);
  return directory;
}
afterEach(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });
// Preparation records these bytes beside a verified tree.
const manifest = Buffer.from('{"revision": "fixture"}\n');

test("tree hash retains top-down ordering and hashes symlink targets without following them", () => {
  const root = temporary();
  mkdirSync(join(root, "a"));
  writeFileSync(join(root, "z"), "outer");
  writeFileSync(join(root, "a", "inner"), "inner");
  symlinkSync("a", join(root, "link"));
  const hash = (text: string) => createHash("sha256").update(text).digest("hex");
  const expected = hash(`link\0link:a\0z\0file:${hash("outer")}\0a/inner\0file:${hash("inner")}\0`);
  expect(treeHash(root)).toBe(expected);
  writeFileSync(join(root, "z"), "changed");
  expect(() => verifyTree(root, expected)).toThrow("source contents differ");
});

async function fixture(entries: Record<string, string> = { "upstream/file.txt": "reviewed" }) {
  const root = temporary();
  mkdirSync(join(root, "archives"));
  const archive = join(root, "archives", "ghostty.tar.gz");
  await Bun.write(archive, new Bun.Archive(entries, { compress: "gzip" }));
  const tree = temporary();
  writeFileSync(join(tree, "file.txt"), "reviewed");
  return { root, archive, item: { name: "ghostty", url: "https://invalid.example/not-fetched", sha256: fileHash(archive), tree_sha256: treeHash(tree) } };
}

test("extracts verified archive then verifies retained source without downloading", async () => {
  const { root, item } = await fixture();
  await prepareSource(item, root, false, manifest);
  expect(readFileSync(join(root, "source", "file.txt"), "utf8")).toBe("reviewed");
  await prepareSource(item, root, true, manifest);
  writeFileSync(join(root, "source", "file.txt"), "tampered");
  await expect(prepareSource(item, root, false, manifest)).rejects.toThrow("source contents differ");
  expect(readFileSync(join(root, "source", "file.txt"), "utf8")).toBe("tampered");
});

test("records the manifest beside freshly extracted source", async () => {
  const { root, item } = await fixture();
  await prepareSource(item, root, false, manifest);
  expect(readFileSync(join(root, "source.manifest.json"))).toEqual(manifest);
  expect(readdirSync(root).sort()).toEqual(["archives", "patches", "source", "source.manifest.json"]);
  // Builds read the series even when the manifest lists no patches.
  expect(readdirSync(join(root, "patches"))).toEqual(["series"]);
  expect(readFileSync(join(root, "patches", "series"), "utf8")).toBe("");
});

test("creates or refreshes the stamp only when the retained source verifies", async () => {
  const { root, item } = await fixture();
  await prepareSource(item, root, false, manifest);
  const stamp = join(root, "source.manifest.json");
  rmSync(stamp);
  await prepareSource(item, root, false, manifest);
  expect(readFileSync(stamp)).toEqual(manifest);
  // A manifest edit that keeps the pin, such as a new package entry.
  const edited = Buffer.from('{"revision": "fixture", "packages": []}\n');
  await prepareSource(item, root, false, edited);
  expect(readFileSync(stamp)).toEqual(edited);
  writeFileSync(join(root, "source", "file.txt"), "tampered");
  await expect(prepareSource(item, root, false, manifest)).rejects.toThrow("source contents differ");
  expect(readFileSync(stamp)).toEqual(edited);
});

test("leaves a matching stamp and its mtime untouched", async () => {
  const { root, item } = await fixture();
  await prepareSource(item, root, false, manifest);
  const stamp = join(root, "source.manifest.json");
  const past = new Date("2026-01-01T00:00:00Z");
  utimesSync(stamp, past, past);
  await prepareSource(item, root, false, manifest);
  expect(statSync(stamp).mtimeMs).toBe(past.getTime());
});

test("check mode never writes the stamp", async () => {
  const { root, item } = await fixture();
  await prepareSource(item, root, false, manifest);
  const stamp = join(root, "source.manifest.json");
  rmSync(stamp);
  await prepareSource(item, root, true, manifest);
  expect(readdirSync(root)).not.toContain("source.manifest.json");
  writeFileSync(stamp, "stale");
  await prepareSource(item, root, true, manifest);
  expect(readFileSync(stamp, "utf8")).toBe("stale");
});

const patchText = (path: string) => `diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n@@ -1 +1 @@\n-old\n+new\n`;
const patch = (name: string, path = "src/file.zig") => ({ name, bytes: Buffer.from(patchText(path)) });

test("validates manifest patch entries", () => {
  const entry = { name: "fix", file: "third-party/ghostty-patches/fix.patch", sha256: "a".repeat(64), description: "Fixes it." };
  expect(parsePatches([entry, { ...entry, name: "other", file: "third-party/ghostty-patches/other.patch", upstream: "https://example.invalid/1" }])).toHaveLength(2);
  for (const [invalid, message] of [
    [{ ...entry, name: "Fix" }, "invalid Ghostty patch name"],
    [{ ...entry, file: "third-party/ghostty/fix.patch" }, "must be third-party/ghostty-patches/fix.patch"],
    [{ ...entry, file: "third-party/ghostty-patches/../fix.patch" }, "must be third-party/ghostty-patches/fix.patch"],
    [{ ...entry, sha256: "A".repeat(64) }, "invalid sha256"],
    [{ ...entry, description: " " }, "needs a description"],
    [{ ...entry, upstream: "http://example.invalid" }, "https URL"],
  ] as const) {
    expect(() => parsePatches([invalid])).toThrow(message);
  }
  expect(() => parsePatches([entry, entry])).toThrow("duplicate Ghostty patch: fix");
  expect(() => parsePatches(undefined)).toThrow("invalid Ghostty patch list");
});

test("patch paths come from file headers, never from hunk bodies", () => {
  const text = [
    "diff --git a/src/a.zig b/src/a.zig",
    "--- a/src/a.zig",
    "+++ b/src/a.zig",
    "@@ -1,2 +1,2 @@",
    "--- a/include/removed-line-not-a-header",
    "+++ b/include/added-line-not-a-header",
    " context",
    "\\ No newline at end of file",
    "diff --git a/src/new.zig b/src/new.zig",
    "new file mode 100644",
    "--- /dev/null",
    "+++ b/src/new.zig",
    "@@ -0,0 +1 @@",
    "+new",
    "",
  ].join("\n");
  expect(patchPaths(text).sort()).toEqual(["src/a.zig", "src/new.zig"]);
  expect(() => patchPaths(patchText("include/ghostty/vt.h"))).toThrow("under include/");
  for (const path of ["../outside", "src/../../outside", "/absolute", "./src/file.zig"]) {
    expect(() => patchPaths(patchText(path)), path).toThrow("escapes the Ghostty source");
  }
  expect(() => patchPaths("--- a/src/x\n+++ b/src/x\nGIT binary patch\n")).toThrow("binary patches");
  expect(() => patchPaths("not a patch\n")).toThrow("touches no files");
});

test("loads patches only when their bytes match the manifest", () => {
  const root = temporary();
  mkdirSync(join(root, "third-party/ghostty-patches"), { recursive: true });
  const file = "third-party/ghostty-patches/fix.patch";
  writeFileSync(join(root, file), patchText("src/file.zig"));
  const entry = { name: "fix", file, sha256: fileHash(join(root, file)), description: "Fixes it." };
  expect(loadPatches([entry], root)).toEqual([{ name: "fix", bytes: readFileSync(join(root, file)) }]);
  expect(() => loadPatches([{ ...entry, sha256: "0".repeat(64) }], root)).toThrow("checksum mismatch");
  writeFileSync(join(root, file), patchText("include/ghostty/vt.h"));
  expect(() => loadPatches([{ ...entry, sha256: fileHash(join(root, file)) }], root)).toThrow("invalid Ghostty patch fix");
  rmSync(join(root, file));
  expect(() => loadPatches([entry], root)).toThrow("cannot read Ghostty patch fix");
});

test("publishes verified patch copies and their order, removing stale copies", async () => {
  const { root, item } = await fixture();
  await prepareSource(item, root, false, manifest, [], [patch("second"), patch("first")]);
  const patches = join(root, "patches");
  expect(readdirSync(patches).sort()).toEqual(["first.patch", "second.patch", "series"]);
  expect(readFileSync(join(patches, "series"), "utf8")).toBe("second.patch\nfirst.patch\n");
  expect(readFileSync(join(patches, "first.patch"), "utf8")).toBe(patchText("src/file.zig"));
  await prepareSource(item, root, true, manifest, [], [patch("second"), patch("first")]);

  // The retained-source path replaces the set, as after a manifest edit.
  writeFileSync(join(patches, "leftover"), "stale");
  await prepareSource(item, root, false, manifest, [], [patch("first", "src/other.zig")]);
  expect(readdirSync(patches).sort()).toEqual(["first.patch", "series"]);
  expect(readFileSync(join(patches, "series"), "utf8")).toBe("first.patch\n");
  expect(readFileSync(join(patches, "first.patch"), "utf8")).toBe(patchText("src/other.zig"));
  expect(readFileSync(join(root, "source.manifest.json"))).toEqual(manifest);
  // No staging directories remain.
  expect(readdirSync(root).filter(name => name.startsWith("."))).toEqual([]);
});

test("repeated preparation leaves published patches, the series, and the stamp untouched", async () => {
  const { root, item } = await fixture();
  const patches = [patch("fix")];
  await prepareSource(item, root, false, manifest, [], patches);
  const files = ["patches/fix.patch", "patches/series", "source.manifest.json"].map(name => join(root, name));
  const past = new Date("2026-01-01T00:00:00Z");
  for (const file of files) utimesSync(file, past, past);
  await prepareSource(item, root, false, manifest, [], patches);
  for (const file of files) expect(statSync(file).mtimeMs, file).toBe(past.getTime());
  // A changed set rewrites the stamp too, so Cargo reruns the build script.
  await prepareSource(item, root, false, manifest, [], []);
  expect(statSync(join(root, "source.manifest.json")).mtimeMs).not.toBe(past.getTime());
  expect(readFileSync(join(root, "patches", "series"), "utf8")).toBe("");
});

test("check mode compares published patches without writing", async () => {
  const { root, item } = await fixture();
  await prepareSource(item, root, false, manifest, [], [patch("fix")]);
  const series = join(root, "patches", "series");
  writeFileSync(series, "");
  await expect(prepareSource(item, root, true, manifest, [], [patch("fix")])).rejects.toThrow("published Ghostty patches differ from the manifest (series)");
  expect(readFileSync(series, "utf8")).toBe("");
  rmSync(join(root, "patches"), { recursive: true });
  await expect(prepareSource(item, root, true, manifest)).rejects.toThrow("(series)");
  expect(existsSync(join(root, "patches"))).toBe(false);
});

test("check mode never materializes missing sources", async () => {
  const { root, item } = await fixture();
  await expect(prepareSource(item, root, true, manifest)).rejects.toThrow("missing native source");
  expect(existsSync(join(root, "source"))).toBe(false);
});

test("rejects archive checksum and tree checksum mismatches without publishing source", async () => {
  const { root, item } = await fixture();
  await expect(prepareSource({ ...item, sha256: "0".repeat(64) }, root, false, manifest)).rejects.toThrow("archive checksum mismatch");
  await expect(prepareSource({ ...item, tree_sha256: "0".repeat(64) }, root, false, manifest)).rejects.toThrow("source contents differ");
  expect(existsSync(join(root, "source"))).toBe(false);
});

test("rejects a source symlink, including a dangling one", async () => {
  const { root, item } = await fixture();
  symlinkSync("missing", join(root, "source"));
  await expect(prepareSource(item, root, false, manifest)).rejects.toThrow("source contents differ");
});

test("archive traversal cannot write outside the extraction directory", async () => {
  const { root, item } = await fixture({ "../../escaped": "bad" });
  await expect(prepareSource(item, root, false, manifest)).rejects.toThrow();
  expect(existsSync(join(root, "escaped"))).toBe(false);
  expect(existsSync(join(root, "source"))).toBe(false);
});

test("lock releases after an action throws", async () => {
  const root = temporary();
  await expect(withPrepareLock(root, async () => { throw new Error("action failed"); })).rejects.toThrow("action failed");
  expect(await withPrepareLock(root, async () => "reacquired")).toBe("reacquired");
});

test("lock serializes processes and releases after SIGKILL", async () => {
  const root = temporary();
  const module = JSON.stringify(join(import.meta.dir, "prepare-ghostty.ts"));
  const holder = Bun.spawn([process.execPath, "-e", `import {withPrepareLock} from ${module}; await withPrepareLock(${JSON.stringify(root)}, async () => { console.log("held"); await Bun.stdin.text(); });`], { stdin: "pipe", stdout: "pipe", stderr: "pipe" });
  let waiter: ReturnType<typeof Bun.spawn> | undefined;
  try {
    const ready = holder.stdout.getReader();
    expect(new TextDecoder().decode((await ready.read()).value)).toContain("held");
    waiter = Bun.spawn([process.execPath, "-e", `import {withPrepareLock} from ${module}; console.log("waiting"); await withPrepareLock(${JSON.stringify(root)}, async () => { console.log("acquired"); });`], { stdout: "pipe", stderr: "pipe" });
    if (typeof waiter.stdout === "number" || !waiter.stdout) throw new Error("missing waiter output");
    const output = waiter.stdout.getReader();
    expect(new TextDecoder().decode((await output.read()).value)).toBe("waiting\n");
    const acquired = output.read();
    expect(await Promise.race([acquired.then(() => "early"), Bun.sleep(100).then(() => "blocked")])).toBe("blocked");
    holder.kill("SIGKILL");
    await holder.exited;
    expect(new TextDecoder().decode((await acquired).value)).toContain("acquired");
    expect(await waiter.exited).toBe(0);
  } finally {
    holder.kill("SIGKILL");
    waiter?.kill("SIGKILL");
    await holder.exited;
    if (waiter) await waiter.exited;
  }
});

test("preserves safe archive symlinks and rejects escaping targets", async () => {
  const root = temporary();
  const input = temporary();
  const upstream = join(input, "upstream");
  mkdirSync(upstream);
  writeFileSync(join(upstream, "file"), "reviewed");
  symlinkSync("file", join(upstream, "link"));
  mkdirSync(join(root, "archives"));
  const archive = join(root, "archives", "ghostty.tar.gz");
  const makeArchive = () => {
    const result = Bun.spawnSync(["tar", "-czf", archive, "-C", input, "upstream"]);
    expect(result.exitCode).toBe(0);
    return { name: "ghostty", url: "https://invalid.example", sha256: fileHash(archive), tree_sha256: treeHash(upstream) };
  };
  await prepareSource(makeArchive(), root, false, manifest);
  expect(treeHash(join(root, "source"))).toBe(treeHash(upstream));
  rmSync(join(root, "source"), { recursive: true });
  rmSync(join(upstream, "link"));
  symlinkSync("../../outside", join(upstream, "link"));
  await expect(prepareSource(makeArchive(), root, false, manifest)).rejects.toThrow();
  expect(existsSync(join(root, "source"))).toBe(false);
  expect(existsSync(join(root, "outside"))).toBe(false);
});

test("downloads verified bytes and rejects HTTP and checksum failures", async () => {
  const fixtureData = await fixture();
  const bytes = readFileSync(fixtureData.archive);
  let status = 200;
  const server = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: () => new Response(bytes, { status }) });
  try {
    const item = { ...fixtureData.item, url: server.url.toString() };
    const root = temporary();
    await prepareSource(item, root, false, manifest);
    expect(fileHash(join(root, "archives", "ghostty.tar.gz"))).toBe(item.sha256);
    const corrupt = temporary();
    await expect(prepareSource({ ...item, sha256: "0".repeat(64) }, corrupt, false, manifest)).rejects.toThrow("archive checksum mismatch");
    expect(existsSync(join(corrupt, "archives", "ghostty.tar.gz"))).toBe(false);
    status = 500;
    await expect(prepareSource(item, temporary(), false, manifest)).rejects.toThrow("HTTP 500");
  } finally {
    server.stop(true);
  }
});

test("reuses another checkout's archive only when it matches the pin", async () => {
  const other = await fixture();
  const bytes = readFileSync(other.archive);
  let requests = 0;
  const server = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: () => { requests += 1; return new Response(bytes); } });
  try {
    const item = { ...other.item, url: server.url.toString() };
    // A checkout on another pin holds different bytes under the same name.
    const stale = temporary();
    mkdirSync(join(stale, "archives"));
    writeFileSync(join(stale, "archives", "ghostty.tar.gz"), "another pin");
    const root = temporary();
    await prepareSource(item, root, false, manifest, [temporary(), stale, other.root]);
    expect(requests).toBe(0);
    expect(fileHash(join(root, "archives", "ghostty.tar.gz"))).toBe(item.sha256);
    expect(readFileSync(join(root, "source", "file.txt"), "utf8")).toBe("reviewed");
    // An archive that cannot be read is skipped like any other miss.
    const unreadable = temporary();
    mkdirSync(join(unreadable, "archives"));
    writeFileSync(join(unreadable, "archives", "ghostty.tar.gz"), "denied");
    chmodSync(join(unreadable, "archives", "ghostty.tar.gz"), 0o000);
    const fresh = temporary();
    await prepareSource(item, fresh, false, manifest, [unreadable, stale]);
    expect(requests).toBe(1);
    expect(fileHash(join(fresh, "archives", "ghostty.tar.gz"))).toBe(item.sha256);
  } finally {
    server.stop(true);
  }
});

test("lists the repository's other checkouts with the primary first", () => {
  const primary = realpathSync(temporary());
  // Commit hooks export GIT_* paths that would redirect the fixture's Git
  // into the repository being committed.
  const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("GIT_")));
  const git = (...args: string[]) => {
    // Inherited signing and hooks must not affect the fixture's own commits.
    const result = Bun.spawnSync(["git", "-c", "user.name=test", "-c", "user.email=test@invalid.example", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", ...args], { cwd: primary, env, stderr: "pipe" });
    expect(result.exitCode, result.stderr.toString()).toBe(0);
  };
  const parent = realpathSync(temporary());
  const [first, second] = [join(parent, "first"), join(parent, "second")];
  git("init", "--quiet");
  git("commit", "--quiet", "--allow-empty", "--message", "fixture");
  git("worktree", "add", "--quiet", "--detach", first);
  git("worktree", "add", "--quiet", "--detach", second);
  expect(otherCheckouts(second)).toEqual([primary, first]);
  expect(otherCheckouts(primary)).toEqual([first, second]);
  expect(otherCheckouts(realpathSync(temporary()))).toEqual([]);
});
