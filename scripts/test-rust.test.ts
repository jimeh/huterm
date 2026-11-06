import { describe, expect, test } from "bun:test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseFlake, readReport, type Flake } from "./flakes.ts";
import { filterset, followUpCommands, nextestArgs, parseJunit, runWithRetries, type RunResult, type TestCase } from "./test-rust.ts";

function rust(id: string, test: string, match: string, status = "open"): Flake {
  const text = [
    "+++",
    `id = "${id}"`, `title = "${id}"`, `status = "${status}"`, 'kind = "rust"', 'platforms = ["linux-x86_64"]',
    `retries = ${status === "fixed" ? 0 : 2}`, 'review_by = "2026-12-01"',
    "[selector]", 'binary = "huterm-gpui"', `tests = ["${test}"]`,
    "[signature]", `match = [${JSON.stringify(match)}]`,
    "+++", "",
  ].join("\n");
  const { flake, errors } = parseFlake(text, `docs/agents/flakes/${id}.md`, "2026-11-06");
  expect(errors).toEqual([]);
  return flake!;
}

const close = rust("close-button", "window_tests::close", "timed out waiting for the click to close");
const clip = rust("clipboard", "window_tests::clip", "timed out waiting for the clipboard write");
const failed = (test: string, text: string): TestCase => ({ binary: "huterm-gpui", test, failed: true, text });
const passed = (test: string): TestCase => ({ binary: "huterm-gpui", test, failed: false, text: "" });

function environment(extra: NodeJS.ProcessEnv = {}): NodeJS.ProcessEnv {
  return { HUTERM_FLAKE_RETRY: "1", HUTERM_FLAKE_REPORT_DIR: mkdtempSync(join(tmpdir(), "flake-report-")), ...extra };
}

async function run(results: RunResult[], flakes = [close, clip], env = environment()) {
  const calls: string[][] = [];
  const status = await runWithRetries({
    mode: "workspace", profile: "ci", extra: [], platform: "linux-x86_64", flakes, env,
    run: async (args) => {
      calls.push(args);
      const result = results[calls.length - 1];
      if (!result) throw new Error(`unexpected run ${calls.length}: ${args.join(" ")}`);
      return result;
    },
  });
  return { status, calls, env };
}

const closeTimeout = failed("window_tests::close", "panicked: timed out waiting for the click to close the inactive tab");

describe("nextest retries", () => {
  test("a matched failure reruns by exact name and passes", async () => {
    const { status, calls, env } = await run([
      { exitCode: 100, cases: [passed("other"), closeTimeout] },
      { exitCode: 0, cases: [passed("window_tests::close")] },
    ]);
    expect(status).toBe(0);
    expect(calls[1]!.slice(-2)).toEqual(["-E", "(binary_id(=huterm-gpui) & test(=window_tests::close))"]);
    expect(readReport(env)).toMatchObject([{ event: "retried", flake: "close-button", passed: true }]);
  });

  test("an unmatched failure in the same run blocks every rerun", async () => {
    const { status, calls, env } = await run([{ exitCode: 100, cases: [closeTimeout, failed("window_tests::new", "assertion failed")] }]);
    expect(status).toBe(100);
    expect(calls).toHaveLength(1);
    expect(readReport(env).map((record) => record.reason)).toEqual([
      "not rerun, because another failure in this run cannot be retried",
      "no registered flake matches this failure",
    ]);
  });

  test("a failure that keeps failing exhausts the budget and fails", async () => {
    const { status, calls, env } = await run([
      { exitCode: 100, cases: [closeTimeout] },
      { exitCode: 100, cases: [closeTimeout] },
      { exitCode: 100, cases: [closeTimeout] },
    ]);
    expect(status).toBe(100);
    expect(calls).toHaveLength(3);
    expect(readReport(env).at(-1)!.reason).toContain("exhausting its 2 retries");
  });

  test("a rerun that fails with a different signature fails", async () => {
    const { status, calls, env } = await run([
      { exitCode: 100, cases: [closeTimeout] },
      { exitCode: 100, cases: [failed("window_tests::close", "panicked: index out of bounds")] },
    ]);
    expect(status).toBe(100);
    expect(calls).toHaveLength(2);
    expect(readReport(env).at(-1)!.reason).toContain("different signature");
  });

  test("a fixed entry's signature fails as a regression", async () => {
    const fixed = rust("close-button", "window_tests::close", "timed out waiting for the click to close", "fixed");
    const { status, env } = await run([{ exitCode: 100, cases: [closeTimeout] }], [fixed]);
    expect(status).toBe(100);
    expect(readReport(env)[0]!.reason).toContain("recurred after its fix");
  });

  test("build failures, missing reports, and empty runs never rerun", async () => {
    expect((await run([{ exitCode: 101 }])).calls).toHaveLength(1);
    expect((await run([{ exitCode: 4, cases: [] }])).status).toBe(4);
    expect((await run([{ exitCode: 100, cases: [passed("a")] }])).status).toBe(100);
  });

  test("three distinct flakes in one job exceed the cap", async () => {
    const third = rust("third", "window_tests::third", "third flake signature text");
    const { status, calls } = await run([{
      exitCode: 100,
      cases: [closeTimeout, failed("window_tests::clip", "timed out waiting for the clipboard write"), failed("window_tests::third", "third flake signature text")],
    }], [close, clip, third]);
    expect(status).toBe(100);
    expect(calls).toHaveLength(1);
  });

  test("local runs classify but do not rerun", async () => {
    const { status, calls, env } = await run([{ exitCode: 100, cases: [closeTimeout] }], [close], environment({ HUTERM_FLAKE_RETRY: "" }));
    expect(status).toBe(100);
    expect(calls).toHaveLength(1);
    expect(readReport(env)[0]!.reason).toContain("local runs do not retry");
  });
});

