import { afterEach, expect, spyOn, test } from "bun:test";
import * as fs from "node:fs";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { runSession, sessionStatus, withVendorLock } from "./vendor-session";
import { archiveFor, diffTrees, extract, readSources, reproduce, treeEntries, type Source } from "./vendor";

const directories: string[] = [];
const temporary = () => { const directory = mkdtempSync(join(tmpdir(), "huterm-vendor-test-")); directories.push(directory); return directory; };
afterEach(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });
const digest = (bytes: Uint8Array) => createHash("sha256").update(bytes).digest("hex");
const put = (file: string, data: string | Uint8Array) => { mkdirSync(dirname(file), { recursive: true }); writeFileSync(file, data); };

async function fixture(extra: Record<string, string> = {}) {
  const root = temporary();
  const source: Source = {
    name: "fixture", version: "1.0.0", url: "https://static.crates.io/crates/fixture/fixture-1.0.0.crate",
    sha256: "", patches: [{ name: "first", file: "patches/fixture.patch", description: "test change" }],
    upstream: { repository: "https://example.com/upstream", revision: "1".repeat(40), path: "crate" },
  };
  const files = { ".cargo_vcs_info.json": JSON.stringify({ git: { sha1: source.upstream.revision }, path_in_vcs: source.upstream.path }), "Cargo.toml": '[package]\nname = "fixture"\nversion = "1.0.0"\n', "build.rs": "original\n", "removed.txt": "old\n", "data.bin": new Uint8Array([0, 1, 2]), ...extra };
  const vendor = join(root, "fixture-1.0.0");
  for (const [name, data] of Object.entries(files)) put(join(vendor, name), data);
  const archive = join(root, "source.crate");
  await Bun.write(archive, new Bun.Archive(Object.fromEntries(Object.entries(files).map(([name, data]) => [`fixture-1.0.0/${name}`, data])), { compress: "gzip" }));
  source.sha256 = digest(readFileSync(archive));
  put(join(root, source.patches[0]!.file), "");
  return { root, source, vendor, archive };
}

async function refresh(f: Awaited<ReturnType<typeof fixture>>) {
  const original = temporary();
  await extract(f.source, f.archive, original);
  const patch = diffTrees(original, f.vendor);
  put(join(f.root, f.source.patches[0]!.file), patch!);
  return patch!;
}

for (const replacement of ["symlink", "regular file"] as const) {
  test(`tree hashing rejects a regular file replaced by a ${replacement} before opening`, () => {
    const root = temporary();
    const tree = join(root, "tree");
    const file = join(tree, "file");
    const outside = join(root, "outside");
    put(file, "original");
    put(outside, "replacement");
    const original = fs.lstatSync;
    let replaced = false;
    const stat = spyOn(fs, "lstatSync").mockImplementation(((...args: Parameters<typeof fs.lstatSync>) => {
      const result = original(...args);
      if (args[0] === file && !replaced) {
        replaced = true;
        if (replacement === "symlink") {
          rmSync(file);
          symlinkSync(outside, file);
        } else fs.renameSync(outside, file);
      }
      return result;
    }) as typeof fs.lstatSync);
    try {
      expect(() => treeEntries(tree)).toThrow(replacement === "symlink" ? "ELOOP" : "vendor file changed");
      expect(replaced).toBe(true);
    } finally { stat.mockRestore(); }
  });
}

test("archive verification rejects symlinks and non-regular files", async () => {
  const f = await fixture();
  const link = join(f.root, "link.crate");
  symlinkSync(f.archive, link);
  await expect(extract(f.source, link, temporary())).rejects.toThrow();
  await expect(extract(f.source, f.root, temporary())).rejects.toThrow("not regular");
  const fifo = join(f.root, "fifo.crate");
  expect(Bun.spawnSync(["mkfifo", fifo]).exitCode).toBe(0);
  await expect(extract(f.source, fifo, temporary())).rejects.toThrow("not regular");
});

