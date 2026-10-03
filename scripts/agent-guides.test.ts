import { afterEach, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { anchors, checkGuides, ROOT_LINE_BUDGET } from "./agent-guides.ts";

const roots: string[] = [];
afterEach(() => { for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });

function fixture(agents: string, guides: Record<string, string>) {
  const root = mkdtempSync(join(tmpdir(), "huterm-agent-guides-"));
  roots.push(root);
  mkdirSync(join(root, "docs/agents"), { recursive: true });
  writeFileSync(join(root, "AGENTS.md"), agents);
  for (const [name, text] of Object.entries(guides)) writeFileSync(join(root, "docs/agents", name), text);
  return root;
}

test("a routed guide with valid file and heading links passes", () => {
  const root = fixture("# Guide\n\n[CI](docs/agents/ci.md#jobs-and-caches)\n", {
    "ci.md": "# CI\n\n## Jobs and caches\n\nSee [the root](../../AGENTS.md#guide).\n",
  });
  expect(checkGuides(root)).toEqual([]);
});

test("a root file over budget fails with placement guidance", () => {
  const root = fixture(`# Guide\n\n[CI](docs/agents/ci.md)\n${"line\n".repeat(ROOT_LINE_BUDGET)}`, { "ci.md": "# CI\n" });
  const [report] = checkGuides(root);
  expect(report).toContain(`exceeds the ${ROOT_LINE_BUDGET}-line budget`);
  expect(report).toContain("docs/agents guide that owns them");
});

test("an unrouted guide and broken links are reported", () => {
  const root = fixture("# Guide\n\n[CI](docs/agents/ci.md#missing)\n[Gone](docs/agents/gone.md)\n", {
    "ci.md": "# CI\n\n```md\n[ignored](nowhere.md)\n```\n",
    "extra.md": "# Extra\n",
  });
  expect(checkGuides(root)).toEqual([
    "AGENTS.md: docs/agents/extra.md is not linked; add it to the \"Where guidance lives\" table.",
    "AGENTS.md:3: link target docs/agents/ci.md#missing has no heading #missing",
    "AGENTS.md:4: link target docs/agents/gone.md does not exist",
  ]);
});

test("anchors follow GitHub heading slugs", () => {
  expect([...anchors("# The `huterm-ghostty` crate\n## Close and Quit\n## Close and Quit\n```\n# not a heading\n```\n")])
    .toEqual(["the-huterm-ghostty-crate", "close-and-quit", "close-and-quit-1"]);
});

test("every plan needs a recognized status line", () => {
  const root = fixture("# Guide\n\n[CI](docs/agents/ci.md)\n", { "ci.md": "# CI\n" });
  mkdirSync(join(root, "docs/plans"), { recursive: true });
  writeFileSync(join(root, "docs/plans/shipped.md"), "# Shipped\n\nStatus: implemented in PR #1.\n");
  writeFileSync(join(root, "docs/plans/stale.md"), "# Stale\n\nStatus: agreed on Monday.\n");
  writeFileSync(join(root, "docs/plans/missing.md"), "# Missing\n\nNo status here.\n");
  expect(checkGuides(root).map((report) => report.split(":")[0])).toEqual(["docs/plans/missing.md", "docs/plans/stale.md"]);
});

test("anchors use visible link text and longer fences hide shorter example fences", () => {
  expect([...anchors("## [Overview](guide.md)\n## See [the guide][ref]\n````md\n```\n# not a heading\n```\n````\n## After\n")])
    .toEqual(["overview", "see-the-guide", "after"]);
});
