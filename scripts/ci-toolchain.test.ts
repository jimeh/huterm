import { afterEach, expect, test } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const directories: string[] = [];
afterEach(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });

function bootstrap(failure = "none") {
  const root = mkdtempSync(join(tmpdir(), "huterm-rust-bootstrap-"));
  directories.push(root);
  const bin = join(root, "bin");
  const cargo = join(root, "cargo home");
  mkdirSync(bin);
  mkdirSync(join(cargo, "bin"), { recursive: true });
  const executable = (file: string, body: string) => {
    writeFileSync(file, `#!/bin/bash\nset -eu\n${body}\n`);
    chmodSync(file, 0o755);
  };
  executable(join(bin, "mise"), `
if [ "$1" = tool ]; then echo 1.98.1; exit 0; fi
printf '%s\\n' "$*" >> "$TEST_ROOT/log"
if [ "$1" = install ]; then
  if [ ! -e "$TEST_ROOT/installed" ]; then
    touch "$TEST_ROOT/installed"
    case "$TEST_FAILURE" in install|retry-install|no-rustup) exit 1;; esac
  elif [ "$TEST_FAILURE" = retry-install ]; then exit 1; fi
fi
if [ "$1" = run ]; then
  [ "$RUSTUP_TOOLCHAIN" = 1.98.1 ] || exit 99
  case "$TEST_FAILURE" in
    verify|retry-verify|uninstall) [ -e "$TEST_ROOT/repaired" ] || exit 1;;
  esac
  [ "$TEST_FAILURE" != retry-verify ] || exit 1
fi`);
  executable(join(cargo, "bin/rustup"), `
printf '%s\\n' "rustup $*" >> "$TEST_ROOT/log"
[ "$TEST_FAILURE" != uninstall ] || exit 1
touch "$TEST_ROOT/repaired"`);
  if (failure === "no-rustup") rmSync(join(cargo, "bin/rustup"));
  const result = Bun.spawnSync(["bash", join(import.meta.dir, "ci-toolchain.sh")], {
    env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, CARGO_HOME: cargo, RUSTUP_TOOLCHAIN: "wrong-inherited-version", TEST_ROOT: root, TEST_FAILURE: failure },
  });
  return { code: result.exitCode, log: readFileSync(join(root, "log"), "utf8").trim().split("\n") };
}

const install = "install --locked --jobs=1 rust";
const verify = "run verify:toolchain";
const repair = ["rustup toolchain uninstall 1.98.1", "install --locked --force --jobs=1 rust"];

test("healthy Rust bootstrap installs and verifies without removing the toolchain", () => {
  expect(bootstrap()).toEqual({ code: 0, log: [install, verify] });
});
for (const failure of ["install", "verify"]) {
  test(`Rust bootstrap recovers once after ${failure} failure`, () => {
    expect(bootstrap(failure)).toEqual({ code: 0, log: [install, ...(failure === "verify" ? [verify] : []), ...repair, verify] });
  });
}
for (const failure of ["retry-install", "retry-verify"]) {
  test(`Rust bootstrap fails closed after ${failure} failure`, () => {
    const result = bootstrap(failure);
    expect(result.code).not.toBe(0);
    expect(result.log).toEqual([install, ...(failure === "retry-verify" ? [verify] : []), ...repair, ...(failure === "retry-verify" ? [verify] : [])]);
  });
}

test("Rust bootstrap retries an installer failure before Rustup exists", () => {
  expect(bootstrap("no-rustup")).toEqual({ code: 0, log: [install, repair[1]!, verify] });
});

test("Rust bootstrap stops if clean removal fails", () => {
  expect(bootstrap("uninstall")).toEqual({ code: 1, log: [install, verify, repair[0]!] });
});

test("CI and release setup reach Rust recovery before any Cargo tool installation", () => {
  type Step = { uses?: string; run?: string; with?: { install_args?: string } };
  type Workflow = { jobs?: Record<string, { steps?: Step[] }>; runs?: { steps: Step[] } };
  for (const file of [".github/workflows/ci.yml", ".github/workflows/release.yml", ".github/actions/prepare-release-candidate/action.yml"]) {
    const workflow = Bun.YAML.parse(readFileSync(join(import.meta.dir, "..", file), "utf8")) as Workflow;
    const groups = workflow.runs ? [workflow.runs] : Object.values(workflow.jobs!);
    for (const { steps = [] } of groups) {
      const installs = steps.filter(step => step.uses?.startsWith("jdx/mise-action@"));
      if (installs.length === 0) continue;
      const bootstrapIndex = steps.findIndex(step => step.run === "mise run ci:toolchain");
      expect(bootstrapIndex).toBeGreaterThanOrEqual(0);
      for (const step of installs) {
        expect(step.with?.install_args).toBeDefined();
        expect(step.with!.install_args!.split(/\s+/).some(tool => tool === "rust" || tool.startsWith("cargo:"))).toBe(false);
        expect(steps.indexOf(step)).toBeLessThan(bootstrapIndex);
      }
      for (const [index, step] of steps.entries()) {
        if (step.run?.includes("mise install --locked cargo:")) expect(index).toBeGreaterThan(bootstrapIndex);
      }
    }
  }
});