test("extraction uses the verified bytes when the archive path is replaced after reading", async () => {
  const f = await fixture();
  const replacement = await fixture({ "build.rs": "unverified replacement\n" });
  const bytes = readFileSync(replacement.archive);
  const destination = temporary();
  const original = fs.readFileSync;
  let replaced = false;
  const read = spyOn(fs, "readFileSync").mockImplementation(((...args: Parameters<typeof fs.readFileSync>) => {
    const result = original(...args);
    // The first read belongs to archive verification, whether by path or descriptor.
    if (!replaced) {
      replaced = true;
      rmSync(f.archive);
      writeFileSync(f.archive, bytes);
    }
    return result;
  }) as typeof fs.readFileSync);
  try { await extract(f.source, f.archive, destination); }
  finally { read.mockRestore(); }
  expect(replaced).toBe(true);
  expect(readFileSync(join(destination, "build.rs"), "utf8")).toBe("original\n");
});

test("patch generation reproduces edits, additions, deletions, binary data, and executable files", async () => {
  const f = await fixture();
  put(join(f.vendor, "build.rs"), "patched\n");
  put(join(f.vendor, "LICENSE"), "license\n");
  rmSync(join(f.vendor, "removed.txt"));
  put(join(f.vendor, "data.bin"), new Uint8Array([0, 7, 2, 8]));
  chmodSync(join(f.vendor, "build.rs"), 0o755);
  symlinkSync("LICENSE", join(f.vendor, "license-link"));
  const before = treeEntries(f.vendor);
  const patch = await refresh(f);
  expect(patch).toContain("GIT binary patch");
  await reproduce(f.source, f.root, f.archive);
  expect(treeEntries(f.vendor)).toEqual(before);
  expect(await refresh(f)).toBe(patch);
});

for (const change of ["modified", "added", "removed", "executable"] as const) {
  test(`verification rejects ${change} drift without modifying source or patches`, async () => {
    const f = await fixture();
    await refresh(f);
    if (change === "modified") put(join(f.vendor, "build.rs"), "unrecorded\n");
    if (change === "added") put(join(f.vendor, ".extra"), "unrecorded\n");
    if (change === "removed") rmSync(join(f.vendor, "removed.txt"));
    if (change === "executable") chmodSync(join(f.vendor, "build.rs"), 0o755);
    const before = treeEntries(f.vendor);
    await expect(reproduce(f.source, f.root, f.archive)).rejects.toThrow("differs from archive + patch");
    expect(treeEntries(f.vendor)).toEqual(before);
    expect(readFileSync(join(f.root, f.source.patches[0]!.file), "utf8")).toBe("");
  });
}

test("stale patch fails instead of silently accepting the vendor tree", async () => {
  const f = await fixture();
  put(join(f.vendor, "build.rs"), "patched\n");
  const patch = await refresh(f);
  put(join(f.root, f.source.patches[0]!.file), patch.replace("-original", "-not-the-original"));
  await expect(reproduce(f.source, f.root, f.archive)).rejects.toThrow("git apply failed");
  expect(readFileSync(join(f.vendor, "build.rs"), "utf8")).toBe("patched\n");
});

test("checksum and crate identity must match before accepting a patch", async () => {
  const f = await fixture();
  await expect(reproduce({ ...f.source, sha256: "0".repeat(64) }, f.root, f.archive)).rejects.toThrow("checksum mismatch");
  await expect(reproduce({ ...f.source, upstream: { ...f.source.upstream, revision: "2".repeat(40) } }, f.root, f.archive)).rejects.toThrow("VCS metadata differs");
  put(join(f.vendor, "Cargo.toml"), '[package]\nname="fixture"\nversion="2.0.0"\n');
  await expect(reproduce(f.source, f.root, f.archive)).rejects.toThrow("crate identity differs");
});

test("invalid archive roots and traversal are rejected", async () => {
  const f = await fixture();
  for (const name of ["wrong-root/file", "../../escaped"]) {
    await Bun.write(f.archive, new Bun.Archive({ [name]: "bad" }, { compress: "gzip" }));
    f.source.sha256 = digest(readFileSync(f.archive));
    await expect(reproduce(f.source, f.root, f.archive)).rejects.toThrow();
  }
  expect(readFileSync(join(f.vendor, "build.rs"), "utf8")).toBe("original\n");
});

