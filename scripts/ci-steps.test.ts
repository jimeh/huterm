import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

type Step = { id?: string; name?: string; if?: string };
type Workflow = { jobs: Record<string, { steps: Step[] }> };

const workflow = Bun.YAML.parse(readFileSync(join(import.meta.dir, "../.github/workflows/ci.yml"), "utf8")) as Workflow;

// After setup, a failing check must not skip the checks that follow it.
test.each([
  ["policy", "policy-setup"],
  ["checks", "checks-setup"],
  ["smoke", "smoke-prepare"],
])("%s steps after %s run regardless of earlier check failures", (job, setup) => {
  const steps = workflow.jobs[job]!.steps;
  const start = steps.findIndex((step) => step.id === setup);
  expect(start).toBeGreaterThanOrEqual(0);
  const unguarded = steps.slice(start + 1)
    .filter((step) => !/!cancelled\(\)|failure\(\)/.test(step.if ?? ""))
    .map((step) => step.name);
  expect(unguarded).toEqual([]);
});
