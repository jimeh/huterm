import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { releaseToolInstalls } from "./release-tool-installs.ts";

type Step = { name?: string; run?: string; env?: Record<string, string>; with?: { install_args?: string } };
type Workflow = { jobs: Record<string, { strategy?: { matrix?: { include?: { runner?: string }[] } }; steps: Step[] }> };

const root = join(import.meta.dir, "..");
const parse = <T>(file: string) => Bun.YAML.parse(readFileSync(join(root, file), "utf8")) as T;

test("release tool lists follow local actions into their Mise installs", () => {
  const action = parse<{ runs: { steps: Step[] } }>(".github/actions/prepare-release-candidate/action.yml");
  const candidateTools = action.runs.steps.find(step => step.with?.install_args)?.with?.install_args;
  expect(candidateTools).toBeDefined();
  expect(releaseToolInstalls().get("macos-15")).toContain(candidateTools!);
});

test("CI installs every release runner's tool lists on that runner", () => {
  const ci = parse<Workflow>(".github/workflows/ci.yml");
  const packageJob = ci.jobs.package!;
  const step = packageJob.steps.find(step => step.name === "Install release tools in a clean Mise state");
  expect(step?.env?.RELEASE_RUNNER).toBe("${{ matrix.runner }}");
  expect(step?.run).toContain('bun scripts/release-tool-installs.ts "$RELEASE_RUNNER"');
  const ciRunners = (packageJob.strategy?.matrix?.include ?? []).map(entry => entry.runner);
  // Every CI job already installs Bun, so a Bun-only release runner needs no package job.
  const uncovered = [...releaseToolInstalls()]
    .filter(([runner, lists]) => !ciRunners.includes(runner) && lists.some(tools => tools !== "bun"))
    .map(([runner]) => runner);
  expect(uncovered).toEqual([]);
});
