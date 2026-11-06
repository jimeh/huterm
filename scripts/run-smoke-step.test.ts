import { describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseFlake, readReport, type Flake } from "./flakes.ts";
import { superviseSmokeStep, type AttemptRunner } from "./run-smoke-step.ts";
import type { SmokeOutcome } from "./smoke-process.ts";

function entry(id: string, step: string, match: string, reject: string[] = []): Flake {
  const text = [
    "+++",
    `id = "${id}"`, `title = "${id}"`, 'status = "open"', 'kind = "smoke"', 'platforms = ["linux-x86_64"]', "retries = 1", 'review_by = "2026-12-01"',
    "[selector]", `steps = ["${step}"]`,
    "[signature]", `match = [${JSON.stringify(match)}]`, `reject = ${JSON.stringify(reject)}`,
    "+++", "",
  ].join("\n");
  const { flake, errors } = parseFlake(text, `docs/agents/flakes/${id}.md`, "2026-11-06");
  expect(errors).toEqual([]);
  return flake!;
}

const renderer = entry("renderer-hang", "renderer", "renderer smoke timed out after \\d+ms", ["RENDERER_SMOKE prepared"]);
const hang: SmokeOutcome = { exitCode: 1, signalCode: null, timedOut: false, stopped: false, stdout: "RENDERER_SMOKE launching\n", stderr: "error: renderer smoke timed out after 15000ms\n", elapsedMs: 16_000 };
const pass: SmokeOutcome = { exitCode: 0, signalCode: null, timedOut: false, stopped: false, stdout: "RENDERER_SMOKE passed\n", stderr: "", elapsedMs: 3_000 };

/** A runner that returns `outcomes` in turn and advances a fake clock by each one's duration. */
function scripted(outcomes: SmokeOutcome[]) {
  let now = 0;
  const calls: Parameters<AttemptRunner>[1][] = [];
  const run: AttemptRunner = async (attempt, options) => {
    calls.push(options);
    const outcome = outcomes[attempt - 1];
    if (!outcome) throw new Error(`unexpected attempt ${attempt}`);
    now += outcome.elapsedMs;
    return outcome;
  };
  return { run, calls, clock: () => now };
}

function environment(extra: NodeJS.ProcessEnv = {}): NodeJS.ProcessEnv {
  return { HUTERM_FLAKE_RETRY: "1", HUTERM_FLAKE_REPORT_DIR: mkdtempSync(join(tmpdir(), "flake-report-")), ...extra };
}

async function supervise(outcomes: SmokeOutcome[], env = environment(), flakes = [renderer], budgetMs?: number) {
  const runner = scripted(outcomes);
  const evidence = mkdtempSync(join(tmpdir(), "smoke-evidence-"));
  const result = superviseSmokeStep({ step: "renderer", platform: "linux-x86_64", flakes, env, evidence, budgetMs, run: runner.run, clock: runner.clock });
  return { result, calls: runner.calls, env, evidence };
}

