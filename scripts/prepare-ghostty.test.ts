import { afterEach, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileHash, otherCheckouts, prepareSource, treeHash, verifyTree, withPrepareLock } from "./prepare-ghostty.ts";

const directories: string[] = [];
function temporary(): string {
  const directory = mkdtempSync(join(tmpdir(), "huterm-prepare-"));
  directories.push(directory);
  return directory;
}
afterEach(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });

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
  await prepareSource(item, root, false);
  expect(readFileSync(join(root, "source", "file.txt"), "utf8")).toBe("reviewed");
  await prepareSource(item, root, true);
  writeFileSync(join(root, "source", "file.txt"), "tampered");
  await expect(prepareSource(item, root, false)).rejects.toThrow("source contents differ");
  expect(readFileSync(join(root, "source", "file.txt"), "utf8")).toBe("tampered");
});

test("check mode never materializes missing sources", async () => {
  const { root, item } = await fixture();
  await expect(prepareSource(item, root, true)).rejects.toThrow("missing native source");
  expect(existsSync(join(root, "source"))).toBe(false);
});

test("rejects archive checksum and tree checksum mismatches without publishing source", async () => {
  const { root, item } = await fixture();
  await expect(prepareSource({ ...item, sha256: "0".repeat(64) }, root, false)).rejects.toThrow("archive checksum mismatch");
  await expect(prepareSource({ ...item, tree_sha256: "0".repeat(64) }, root, false)).rejects.toThrow("source contents differ");
  expect(existsSync(join(root, "source"))).toBe(false);
});

test("rejects a source symlink, including a dangling one", async () => {
  const { root, item } = await fixture();
  symlinkSync("missing", join(root, "source"));
  await expect(prepareSource(item, root, false)).rejects.toThrow("source contents differ");
});

test("archive traversal cannot write outside the extraction directory", async () => {
  const { root, item } = await fixture({ "../../escaped": "bad" });
  await expect(prepareSource(item, root, false)).rejects.toThrow();
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
  await prepareSource(makeArchive(), root, false);
  expect(treeHash(join(root, "source"))).toBe(treeHash(upstream));
  rmSync(join(root, "source"), { recursive: true });
  rmSync(join(upstream, "link"));
  symlinkSync("../../outside", join(upstream, "link"));
  await expect(prepareSource(makeArchive(), root, false)).rejects.toThrow();
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
    await prepareSource(item, root, false);
    expect(fileHash(join(root, "archives", "ghostty.tar.gz"))).toBe(item.sha256);
    const corrupt = temporary();
    await expect(prepareSource({ ...item, sha256: "0".repeat(64) }, corrupt, false)).rejects.toThrow("archive checksum mismatch");
    expect(existsSync(join(corrupt, "archives", "ghostty.tar.gz"))).toBe(false);
    status = 500;
    await expect(prepareSource(item, temporary(), false)).rejects.toThrow("HTTP 500");
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
    await prepareSource(item, root, false, [temporary(), stale, other.root]);
    expect(requests).toBe(0);
    expect(fileHash(join(root, "archives", "ghostty.tar.gz"))).toBe(item.sha256);
    expect(readFileSync(join(root, "source", "file.txt"), "utf8")).toBe("reviewed");
    const fresh = temporary();
    await prepareSource(item, fresh, false, [stale]);
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