test("downloads are verified, cached, and never silently replace corrupt cache entries", async () => {
  const f = await fixture();
  let requests = 0;
  const server = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: () => { requests++; return new Response(readFileSync(f.archive)); } });
  const source = { ...f.source, url: `http://127.0.0.1:${server.port}/source.crate` };
  const cache = join(f.root, "cache");
  try {
    const archive = await archiveFor(source, cache, join(f.root, "no-cargo"));
    expect(readFileSync(archive)).toEqual(readFileSync(f.archive));
    expect(await archiveFor(source, cache, join(f.root, "no-cargo"))).toBe(archive);
    expect(requests).toBe(1);
    writeFileSync(archive, "corrupt");
    await expect(archiveFor(source, cache, join(f.root, "no-cargo"))).rejects.toThrow("checksum mismatch");
    expect(requests).toBe(1);
    await expect(archiveFor({ ...source, sha256: "0".repeat(64) }, cache, join(f.root, "no-cargo"))).rejects.toThrow("checksum mismatch");
    expect(existsSync(join(cache, `${"0".repeat(64)}.crate`))).toBe(false);
  } finally { server.stop(true); }
});

test("Cargo's archive cache is verified and reused without rewriting it", async () => {
  const f = await fixture();
  const cargo = join(f.root, "cargo");
  const archive = join(cargo, "registry/cache/example/fixture-1.0.0.crate");
  put(archive, readFileSync(f.archive));
  expect(await archiveFor(f.source, join(f.root, "cache"), cargo)).toBe(archive);
  writeFileSync(archive, "corrupt");
  await expect(archiveFor(f.source, join(f.root, "cache"), cargo)).rejects.toThrow("checksum mismatch");
  expect(readFileSync(archive, "utf8")).toBe("corrupt");
});

test("manifest rejects ambiguous sources and unsafe patch paths", async () => {
  const f = await fixture();
  const file = join(f.root, "sources.json");
  put(file, JSON.stringify({ schema: 2, sources: [f.source] }));
  expect(readSources(file)).toEqual([f.source]);
  put(file, JSON.stringify({ schema: 2, sources: [] }));
  expect(readSources(file)).toEqual([]);
  for (const sources of [[f.source, f.source], [{ ...f.source, patches: [{ ...f.source.patches[0], file: "../outside.patch" }] }], [{ ...f.source, name: "../outside" }], [{ ...f.source, url: "https://unrecorded.example/archive" }]]) {
    put(file, JSON.stringify({ schema: 2, sources }));
    expect(() => readSources(file)).toThrow("invalid or duplicate vendor");
  }
});

async function series(overlap = false, archiveFiles: Record<string, string> = {}) {
  const f = await fixture(archiveFiles);
  put(join(f.vendor, "removed.txt"), "first patch\n");
  await refresh(f);
  for (const [name, filename, contents] of [
    ["target", "build.rs", "target patch\n"],
    ["last", overlap ? "build.rs" : "last.txt", "last patch\n"],
  ]) {
    const before = temporary();
    const { cpSync } = await import("node:fs");
    cpSync(f.vendor, before, { recursive: true });
    put(join(f.vendor, filename!), contents!);
    const patch = { name: name!, file: `patches/${name}.patch`, description: name! };
    f.source.patches.push(patch);
    put(join(f.root, patch.file), diffTrees(before, f.vendor));
  }
  const storage = join(f.root, "state");
  const repo = join(storage, "sessions/fixture/repo");
  return { ...f, storage, repo, work: join(repo, "tree") };
}
const contents = (f: Awaited<ReturnType<typeof series>>) => f.source.patches.map((patch) => readFileSync(join(f.root, patch.file), "utf8"));
const action = (f: Awaited<ReturnType<typeof series>>, mode: string) => runSession(mode, f.source, f.root, f.storage, "target", f.archive);

