/**
 * Run the Rust tests through cargo-nextest, rerunning only failures that match
 * a registered known flake (scripts/flakes.ts).
 *
 * Modes keep the arguments the test tasks used before nextest:
 *   workspace     every workspace target with GPUI's test support, then
 *                 huterm-ghostty's doctests, which nextest cannot run
 *   releasesafe   huterm-ghostty and huterm-core against a ReleaseSafe
 *                 Ghostty build in its own target directory
 *
 * A build or listing failure, a missing JUnit report, or zero tests fails at
 * once. Otherwise every failed test is classified; if any is unmatched, or the
 * registry allows no rerun for it, the run fails without rerunning anything,
 * so a real regression in the same run is never hidden. Matched failures rerun
 * by exact name, and a rerun that fails with a different signature fails.
 * Extra arguments after the mode go to the first nextest run.
 */
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import {
  classify, currentPlatform, decideRetry, loadFlakes, reportDirectory, reportFailure, reportRetry, retriedEntries, retryEnabled,
  type Flake, type Platform,
} from "./flakes.ts";

export type Mode = "workspace" | "releasesafe";

const RELEASESAFE_TARGET = "target/ghostty-releasesafe";

/** The nextest arguments for `mode`, before the profile and any filters. */
export function nextestArgs(mode: Mode): string[] {
  if (mode === "workspace") {
    return ["nextest", "run", "--workspace", "--all-targets", "--features", "huterm-gpui/test-support", "--locked", "--no-fail-fast"];
  }
  // ReleaseSafe keeps Zig's bounds, overflow, and tag checks, which ReleaseFast
  // turns into undefined behavior. `.cargo/config.toml` forces ReleaseFast, so
  // both overrides are required; dropping one silently restores ReleaseFast.
  return [
    "nextest", "run", "--locked", "--no-fail-fast", "--all-targets",
    "-p", "huterm-ghostty", "-p", "huterm-core",
    "--target-dir", RELEASESAFE_TARGET,
    "--config", 'env.HUTERM_GHOSTTY_OPTIMIZE.value="ReleaseSafe"',
    "--config", "env.HUTERM_GHOSTTY_OPTIMIZE.force=true",
  ];
}

/** Commands that run after nextest in `mode`, such as doctests. */
export function followUpCommands(mode: Mode): string[][] {
  return mode === "workspace" ? [["cargo", "test", "--locked", "-p", "huterm-ghostty", "--doc"]] : [];
}

export function junitPath(mode: Mode, profile: string): string {
  return join(mode === "releasesafe" ? RELEASESAFE_TARGET : "target", "nextest", profile, "junit.xml");
}

export type TestCase = { binary: string; test: string; failed: boolean; text: string };

