import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

type Step = { id?: string; name?: string; if?: string; run?: string; uses?: string };
type Workflow = { jobs: Record<string, { steps: Step[] }> };

const workflow = Bun.YAML.parse(readFileSync(join(import.meta.dir, "../.github/workflows/ci.yml"), "utf8")) as Workflow;

/** A step condition without its `${{ }}` wrapper. */
export function expression(condition: string): string {
  return condition.trim().replace(/^\$\{\{\s*([\s\S]*?)\s*\}\}$/, "$1");
}

/**
 * The only conditions a check step may use: run after an earlier check fails,
 * never after its gate fails, optionally on one platform. GitHub's expression
 * language has too many ways to skip a step to list the bad ones, so any other
 * shape fails until it is added here on purpose.
 */
export function checkGuards(gate: string): string[] {
  const guard = `!cancelled() && steps.${gate}.outcome == 'success'`;
  return [guard, `${guard} && runner.os == 'Linux'`, `${guard} && runner.os == 'macOS'`];
}

// Cleanup actions report or cache whatever the job produced, so each of their
// conditions still runs after a failed check.
const cleanup = /\/(save-ghostty|upload-artifact)\b/;
export const cleanupGuards = [
  "!cancelled()",
  "failure() && runner.os == 'Linux'",
  "!cancelled() && (steps.smoke-build.outcome == 'success' || failure())",
];

// Smoke runs need compiled binaries, so they gate on smoke-build; the steps
// that prepare and compile gate on smoke-prepare.
test.each([
  ["policy", "policy-setup", () => "policy-setup"],
  ["checks", "checks-setup", () => "checks-setup"],
  ["smoke", "smoke-prepare", (step: Step) => step.run?.includes("HUTERM_CI_SMOKE_STEP=") ? "smoke-build" : "smoke-prepare"],
] as const)("%s steps after %s use an allowed guard", (job, setup, gateFor) => {
  const steps = workflow.jobs[job]!.steps;
  const start = steps.findIndex((step) => step.id === setup);
  expect(start).toBeGreaterThanOrEqual(0);
  const unguarded = steps.slice(start + 1).filter((step) => {
    const allowed = step.uses && cleanup.test(step.uses) ? cleanupGuards : checkGuards(gateFor(step));
    return !allowed.includes(expression(step.if ?? ""));
  }).map((step) => step.name);
  expect(unguarded).toEqual([]);
});

test("only the exact guards are allowed", () => {
  const allowed = checkGuards("checks-setup");
  expect(allowed).toContain(expression("${{ !cancelled() && steps.checks-setup.outcome == 'success' && runner.os == 'Linux' }}"));
  // Each of these once passed a looser rule, or was named in review.
  for (const condition of [
    "",
    "${{ !cancelled() }}",
    "failure()",
    "${{ !cancelled() || steps.checks-setup.outcome == 'success' }}",
    "${{ !cancelled() && steps.smoke-prepare.outcome == 'success' }}",
    "${{ steps.checks-setup.outcome == 'success' }}",
    "${{ !cancelled() && steps.checks-setup.outcome == 'success' && failure() }}",
    "${{ !cancelled() && steps.checks-setup.outcome == 'success' && success() }}",
    "${{ !cancelled() && steps.checks-setup.outcome == 'success' && steps.lint.outcome == 'success' }}",
    "${{ !cancelled() && steps.checks-setup.conclusion == 'success' }}",
  ]) expect(allowed, condition).not.toContain(expression(condition));
  for (const condition of ["${{ !cancelled() && success() }}", "success()", "${{ always() }}"]) {
    expect(cleanupGuards, condition).not.toContain(expression(condition));
  }
});

test("CI smoke steps, ci:smoke:run, and the step supervisor name the same steps", () => {
  const root = join(import.meta.dir, "..");
  const fromWorkflow = (os: string) => workflow.jobs.smoke!.steps
    .filter((step) => step.if?.includes(`runner.os == '${os}'`) || (step.if?.includes("smoke-build") && !step.if.includes("runner.os")))
    .map((step) => /HUTERM_CI_SMOKE_STEP=([\w-]+)/.exec((step as { run?: string }).run ?? "")?.[1])
    .filter((name): name is string => Boolean(name))
    .sort();
  const run = (Bun.TOML.parse(readFileSync(join(root, "mise.toml"), "utf8")) as { tasks: Record<string, { run: string }> }).tasks["ci:smoke:run"]!.run;
  const [darwin, linux] = [...run.matchAll(/all\|([\w|-]+)\) ;;/g)].map((match) => match[1]!.split("|").sort());
  expect(fromWorkflow("macOS")).toEqual(darwin!);
  expect(fromWorkflow("Linux")).toEqual(linux!);
  const supervisor = /new Set\(\[([^\]]+)\]\)/.exec(readFileSync(join(root, "scripts/run-smoke-step.ts"), "utf8"))![1]!;
  const supervised = [...supervisor.matchAll(/"([\w-]+)"/g)].map((match) => match[1]!).sort();
  expect(supervised).toEqual([...new Set([...darwin!, ...linux!])].sort());
});