// These tests use real archives and Git merge machinery; the source tree is the oracle.
// Process and filesystem overhead can exceed Bun's five-second default on Linux.
const sessionTest = (name: string, action: () => Promise<void>) => test(name, action, 30_000);
test("a session folds repeated build-tree edits into the target and retains later changes", async () => {
  const f = await series();
  const patches = contents(f);
  const before = treeEntries(f.vendor);
  await action(f, "start");
  expect(treeEntries(f.vendor)).toEqual(before);
  expect(sessionStatus(f.source, f.storage)).toContain("editing, target target");
  put(join(f.vendor, "build.rs"), "first attempt\n");
  put(join(f.vendor, "build.rs"), "tested fix\n");
  put(join(f.vendor, "data.bin"), new Uint8Array([0, 22, 33]));
  chmodSync(join(f.vendor, "build.rs"), 0o755);
  symlinkSync("build.rs", join(f.vendor, "new-link"));
  const desired = treeEntries(f.vendor);
  await action(f, "finish");
  expect(treeEntries(f.vendor)).toEqual(desired);
  expect(contents(f)[0]).toBe(patches[0]);
  expect(contents(f)[2]).toBe(patches[2]);
  expect(contents(f)[1]).toContain("tested fix");
  expect(readFileSync(join(f.vendor, "last.txt"), "utf8")).toBe("last patch\n");
  await reproduce(f.source, f.root, f.archive);
  expect(sessionStatus(f.source, f.storage)).toContain("no active session");
});

sessionTest("overlapping edits and later replay conflicts can be resumed without changing build inputs", async () => {
  const f = await series(true);
  const patches = contents(f);
  await action(f, "start");
  put(join(f.vendor, "build.rs"), "tested fix\n");
  const desired = treeEntries(f.vendor);
  await expect(action(f, "finish")).rejects.toThrow("Resolve target");
  expect(contents(f)).toEqual(patches);
  expect(treeEntries(f.vendor)).toEqual(desired);
  await expect(action(f, "continue")).rejects.toThrow("Resolve target");
  const { git } = await import("./vendor");
  put(join(f.work, "build.rs"), "tested fix\n");
  git(f.repo, ["add", "tree/build.rs"]);
  await expect(action(f, "continue")).rejects.toThrow("Resolve last");
  // A syntactically resolved merge that drops the tested fix must not publish.
  put(join(f.work, "build.rs"), "last patch\n");
  git(f.repo, ["add", "tree/build.rs"]);
  await expect(action(f, "continue")).rejects.toThrow("replayed stack differs from the tested vendor tree");
  expect(contents(f)).toEqual(patches);
  put(join(f.work, "build.rs"), "tested fix\n");
  await action(f, "continue");
  expect(treeEntries(f.vendor)).toEqual(desired);
  await reproduce(f.source, f.root, f.archive);
});

sessionTest("start rejects existing drift and an existing session", async () => {
  const f = await series();
  put(join(f.vendor, "build.rs"), "unowned edit\n");
  await expect(action(f, "start")).rejects.toThrow("differs from archive + patches");
  expect(sessionStatus(f.source, f.storage)).toContain("no active session");
  put(join(f.vendor, "build.rs"), "target patch\n");
  await action(f, "start");
  await expect(action(f, "start")).rejects.toThrow("Resume or cancel");
});

sessionTest("a changed recipe blocks finish, and cancel preserves source edits and recovery state", async () => {
  const f = await series();
  await action(f, "start");
  put(join(f.vendor, "build.rs"), "valuable edit\n");
  put(join(f.root, f.source.patches[2]!.file), "someone else's patch\n");
  await expect(action(f, "finish")).rejects.toThrow("patch changed during the session");
  await action(f, "cancel");
  expect(readFileSync(join(f.vendor, "build.rs"), "utf8")).toBe("valuable edit\n");
  expect(contents(f)[2]).toBe("someone else's patch\n");
  expect(sessionStatus(f.source, f.storage)).toContain("no active session");
  const { readdirSync } = await import("node:fs");
  expect(readdirSync(join(f.storage, "sessions")).some((name) => name.startsWith("fixture.cancelled-"))).toBe(true);
});