function unescape(text: string): string {
  return text
    .replace(/<!\[CDATA\[([\s\S]*?)\]\]>/g, "$1")
    .replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&apos;/g, "'")
    .replace(/&#(\d+);/g, (_, code: string) => String.fromCodePoint(Number(code)))
    .replace(/&#x([0-9a-f]+);/gi, (_, code: string) => String.fromCodePoint(parseInt(code, 16)))
    .replace(/&amp;/g, "&");
}

function attribute(tag: string, name: string): string | undefined {
  const match = new RegExp(`\\s${name}="([^"]*)"`).exec(tag);
  return match ? unescape(match[1]!) : undefined;
}

/**
 * The test cases in a nextest JUnit report. Each `<testsuite>` is one binary;
 * a failed case carries `<failure>` or `<error>`, and its text joins the
 * failure message with the captured output.
 */
export function parseJunit(xml: string): TestCase[] {
  const cases: TestCase[] = [];
  for (const suite of xml.matchAll(/<testsuite\b([^>]*)>([\s\S]*?)<\/testsuite>/g)) {
    const binary = attribute(suite[1]!, "name") ?? "";
    for (const item of suite[2]!.matchAll(/<testcase\b([^>]*?)(?:\/>|>([\s\S]*?)<\/testcase>)/g)) {
      const test = attribute(item[1]!, "name") ?? "";
      const body = item[2] ?? "";
      const failure = /<(failure|error)\b([^>]*?)(?:\/>|>([\s\S]*?)<\/\1>)/.exec(body);
      const streams = [...body.matchAll(/<(system-out|system-err)>([\s\S]*?)<\/\1>/g)].map((stream) => unescape(stream[2]!));
      const text = failure ? [attribute(failure[2]!, "message") ?? "", unescape(failure[3] ?? ""), ...streams].join("\n") : "";
      cases.push({ binary, test, failed: !!failure, text });
    }
  }
  return cases;
}

/** A nextest filterset selecting exactly `cases`. */
export function filterset(cases: Pick<TestCase, "binary" | "test">[]): string {
  return cases.map((item) => `(binary_id(=${item.binary}) & test(=${item.test}))`).join(" | ");
}

export type RunResult = { exitCode: number; cases?: TestCase[] };
export type Runner = (args: string[]) => Promise<RunResult>;

/**
 * Run nextest and rerun matched failures. Returns the exit code to report.
 * `run` runs one nextest invocation and returns its exit code and parsed
 * JUnit cases, or no cases when the report is missing.
 */
export async function runWithRetries(options: {
  mode: Mode;
  profile: string;
  extra: string[];
  platform: Platform | undefined;
  flakes: Flake[];
  run: Runner;
  env?: NodeJS.ProcessEnv;
}): Promise<number> {
  const env = options.env ?? process.env;
  const base = [...nextestArgs(options.mode), "--profile", options.profile];
  const first = await options.run([...base, ...options.extra]);
  if (first.exitCode === 0) return 0;
  if (!first.cases) {
    console.error("test-rust: nextest failed without a JUnit report, so the build or test listing failed; not retrying");
    return first.exitCode;
  }
  if (!first.cases.length) {
    console.error("test-rust: nextest selected no tests; not retrying");
    return first.exitCode;
  }
  let failures = first.cases.filter((item) => item.failed);
  if (!failures.length) {
    console.error(`test-rust: nextest exited ${first.exitCode} with no failed test in its report; not retrying`);
    return first.exitCode;
  }
  const platform = options.platform;
  const selector = (item: TestCase) => `rust:${item.binary}::${item.test}`;
  const matchOf = (item: TestCase) => platform
    ? classify(options.flakes, { kind: "rust", platform, binary: item.binary, test: item.test, text: item.text })
    : undefined;
  const retried = new Map<string, string>();
  for (let attempt = 1; ; attempt++) {
    const known = retriedEntries(env);
    const plans = failures.map((item) => {
      const flake = matchOf(item);
      const expected = retried.get(selector(item));
      if (expected && flake?.id !== expected) {
        return { item, decision: { retry: false as const, flake, reason: `the rerun of known flake ${expected} failed with a different signature` } };
      }
      const decision = decideRetry(flake, { attempt, retriedEntries: known, enabled: retryEnabled(env) });
      if (decision.retry) known.add(decision.flake.id);
      return { item, decision };
    });
    const blocked = plans.filter((plan) => !plan.decision.retry);
    if (blocked.length) {
      for (const { item, decision } of plans) {
        const reason = decision.retry ? "not rerun, because another failure in this run cannot be retried" : decision.reason;
        if (platform) reportFailure({ event: "failed", flake: decision.flake?.id, kind: "rust", selector: selector(item), platform, attempt, reason }, env);
      }
      return first.exitCode || 1;
    }
    for (const { item, decision } of plans) {
      if (!decision.retry) continue;
      retried.set(selector(item), decision.flake.id);
      console.error(`test-rust: ${item.binary} ${item.test} failed with the signature of known flake ${decision.flake.id}${decision.overdue ? " (review overdue)" : ""}; rerunning (docs/agents/flakes/${decision.flake.id}.md)`);
    }
    const rerun = await options.run([...base, "-E", filterset(failures)]);
    if (!rerun.cases) {
      console.error("test-rust: the rerun produced no JUnit report; failing");
      return rerun.exitCode || 1;
    }
    const outcome = new Map(rerun.cases.map((item) => [selector(item), item]));
    const next: TestCase[] = [];
    for (const item of failures) {
      const result = outcome.get(selector(item));
      const passed = !!result && !result.failed;
      const overdue = plans.find((plan) => plan.item === item)?.decision;
      reportRetry({ event: "retried", flake: retried.get(selector(item))!, kind: "rust", selector: selector(item), platform: platform!, attempt, passed, overdue: overdue?.retry ? overdue.overdue : false }, env);
      if (!result) {
        console.error(`test-rust: the rerun did not run ${item.binary} ${item.test}; failing`);
        return 1;
      }
      if (!passed) next.push(result);
    }
    if (!next.length) {
      console.error(`test-rust: every failure passed on rerun; see the known-flake annotations`);
      return 0;
    }
    failures = next;
  }
}

async function exec(command: string[]): Promise<number> {
  const proc = Bun.spawn(["bash", "scripts/build-exec.sh", ...command], { stdio: ["inherit", "inherit", "inherit"] });
  return await proc.exited;
}

if (import.meta.main) {
  const [mode, ...extra] = Bun.argv.slice(2);
  if (mode !== "workspace" && mode !== "releasesafe") {
    console.error("usage: bun scripts/test-rust.ts workspace|releasesafe [nextest arguments]");
    process.exit(2);
  }
  const profile = process.env.CI ? "ci" : "default";
  const junit = junitPath(mode, profile);
  const { flakes, errors } = loadFlakes();
  const reports = reportDirectory();
  let runs = 0;
  // A broken registry must not block tests; flakes:check reports it.
  for (const error of errors) console.error(`flakes: ignoring invalid entry: ${error}`);
  const status = await runWithRetries({
    mode, profile, extra, flakes, platform: currentPlatform(),
    run: async (args) => {
      rmSync(junit, { force: true });
      const exitCode = await exec(["cargo", ...args]);
      runs++;
      // Keep each failed run's JUnit report, with every test's duration and output, for the CI artifact.
      if (exitCode !== 0 && reports && existsSync(junit)) {
        mkdirSync(reports, { recursive: true });
        copyFileSync(junit, join(reports, `junit-${mode}-${runs}.xml`));
      }
      return { exitCode, cases: existsSync(junit) ? parseJunit(readFileSync(junit, "utf8")) : undefined };
    },
  });
  let followUp = 0;
  for (const command of followUpCommands(mode)) followUp ||= await exec(command);
  process.exit(status || followUp);
}
