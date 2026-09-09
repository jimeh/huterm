import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

test("Cargo isolates Ghostty from the enclosing release tag while preserving root Git discovery", () => {
  const root = mkdtempSync(join(tmpdir(), "huterm-release-git-"));
  try {
    mkdirSync(join(root, ".cargo"));
    mkdirSync(join(root, ".native/ghostty/source"), { recursive: true });
    mkdirSync(join(root, "src"));
    writeFileSync(join(root, ".cargo/config.toml"), readFileSync(join(import.meta.dir, "../.cargo/config.toml")));
    writeFileSync(join(root, "Cargo.toml"), '[package]\nname = "ghostty-git-fixture"\nversion = "0.1.0"\nedition = "2024"\n');
    writeFileSync(join(root, "src/lib.rs"), "");
    writeFileSync(join(root, "build.rs"), `
use std::process::Command;
fn main() {
    let root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let native = std::env::var("GHOSTTY_SOURCE_DIR").unwrap();
    let describe = |cwd: &str| Command::new("git").current_dir(cwd)
        .args(["describe", "--exact-match", "--tags"]).output().unwrap();
    let parent = describe(&root);
    assert!(parent.status.success(), "root Git discovery must remain available");
    assert_eq!(String::from_utf8_lossy(&parent.stdout).trim(), "v0.1.1");
    assert!(!describe(&native).status.success(), "Ghostty must not inherit the enclosing release tag");
}
`);
    const env: NodeJS.ProcessEnv = { ...process.env, CARGO_TARGET_DIR: join(root, "target") };
    // Hooks export Git paths that would redirect fixture writes into the caller.
    for (const key of Object.keys(env)) if (key.startsWith("GIT_")) delete env[key];
    env.GIT_CEILING_DIRECTORIES = tmpdir();
    for (const args of [["init", "--quiet"], ["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "commit", "--quiet", "--allow-empty", "-m", "fixture"], ["-c", "tag.gpgsign=false", "tag", "v0.1.1"]]) {
      const result = Bun.spawnSync(["git", ...args], { cwd: root, env });
      expect(result.exitCode, result.stderr.toString()).toBe(0);
    }
    const result = Bun.spawnSync(["cargo", "check", "--offline", "--quiet"], { cwd: root, env });
    expect(result.exitCode, result.stderr.toString()).toBe(0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}, 30_000);