sessionTest("recipe metadata changes and edits made after finish began are not silently absorbed", async () => {
  const f = await series(true);
  await action(f, "start");
  const old = f.source.patches[0]!.description;
  f.source.patches[0]!.description = "changed ownership";
  await expect(action(f, "finish")).rejects.toThrow("manifest changed");
  f.source.patches[0]!.description = old;
  put(join(f.vendor, "build.rs"), "tested fix\n");
  await expect(action(f, "finish")).rejects.toThrow("Resolve target");
  put(join(f.vendor, "another.txt"), "later edit\n");
  const { git } = await import("./vendor");
  put(join(f.work, "build.rs"), "tested fix\n");
  git(f.repo, ["add", "tree/build.rs"]);
  await expect(action(f, "continue")).rejects.toThrow("Resolve last");
  put(join(f.work, "build.rs"), "tested fix\n");
  git(f.repo, ["add", "tree/build.rs"]);
  await expect(action(f, "continue")).rejects.toThrow("vendored source changed after finish began");
  expect(readFileSync(join(f.vendor, "another.txt"), "utf8")).toBe("later edit\n");
});

sessionTest("vendor command locking rejects a concurrent command and releases after failure", async () => {
  const storage = temporary();
  await withVendorLock(storage, "fixture", async () => {
    await expect(withVendorLock(storage, "fixture", async () => {})).rejects.toThrow("another vendor command");
  });
  await expect(withVendorLock(storage, "fixture", async () => { throw new Error("failure"); })).rejects.toThrow("failure");
  expect(await withVendorLock(storage, "fixture", async () => "released")).toBe("released");
});

sessionTest("unchanged sessions preserve every patch byte and first/last targets finish", async () => {
  const f = await series();
  const patches = contents(f);
  await action(f, "start");
  await action(f, "finish");
  expect(contents(f)).toEqual(patches);
  for (const target of ["first", "last"]) {
    await runSession("start", f.source, f.root, f.storage, target, f.archive);
    put(join(f.vendor, `${target}-addition`), `${target}\n`);
    await action(f, "finish");
    await reproduce(f.source, f.root, f.archive);
    expect(contents(f)[target === "first" ? 0 : 2]).toContain(`${target}-addition`);
  }
});

sessionTest("reopening a conflicted session keeps current edits and permits another finish", async () => {
  const f = await series(true);
  await action(f, "start");
  put(join(f.vendor, "build.rs"), "first fix\n");
  await expect(action(f, "finish")).rejects.toThrow("Resolve target");
  await action(f, "reopen");
  expect(sessionStatus(f.source, f.storage)).toContain("editing");
  put(join(f.vendor, "build.rs"), "last patch\n");
  put(join(f.vendor, "new-fix"), "second fix\n");
  await action(f, "finish");
  await reproduce(f.source, f.root, f.archive);
  expect(contents(f)[1]).toContain("new-fix");
});

sessionTest("explicit adoption assigns existing edits to the selected patch", async () => {
  const f = await series();
  put(join(f.vendor, "new-fix"), "existing edit\n");
  await expect(action(f, "start")).rejects.toThrow("differs from archive + patches");
  await runSession("start", f.source, f.root, f.storage, "target", f.archive, true);
  await action(f, "finish");
  await reproduce(f.source, f.root, f.archive);
  expect(contents(f)[1]).toContain("existing edit");
});

sessionTest("private Git snapshots preserve ignored files and attribute-sensitive bytes", async () => {
  const f = await series(false, {
    ".gitignore": "ignored.txt\n",
    ".gitattributes": "*.txt text eol=crlf export-ignore\n",
    "ignored.txt": "literal\r\nbytes\r\n",
  });
  await action(f, "start");
  put(join(f.vendor, "ignored.txt"), "updated\r\nbytes\r\n");
  await action(f, "finish");
  expect(readFileSync(join(f.vendor, "ignored.txt"), "utf8")).toBe("updated\r\nbytes\r\n");
  await reproduce(f.source, f.root, f.archive);
});