describe("smoke step retries", () => {
  test("a matched failure reruns once with separate evidence and is reported", async () => {
    const { result, calls, env, evidence } = await supervise([hang, pass]);
    expect(await result).toEqual({ attempts: 2, retried: "renderer-hang" });
    expect(calls.map((call) => call.evidenceDir)).toEqual([join(evidence, "steps", "renderer"), join(evidence, "steps", "renderer", "attempt-2")]);
    expect(calls[0]!.env.HUTERM_SMOKE_EVIDENCE_DIR).toBeUndefined();
    expect(calls[1]!.env.HUTERM_SMOKE_EVIDENCE_DIR).toBe(join(evidence, "retries", "renderer-attempt-2"));
    expect(readReport(env)).toMatchObject([{ event: "retried", flake: "renderer-hang", passed: true, attempt: 1 }]);
  });

  test("a failure that recurs after its one retry fails the step", async () => {
    const { result, env } = await supervise([hang, hang]);
    await expect(result).rejects.toThrow("exited 1");
    expect(readReport(env).map((record) => record.event)).toEqual(["retried", "failed"]);
    expect(readReport(env)[1]!.reason).toContain("may be a real regression");
  });

  test("a reject pattern in standard output prevents the retry", async () => {
    const prepared = { ...hang, stdout: `${hang.stdout}RENDERER_SMOKE prepared panel=1\n` };
    const { result, calls } = await supervise([prepared]);
    await expect(result).rejects.toThrow("exited 1");
    expect(calls).toHaveLength(1);
  });

  test("a rerun that fails differently fails at once", async () => {
    const other = { ...hang, stderr: "error: renderer smoke exited 101: panicked\n" };
    const { result, calls } = await supervise([hang, other]);
    await expect(result).rejects.toThrow("failed with a different signature");
    expect(calls).toHaveLength(2);
  });

  test("no retry when another attempt cannot fit the step budget", async () => {
    const slow = { ...hang, elapsedMs: 200_000 };
    const { result, calls, env } = await supervise([slow], environment(), [renderer], 330_000);
    await expect(result).rejects.toThrow("exited 1");
    expect(calls).toHaveLength(1);
    expect(readReport(env)[0]!.reason).toContain("cannot fit");
  });

  test("the second attempt's timeout is what remains of the budget", async () => {
    const { result, calls } = await supervise([hang, pass], environment(), [renderer], 330_000);
    await result;
    expect(calls[0]!.timeoutMs).toBe(300_000);
    expect(calls[1]!.timeoutMs).toBe(330_000 - 16_000 - 5_000 - 10_000);
  });

  test("the job-wide cap counts entries retried by earlier supervisor processes", async () => {
    const env = environment();
    for (const id of ["first", "second"]) {
      const other = entry(id, id, "harness failed with a known message");
      const failed = { ...hang, stderr: "harness failed with a known message\n" };
      const runner = scripted([failed, pass]);
      await superviseSmokeStep({ step: id, platform: "linux-x86_64", flakes: [other], env, run: runner.run, clock: runner.clock });
    }
    const { result, calls } = await supervise([hang], env);
    await expect(result).rejects.toThrow("exited 1");
    expect(calls).toHaveLength(1);
    expect(readReport(env).at(-1)!.reason).toContain("already retried 2 other flakes");
  });

  test("local runs and interrupted attempts do not retry", async () => {
    const local = await supervise([hang], environment({ HUTERM_FLAKE_RETRY: undefined }));
    await expect(local.result).rejects.toThrow();
    expect(local.calls).toHaveLength(1);
    const interrupted = await supervise([{ ...hang, signalCode: "SIGTERM", interrupted: "SIGTERM" }]);
    await expect(interrupted.result).rejects.toThrow("terminated by SIGTERM");
    expect(interrupted.calls).toHaveLength(1);
  });

  test("a retry on an overdue entry says so in its report", async () => {
    const overdue = entry("renderer-hang", "renderer", "renderer smoke timed out after \\d+ms");
    overdue.review_by = "2000-01-01";
    const { result, env } = await supervise([hang, pass], environment(), [overdue]);
    await result;
    expect(readReport(env)).toMatchObject([{ event: "retried", passed: true, overdue: true }]);
  });

  test("a retried pass is annotated and summarized in Actions", async () => {
    const summary = join(mkdtempSync(join(tmpdir(), "summary-")), "summary.md");
    const lines: string[] = [];
    const log = console.log;
    console.log = (line: string) => lines.push(line);
    try {
      const { result } = await supervise([hang, pass], environment({ GITHUB_ACTIONS: "true", GITHUB_STEP_SUMMARY: summary }));
      await result;
    } finally {
      console.log = log;
    }
    expect(lines.join("\n")).toContain("::warning title=Known flake renderer-hang::smoke");
    expect(readFileSync(summary, "utf8")).toContain("| renderer-hang | `smoke:renderer` | 1 | rerun passed |");
  });
});