describe("command modes", () => {
  test("workspace mode keeps the test task's targets and features, then runs doctests", () => {
    expect(nextestArgs("workspace")).toEqual(["nextest", "run", "--workspace", "--all-targets", "--features", "huterm-gpui/test-support", "--locked", "--no-fail-fast"]);
    expect(followUpCommands("workspace")).toEqual([["cargo", "test", "--locked", "-p", "huterm-ghostty", "--doc"]]);
  });

  test("releasesafe mode keeps both optimize overrides and its target directory", () => {
    const args = nextestArgs("releasesafe");
    expect(args).toContain('env.HUTERM_GHOSTTY_OPTIMIZE.value="ReleaseSafe"');
    expect(args).toContain("env.HUTERM_GHOSTTY_OPTIMIZE.force=true");
    expect(args.join(" ")).toContain("-p huterm-ghostty -p huterm-core --target-dir target/ghostty-releasesafe");
    expect(followUpCommands("releasesafe")).toEqual([]);
  });
});

describe("JUnit parsing", () => {
  test("reads binaries, names, failures, and escaped output", () => {
    const xml = `<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="3" failures="1" errors="0">
  <testsuite name="huterm-core" tests="1" disabled="0" errors="0" failures="0">
    <testcase name="terminal::tests::ok" classname="huterm-core" timestamp="t" time="0.1"/>
  </testsuite>
  <testsuite name="huterm-gpui" tests="2" disabled="0" errors="0" failures="1">
    <testcase name="window_tests::close" classname="huterm-gpui" time="10.1">
      <failure message="test failed with exit code 101" type="test failure">thread panicked at &apos;timed out&apos;</failure>
      <system-out>out &lt;tag&gt; &amp; more</system-out>
      <system-err>timed out waiting for the click to close the inactive tab</system-err>
    </testcase>
    <testcase name="window_tests::fine" classname="huterm-gpui" time="1.0"></testcase>
  </testsuite>
</testsuites>`;
    const cases = parseJunit(xml);
    expect(cases.map((item) => [item.binary, item.test, item.failed])).toEqual([
      ["huterm-core", "terminal::tests::ok", false],
      ["huterm-gpui", "window_tests::close", true],
      ["huterm-gpui", "window_tests::fine", false],
    ]);
    expect(cases[1]!.text).toContain("thread panicked at 'timed out'");
    expect(cases[1]!.text).toContain("out <tag> & more");
    expect(cases[1]!.text).toContain("timed out waiting for the click to close");
  });

  test("selects several exact tests in one filterset", () => {
    expect(filterset([{ binary: "a", test: "x" }, { binary: "b::it", test: "y::z" }])).toBe("(binary_id(=a) & test(=x)) | (binary_id(=b::it) & test(=y::z))");
  });
});
