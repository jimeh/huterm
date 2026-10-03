import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

type Step = { id?: string; name?: string; if?: string; run?: string; uses?: string };
type Workflow = { jobs: Record<string, { steps: Step[] }> };

const workflow = Bun.YAML.parse(readFileSync(join(import.meta.dir, "../.github/workflows/ci.yml"), "utf8")) as Workflow;

/** The top-level `&&` terms of a step condition, or none when it uses `||`. */
export function conjuncts(condition: string): string[] {
  const expression = condition.trim().replace(/^\$\{\{\s*([\s\S]*?)\s*\}\}$/, "$1");
  if (expression.includes("||")) return [];
  return expression.split("&&").map((term) => term.trim());
}

/** Whether a step runs after any earlier check fails, but never after its gate fails. */
export function guarded(condition: string, gate: string): boolean {
  const terms = conjuncts(condition);
  // Another status term, such as failure() or success(), would skip the
  // check after success or after an earlier failure.
  const otherStatus = terms.some((term) => term !== "!cancelled()" && /\b(always|cancelled|failure|success)\(\)/.test(term));
  return !otherStatus && terms.includes("!cancelled()") && terms.includes(`steps.${gate}.outcome == 'success'`);
}

// Cleanup actions that report or cache whatever the job produced.
const cleanup = /\/(save-ghostty|upload-artifact)\b/;

// After setup, a failing check must not skip the checks that follow it, and a
// failed prerequisite must skip them. Smoke runs need compiled binaries, so
// they gate on smoke-build; the steps that prepare and compile gate on
// smoke-prepare. Only cleanup actions may instead run on failure alone.
test.each([
  ["policy", "policy-setup", () => "policy-setup"],
  ["checks", "checks-setup", () => "checks-setup"],
  ["smoke", "smoke-prepare", (step: Step) => step.run?.includes("HUTERM_CI_SMOKE_STEP=") ? "smoke-build" : "smoke-prepare"],
] as const)("%s steps after %s run regardless of earlier check failures", (job, setup, gateFor) => {
  const steps = workflow.jobs[job]!.steps;
  const start = steps.findIndex((step) => step.id === setup);
  expect(start).toBeGreaterThanOrEqual(0);
  const unguarded = steps.slice(start + 1).filter((step) => {
    const condition = step.if ?? "";
    if (step.uses && cleanup.test(step.uses)) return !/!cancelled\(\)|failure\(\)/.test(condition);
    return !guarded(condition, gateFor(step));
  }).map((step) => step.name);
  expect(unguarded).toEqual([]);
});

test("a guard needs both terms joined by &&, with the expected gate", () => {
  const gate = "checks-setup";
  expect(guarded("${{ !cancelled() && steps.checks-setup.outcome == 'success' && runner.os == 'Linux' }}", gate)).toBe(true);
  for (const condition of [
    "${{ !cancelled() }}",
    "failure()",
    "${{ !cancelled() || steps.checks-setup.outcome == 'success' }}",
    "${{ !cancelled() && steps.smoke-prepare.outcome == 'success' }}",
    "${{ steps.checks-setup.outcome == 'success' }}",
    "${{ !cancelled() && steps.checks-setup.outcome == 'success' && failure() }}",
    "${{ !cancelled() && steps.checks-setup.outcome == 'success' && success() }}",
  ]) expect(guarded(condition, gate), condition).toBe(false);
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
