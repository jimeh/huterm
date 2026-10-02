/**
 * Run the Bun tests under `scripts/` that the given changed paths can affect,
 * such as the staged files from a pre-commit hook.
 *
 * A test depends on every script it imports and every script whose file name
 * appears in its source, which covers fixtures that spawn a script or read a
 * shell file by path. Dependencies are followed transitively. A test is also
 * affected when it or a script it depends on mentions a changed path outside
 * `scripts/`, such as a workflow or `mise.toml` it inspects. Changes to Bun's
 * package, lockfile, or compiler configuration select every test.
 *
 * A deleted or renamed script still counts through the names and relative
 * imports that refer to it, so the tests that still use it are selected.
 *
 * File-name matching can select extra tests, which is safe. A test that builds
 * a path from separate segments is not matched; CI runs the full suite.
 *
 * `--fast` is the pre-commit scope. It never selects every test and skips the
 * suites in `SLOW_SUITES`, leaving them to `mise run test:scripts` and CI.
 * `--staged` reads the staged paths from Git, including deletions, which
 * lefthook's `{staged_files}` omits.
 */
import { readdirSync, readFileSync, realpathSync } from "node:fs";
import { basename, dirname, join, relative, sep } from "node:path";

const everyTestInputs = new Set(["package.json", "bun.lock", "bunfig.toml", "tsconfig.json"]);

/**
 * Suites that each took over 3 seconds alone on 2026-10-02; every other suite
 * took about 2 seconds or less. Add a suite here when it becomes that slow.
 */
export const SLOW_SUITES: ReadonlySet<string> = new Set([
  "scripts/linux-vm.test.ts",
  "scripts/macos-vm.test.ts",
  "scripts/smoke-process.test.ts",
  "scripts/vendor.test.ts",
]);

function scriptFiles(root: string, directory = "scripts"): string[] {
  const files: string[] = [];
  for (const entry of readdirSync(join(root, directory), { withFileTypes: true })) {
    const path = `${directory}/${entry.name}`;
    if (entry.isDirectory()) {
      if (entry.name !== "node_modules") files.push(...scriptFiles(root, path));
    } else if (entry.isFile()) {
      files.push(path);
    }
  }
  return files.sort();
}

/** Return the repository-relative test files affected by `changed` paths. */
export function affectedTests(root: string, changed: readonly string[], options: { fast?: boolean } = {}): string[] {
  const files = scriptFiles(root);
  const tests = files.filter((file) => file.endsWith(".test.ts") && !(options.fast && SLOW_SUITES.has(file)));
  // Fast mode skips only this shortcut; tests that read these files by name
  // are still selected below.
  if (!options.fast && changed.some((path) => everyTestInputs.has(path))) return tests;
  const existing = new Set(files);
  const removedScripts = changed.filter((path) => path.startsWith("scripts/") && !existing.has(path));

  // Bun resolves imports to real paths, so compare them against the real root.
  const realRoot = realpathSync(root);
  const transpiler = new Bun.Transpiler({ loader: "ts" });
  const sources = new Map(files.filter((file) => file.endsWith(".ts")).map((file) => [file, readFileSync(join(root, file), "utf8")]));
  const dependencies = new Map<string, Set<string>>();
  for (const [file, source] of sources) {
    const direct = new Set<string>();
    for (const { path } of transpiler.scanImports(source)) {
      if (!path.startsWith(".")) continue;
      try {
        direct.add(relative(realRoot, Bun.resolveSync(path, join(realRoot, dirname(file)))).split(sep).join("/"));
      } catch {
        // The target may be a deleted script; keep the edge so a staged
        // deletion still selects this file. Extensionless imports name a .ts.
        const target = join(dirname(file), path).split(sep).join("/");
        direct.add(target);
        direct.add(`${target}.ts`);
      }
    }
    for (const other of [...files, ...removedScripts]) if (other !== file && source.includes(basename(other))) direct.add(other);
    dependencies.set(file, direct);
  }

  const changedScripts = new Set(changed.filter((path) => path.startsWith("scripts/")));
  const changedElsewhere = changed.filter((path) => !path.startsWith("scripts/"));
  const affected = (file: string, seen: Set<string>): boolean => {
    if (seen.has(file)) return false;
    seen.add(file);
    if (changedScripts.has(file)) return true;
    const source = sources.get(file);
    if (source && changedElsewhere.some((path) => source.includes(path))) return true;
    return [...(dependencies.get(file) ?? [])].some((dependency) => affected(dependency, seen));
  };
  return tests.filter((test) => affected(test, new Set()));
}

if (import.meta.main) {
  const root = join(import.meta.dir, "..");
  const args = Bun.argv.slice(2);
  const fast = args.includes("--fast");
  const changed = args.filter((arg) => arg !== "--fast" && arg !== "--staged");
  if (args.includes("--staged")) {
    const staged = Bun.spawnSync(["git", "diff", "--cached", "--name-only", "--no-renames", "-z"], { cwd: root });
    if (staged.exitCode !== 0) throw new Error(`git diff --cached failed: ${staged.stderr.toString()}`);
    changed.push(...staged.stdout.toString().split("\0").filter(Boolean));
  }
  const tests = affectedTests(root, changed, { fast });
  if (fast) {
    const skipped = affectedTests(root, changed).filter((test) => !tests.includes(test));
    if (skipped.length) console.log(`affected-tests: left ${skipped.length} slow or full-suite test file(s) to \`mise run test:scripts\``);
  }
  if (!tests.length) {
    console.log("affected-tests: no fast script tests depend on the changed paths");
    process.exit(0);
  }
  console.log(`affected-tests: running ${tests.length} test file(s): ${tests.map((test) => basename(test)).join(", ")}`);
  const result = Bun.spawnSync(["bun", "test", ...tests.map((test) => `./${test}`)], { cwd: root, stdio: ["inherit", "inherit", "inherit"] });
  process.exit(result.exitCode ?? 1);
}