sessionTest("continue recovers when Git committed a replay step before state was saved", async () => {
  const f = await series(true);
  await action(f, "start");
  put(join(f.vendor, "build.rs"), "tested fix\n");
  await expect(action(f, "finish")).rejects.toThrow("Resolve target");
  const { git } = await import("./vendor");
  put(join(f.work, "build.rs"), "tested fix\n");
  git(f.repo, ["add", "tree/build.rs"]);
  // Model process death between Git's commit and the next state.json write.
  git(f.repo, ["commit", "--allow-empty", "-m", "resolved target"]);
  await expect(action(f, "continue")).rejects.toThrow("Resolve last");
  put(join(f.work, "build.rs"), "tested fix\n");
  git(f.repo, ["add", "tree/build.rs"]);
  await action(f, "continue");
  await reproduce(f.source, f.root, f.archive);
});

for (const recovery of ["continue", "cancel"] as const) {
  sessionTest(`partial patch publication recovers through ${recovery}`, async () => {
    const f = await series();
    await action(f, "start");
    const originals = contents(f);
    put(join(f.vendor, "build.rs"), "tested fix\n");
    const desired = treeEntries(f.vendor);
    const realRename = fs.renameSync;
    const publication = spyOn(fs, "renameSync").mockImplementation((from, to) => {
      if (String(to) === join(f.root, f.source.patches[2]!.file)) throw new Error("injected publication I/O error");
      return realRename(from, to);
    });
    try { await expect(action(f, "finish")).rejects.toThrow("injected publication I/O error"); }
    finally { publication.mockRestore(); }
    expect(contents(f)[1]).toContain("tested fix");
    expect(sessionStatus(f.source, f.storage)).toContain("publishing");
    await action(f, recovery);
    expect(treeEntries(f.vendor)).toEqual(desired);
    if (recovery === "continue") await reproduce(f.source, f.root, f.archive);
    else expect(contents(f)).toEqual(originals);
    expect(sessionStatus(f.source, f.storage)).toContain("no active session");
  });
}

sessionTest("a named empty patch can record a new fix without altering its predecessors", async () => {
  const f = await series();
  const original = contents(f);
  f.source.patches.push({ name: "new-fix", file: "patches/new-fix.patch", description: "new independent fix" });
  put(join(f.root, "patches/new-fix.patch"), "");
  await runSession("start", f.source, f.root, f.storage, "new-fix", f.archive);
  put(join(f.vendor, "new-file"), "new behavior\n");
  await action(f, "finish");
  expect(contents(f).slice(0, 3)).toEqual(original);
  expect(contents(f)[3]).toContain("new behavior");
  await reproduce(f.source, f.root, f.archive);
});

sessionTest("interrupted cleanup cannot leave a broken active session", async () => {
  const f = await series();
  await action(f, "start");
  put(join(f.vendor, "build.rs"), "tested fix\n");
  const desired = treeEntries(f.vendor);
  const realRemove = fs.rmSync;
  const cleanup = spyOn(fs, "rmSync").mockImplementation((file, options) => {
    if (String(file).includes(".finished-")) throw new Error("injected cleanup interruption");
    return realRemove(file, options);
  });
  try { await expect(action(f, "finish")).rejects.toThrow("injected cleanup interruption"); }
  finally { cleanup.mockRestore(); }
  expect(sessionStatus(f.source, f.storage)).toContain("no active session");
  expect(treeEntries(f.vendor)).toEqual(desired);
  await reproduce(f.source, f.root, f.archive);
});

sessionTest("patch verification works inside a parent Git checkout", async () => {
  const f = await series();
  const { git } = await import("./vendor");
  git(f.root, ["init", "--quiet", "--template="]);
  const indexBefore = git(f.root, ["ls-files", "--stage"]);
  await action(f, "start");
  put(join(f.vendor, "build.rs"), "nested checkout fix\n");
  await expect(action(f, "finish")).resolves.toBeUndefined();
  expect(contents(f)[1]).toContain("nested checkout fix");
  expect(git(f.root, ["ls-files", "--stage"])).toBe(indexBefore);
  await reproduce(f.source, f.root, f.archive);
});
