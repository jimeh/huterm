import { expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

test("Ghostty build staging preserves verified source and isolates generated inputs", () => {
  const directory = mkdtempSync(join(tmpdir(), "huterm-ghostty-build-tests-"));
  try {
    const binary = join(directory, "build-tests");
    const compile = Bun.spawnSync([
      "rustc", "--edition=2024", "--test",
      join(import.meta.dir, "../third-party/vendor/libghostty-vt-sys-0.2.1/build.rs"),
      "-o", binary,
    ]);
    expect(compile.exitCode, compile.stderr.toString()).toBe(0);
    const result = Bun.spawnSync([binary, "--nocapture"]);
    expect(result.exitCode, result.stdout.toString() + result.stderr.toString()).toBe(0);
    expect(result.stdout.toString()).not.toContain("running 0 tests");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}, 30_000);
