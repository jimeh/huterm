import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

type Step = { id?: string; name?: string; if?: string; run?: string };
type Workflow = { jobs: Record<string, { steps: Step[] }> };

const workflow = Bun.YAML.parse(readFileSync(join(import.meta.dir, "../.github/workflows/ci.yml"), "utf8")) as Workflow;

// After setup, a failing check must not skip the checks that follow it, and a
// failed setup must skip them. Cleanup actions only need to survive failures.
test.each([
  ["policy", "policy-setup", ["policy-setup"]],
  ["checks", "checks-setup", ["checks-setup"]],
  ["smoke", "smoke-prepare", ["smoke-prepare", "smoke-build"]],
])("%s steps after %s run regardless of earlier check failures", (job, setup, gates) => {
  const steps = workflow.jobs[job]!.steps;
  const start = steps.findIndex((step) => step.id === setup);
  expect(start).toBeGreaterThanOrEqual(0);
  const gated = (condition: string) => gates.some((gate) => condition.includes(`steps.${gate}.outcome == 'success'`));
  const unguarded = steps.slice(start + 1).filter((step) => {
    const condition = step.if ?? "";
    if (/failure\(\)/.test(condition)) return false;
    if (!/!cancelled\(\)/.test(condition)) return true;
    return step.run !== undefined && !gated(condition);
  }).map((step) => step.name);
  expect(unguarded).toEqual([]);
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
