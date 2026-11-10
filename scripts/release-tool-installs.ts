/**
 * Lists the Mise install arguments that the Release workflow's jobs pass to
 * `jdx/mise-action` on one runner label, following local composite actions.
 * CI's package jobs install each list from a clean Mise state, so a release
 * tool that cannot install fails a pull request instead of a release.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";

type Step = { uses?: string; with?: { install?: boolean; install_args?: string } };
type Job = { "runs-on"?: string; steps?: Step[] };

const root = join(import.meta.dir, "..");
const releaseWorkflow = ".github/workflows/release.yml";

function parse<T>(file: string): T {
  return Bun.YAML.parse(readFileSync(join(root, file), "utf8")) as T;
}

function stepInstalls(steps: Step[], file: string): string[] {
  return steps.flatMap(step => {
    if (step.uses?.startsWith("jdx/mise-action@")) {
      if (step.with?.install === false) return [];
      const tools = step.with?.install_args?.trim().split(/\s+/).join(" ");
      if (!tools) throw new Error(`${file}: a Mise action step installs every configured tool; name its tools in install_args`);
      return [tools];
    }
    const local = step.uses?.match(/^\.\/(\.github\/actions\/[^@\s]+)$/)?.[1];
    if (!local) return [];
    const action = `${local}/action.yml`;
    return stepInstalls(parse<{ runs: { steps?: Step[] } }>(action).runs.steps ?? [], action);
  });
}

/** Every release runner label mapped to its distinct install lists, in workflow order. */
export function releaseToolInstalls(): Map<string, string[]> {
  const installs = new Map<string, string[]>();
  for (const job of Object.values(parse<{ jobs: Record<string, Job> }>(releaseWorkflow).jobs)) {
    const runner = job["runs-on"];
    if (!runner) throw new Error(`${releaseWorkflow}: every job needs a literal runs-on label`);
    const lists = installs.get(runner) ?? [];
    for (const tools of stepInstalls(job.steps ?? [], releaseWorkflow)) if (!lists.includes(tools)) lists.push(tools);
    installs.set(runner, lists);
  }
  return installs;
}

if (import.meta.main) {
  const runner = process.argv[2];
  if (!runner || process.argv.length > 3) {
    console.error("usage: bun scripts/release-tool-installs.ts <runner-label>");
    process.exit(2);
  }
  const lists = releaseToolInstalls().get(runner);
  if (!lists) {
    console.error(`release-tool-installs: no Release job runs on ${runner}`);
    process.exit(1);
  }
  for (const tools of lists) console.log(tools);
}
