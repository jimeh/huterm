import { afterEach, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
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
