import { afterEach, expect, test } from "bun:test";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { affectedTests, SLOW_SUITES } from "./affected-tests.ts";

const roots: string[] = [];
afterEach(() => { for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "huterm-affected-tests-"));
  roots.push(root);
  const files: Record<string, string> = {
    "scripts/shared.ts": `import { cycle } from "./cycle.ts";\nexport const shared = cycle;\n`,
    "scripts/cycle.ts": `import "./shared";\nexport const cycle = 1;\n`,
    "scripts/tool.ts": `import { shared } from "./shared";\nexport const tool = shared;\n`,
    "scripts/tool.test.ts": `import { tool } from "./tool.ts";\n`,
    "scripts/spawn.test.ts": `Bun.spawnSync(["bash", "scripts/vm/run.sh"]);\n`,
    "scripts/vm/run.sh": "true\n",
    "scripts/workflow.test.ts": `const file = ".github/workflows/ci.yml";\n`,
    "scripts/unrelated.test.ts": "export {};\n",
  };
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(join(root, path, ".."), { recursive: true });
    writeFileSync(join(root, path), text);
  }
  return root;
}

test("tests follow transitive imports, including import cycles", () => {
  expect(affectedTests(fixture(), ["scripts/cycle.ts"])).toEqual(["scripts/tool.test.ts"]);
});

test("tests that name a script or an outside path are selected", () => {
  const root = fixture();
  expect(affectedTests(root, ["scripts/vm/run.sh"])).toEqual(["scripts/spawn.test.ts"]);
  expect(affectedTests(root, [".github/workflows/ci.yml"])).toEqual(["scripts/workflow.test.ts"]);
  expect(affectedTests(root, ["crates/core/src/lib.rs"])).toEqual([]);
});

test("Bun package and compiler configuration selects every test", () => {
  expect(affectedTests(fixture(), ["tsconfig.json"])).toEqual([
    "scripts/spawn.test.ts",
    "scripts/tool.test.ts",
    "scripts/unrelated.test.ts",
    "scripts/workflow.test.ts",
  ]);
});

test("fast mode skips slow suites and never selects every test", () => {
  const root = fixture();
  writeFileSync(join(root, "scripts/vendor.test.ts"), `import { tool } from "./tool.ts";\n`);
  expect(affectedTests(root, ["scripts/tool.ts"])).toEqual(["scripts/tool.test.ts", "scripts/vendor.test.ts"]);
  expect(affectedTests(root, ["scripts/tool.ts"], { fast: true })).toEqual(["scripts/tool.test.ts"]);
  expect(affectedTests(root, ["bun.lock", ".github/workflows/ci.yml"], { fast: true })).toEqual(["scripts/workflow.test.ts"]);
});

test("every slow suite still exists", () => {
  for (const suite of SLOW_SUITES) expect(existsSync(join(import.meta.dir, "..", suite)), suite).toBe(true);
});

test("fast mode still selects tests that read a Bun configuration file", () => {
  const root = fixture();
  writeFileSync(join(root, "scripts/cooldown.test.ts"), `const config = "bunfig.toml";\n`);
  expect(affectedTests(root, ["bunfig.toml"], { fast: true })).toEqual(["scripts/cooldown.test.ts"]);
});

test("deleting a script selects the tests that still import or name it", () => {
  const root = fixture();
  rmSync(join(root, "scripts/tool.ts"));
  rmSync(join(root, "scripts/vm/run.sh"));
  expect(affectedTests(root, ["scripts/tool.ts"])).toEqual(["scripts/tool.test.ts"]);
  expect(affectedTests(root, ["scripts/vm/run.sh"])).toEqual(["scripts/spawn.test.ts"]);
});

test("an extensionless import of a deleted script still selects its tests", () => {
  const root = fixture();
  // tool.ts imports "./shared" without naming shared.ts, so only the kept
  // import edge can connect the deletion to tool.test.ts.
  rmSync(join(root, "scripts/shared.ts"));
  expect(affectedTests(root, ["scripts/shared.ts"])).toEqual(["scripts/tool.test.ts"]);
});

test("--staged reads staged deletions and both sides of a rename from Git", () => {
  const root = mkdtempSync(join(tmpdir(), "huterm-affected-staged-"));
  roots.push(root);
  mkdirSync(join(root, "scripts"));
  copyFileSync(join(import.meta.dir, "affected-tests.ts"), join(root, "scripts/affected-tests.ts"));
  const passing = `import { test } from "bun:test";\ntest("ok", () => {});\n`;
  writeFileSync(join(root, "scripts/gone.sh"), "true\n");
  writeFileSync(join(root, "scripts/gone.test.ts"), `${passing}// runs gone.sh\n`);
  writeFileSync(join(root, "scripts/old-name.ts"), "export const value = 1;\n");
  writeFileSync(join(root, "scripts/renamed.test.ts"), `${passing}// reads old-name.ts\n`);
  writeFileSync(join(root, "scripts/other.test.ts"), passing);
  // Commit hooks export GIT_* paths that would redirect a fixture's Git.
  const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("GIT_")));
  const git = (...args: string[]) => {
    // Inherited signing and hooks must not affect the fixture's own commits.
    const result = Bun.spawnSync(["git", "-c", "user.email=test@example.com", "-c", "user.name=test", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", ...args], { cwd: root, env });
    if (result.exitCode !== 0) throw new Error(result.stderr.toString());
  };
  git("init", "-q");
  git("add", ".");
  git("commit", "-q", "-m", "fixture");
  git("rm", "-q", "scripts/gone.sh");
  git("mv", "scripts/old-name.ts", "scripts/new name.ts");
  const run = Bun.spawnSync(["bun", "scripts/affected-tests.ts", "--fast", "--staged"], { cwd: root, env });
  expect(run.exitCode).toBe(0);
  expect(run.stdout.toString()).toContain("running 2 test file(s): gone.test.ts, renamed.test.ts");
});
