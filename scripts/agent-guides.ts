/**
 * Check the agent guides: the root AGENTS.md stays within its line budget,
 * every guide under docs/agents is linked from AGENTS.md, relative Markdown
 * links in these files resolve to existing files and headings, and every plan
 * under docs/plans states where it stands.
 *
 * The budget keeps topic hazards in the guide that owns them instead of the
 * root file every agent session loads. Links are read from inline `[text](url)`
 * syntax outside fenced code blocks; reference-style links are not checked.
 */
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";

export const ROOT_LINE_BUDGET = 200;

const guideDirectory = join("docs", "agents");
const planDirectory = join("docs", "plans");

/** A plan's `Status:` line must open with one of these states. */
export const PLAN_STATES = ["proposed", "approved", "in progress", "implemented", "superseded"] as const;
const planStatus = new RegExp(`^Status: (${PLAN_STATES.join("|")})\\b`, "m");

/** GitHub's heading anchors, including `-1` suffixes for repeated headings. */
export function anchors(markdown: string): Set<string> {
  const result = new Set<string>();
  const seen = new Map<string, number>();
  for (const line of withoutFences(markdown)) {
    const heading = /^#{1,6}\s+(.*?)\s*#*\s*$/.exec(line);
    if (!heading) continue;
    const base = heading[1]!.toLowerCase().replace(/[^\p{L}\p{N} _-]/gu, "").replace(/ /g, "-");
    const count = seen.get(base) ?? 0;
    seen.set(base, count + 1);
    result.add(count === 0 ? base : `${base}-${count}`);
  }
  return result;
}

function withoutFences(markdown: string): string[] {
  const lines: string[] = [];
  let fence: string | undefined;
  for (const line of markdown.split("\n")) {
    const marker = /^\s*(```|~~~)/.exec(line)?.[1];
    if (marker && (!fence || marker === fence)) {
      fence = fence ? undefined : marker;
      continue;
    }
    if (!fence) lines.push(line);
  }
  return lines;
}

function links(markdown: string): { line: number; target: string }[] {
  const result: { line: number; target: string }[] = [];
  const lines = markdown.split("\n");
  const text = new Set(withoutFences(markdown));
  lines.forEach((line, index) => {
    if (!text.has(line)) return;
    for (const match of line.matchAll(/\]\(([^()\s]+)\)/g)) result.push({ line: index + 1, target: match[1]! });
  });
  return result;
}

/** Check the guides under `root`, returning `path:line: message` reports. */
export function checkGuides(root: string): string[] {
  const reports: string[] = [];
  const rootGuide = join(root, "AGENTS.md");
  const rootText = readFileSync(rootGuide, "utf8");
  const rootLines = rootText.replace(/\n$/, "").split("\n").length;
  if (rootLines > ROOT_LINE_BUDGET) {
    reports.push(
      `AGENTS.md: ${rootLines} lines exceeds the ${ROOT_LINE_BUDGET}-line budget. ` +
      "Move topic-specific rules into the docs/agents guide that owns them; keep only rules that apply to most changes.",
    );
  }

  const guides = readdirSync(join(root, guideDirectory)).filter((name) => name.endsWith(".md")).sort();
  const routed = new Set(links(rootText).map(({ target }) => resolve(root, target.split("#")[0]!)));
  for (const name of guides) {
    if (!routed.has(resolve(root, guideDirectory, name))) {
      reports.push(`AGENTS.md: ${guideDirectory}/${name} is not linked; add it to the "Where guidance lives" table.`);
    }
  }

  for (const file of [rootGuide, ...guides.map((name) => join(root, guideDirectory, name))]) {
    const path = relative(root, file).split(sep).join("/");
    for (const { line, target } of links(readFileSync(file, "utf8"))) {
      if (/^[a-z][a-z0-9+.-]*:/i.test(target)) continue;
      const [pathPart, anchor] = target.split("#") as [string, string | undefined];
      const destination = pathPart ? resolve(dirname(file), pathPart) : file;
      if (!existsSync(destination)) {
        reports.push(`${path}:${line}: link target ${target} does not exist`);
      } else if (anchor && destination.endsWith(".md") && !anchors(readFileSync(destination, "utf8")).has(anchor)) {
        reports.push(`${path}:${line}: link target ${target} has no heading #${anchor}`);
      }
    }
  }
  const plans = join(root, planDirectory);
  for (const name of existsSync(plans) ? readdirSync(plans).filter((entry) => entry.endsWith(".md")).sort() : []) {
    if (!planStatus.test(readFileSync(join(plans, name), "utf8"))) {
      reports.push(
        `${planDirectory.split(sep).join("/")}/${name}: add a "Status:" line starting with ${PLAN_STATES.join(", ")}; ` +
        "update it in the change that ships, revises, or replaces the plan.",
      );
    }
  }
  return reports;
}

if (import.meta.main) {
  const reports = checkGuides(resolve(Bun.argv[2] ?? join(import.meta.dir, "..")));
  for (const report of reports) console.error(report);
  if (reports.length) {
    console.error(`agent-guides: ${reports.length} problem(s)`);
    process.exit(1);
  }
  console.log("agent-guides: AGENTS.md and docs/agents are within budget and linked");
}
