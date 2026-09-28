import { afterAll, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// huterm-ghostty's build script keeps staging and option parsing in plain
// functions, so its unit tests run without Cargo's build-script environment.
const buildScript = join(import.meta.dir, "../crates/huterm-ghostty/build.rs");
const directory = mkdtempSync(join(tmpdir(), "huterm-ghostty-build-"));
afterAll(() => rmSync(directory, { recursive: true, force: true }));

test("huterm-ghostty build script unit tests pass", () => {
  const binary = join(directory, "build-tests");
  const compile = Bun.spawnSync(["rustc", "--edition=2024", "--test", buildScript, "-o", binary]);
  expect(compile.exitCode, compile.stderr.toString()).toBe(0);

  const run = Bun.spawnSync([binary], { env: { ...process.env, RUST_TEST_THREADS: "1" } });
  const output = run.stdout.toString();
  expect(run.exitCode, `${output}\n${run.stderr.toString()}`).toBe(0);
  const summary = /test result: ok\. (\d+) passed; 0 failed/.exec(output);
  expect(summary, output).not.toBeNull();
  // A test binary that collected nothing would also report success.
  expect(Number(summary![1])).toBeGreaterThanOrEqual(7);
  for (const name of ["staging_copies_a_fresh_tree_and_leaves_the_source_unchanged", "targets_map_to_zig_and_keep_linux_host_builds_native"]) {
    expect(output).toContain(`test tests::${name} ... ok`);
  }
}, 120_000);
