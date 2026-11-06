import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkPullRequest, squashBody, squashMessage } from "./pr-description.ts";

const title = "fix: keep the notes";

// Description lines from #410 and #354, whose squash commits Release Please
// dropped. Neither starts with the call syntax; GitHub's wrapping moved it to
// the start of a commit body line.
const pr410Line = "- `input_racing_a_tab_close_to_the_window_raises_no_notice` (huterm-gpui) holds the race open deterministically. It closes a tab through Mux, then runs Clear Scrollback and types into the pane before any executor task runs. It drains the pane, then checks the window's notices once the tab is gone. When the view check is reverted to exit-only, the test fails at the notice assertion with the CI message, `left: [\"error|terminal:huterm-gpui|terminal runtime has stopped\"]`. Without the command guard, it fails at the command assertion with `Err(Runtime(\"terminal runtime has stopped\"))`.";
const pr354Line = "**Main Centered layout.** A sixth built-in layout, `main_centered`, is `columns(rows(*), pane 50%, rows(*))`: one pane in the middle at 50 %, with the panes before it stacked on the left and the panes after it on the right.";

const roots: string[] = [];
afterEach(() => { for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });

test("the squash body keeps short and fenced lines and rewraps long prose lines at 72 columns", () => {
  const long = `  - ${"word ".repeat(14)}spaced   out ${"x".repeat(80)} end`;
  const fenced = `    ${"code ".repeat(20)}`;
  expect(squashBody(`  - short indented line\r\n\n${long}\n\`\`\`text\n${fenced}\n\`\`\`\n\n`)).toBe([
    "  - short indented line",
    "",
    `- ${"word ".repeat(13)}word`,
    "spaced out",
    "x".repeat(80),
    "end",
    "```text",
    fenced,
    "```",
  ].join("\n"));
  expect(squashMessage({ title, number: 7, body: " \n" })).toBe(`${title} (#7)`);
});

test("a fence closes only on a run of its own character at least as long", () => {
  const long = `${"code ".repeat(20)}`;
  const body = ["````md", "```", long, "```", long, "````", long].join("\n");
  expect(squashBody(body).split("\n")).toEqual(["````md", "```", long, "```", long, "````", ...squashBody(long).split("\n")]);
  expect(squashBody(long).split("\n").length).toBeGreaterThan(1);
});

test.each([["#410", pr410Line, "`Err(Runtime(\"terminal"], ["#354", pr354Line, "`columns(rows(*),"]])(
  "a description line that wraps call syntax to a line start fails like %s",
  (_, line, start) => {
    const [problem, ...rest] = checkPullRequest({ title, number: 1, body: `Intro.\n\n${line}\n` });
    expect(rest).toEqual([]);
    expect(problem).toContain("The squash commit message does not parse (unexpected token");
    expect(problem).toContain(`\n  ${start}`);
    expect(problem).toContain("reword nested or unclosed call syntax");
  },
);

test("balanced call syntax and nested calls inside a line parse", () => {
  const body = "`foo(bar)` at a line start is a footer-shaped token that closes.\nShort lines keep `Err(Runtime(x))` mid-line.\n";
  expect(checkPullRequest({ title, number: 1, body })).toEqual([]);
});

test("a title that is not a conventional commit header fails", () => {
  const [problem] = checkPullRequest({ title: "Keep the notes", number: 1, body: "" });
  expect(problem).toContain("The squash commit message does not parse");
  expect(problem).toContain("must be a conventional commit header");
});

test("a complete commit-override block replaces a description that would not parse", () => {
  const block = "BEGIN_COMMIT_OVERRIDE\nfix: keep the notes (#1)\n\nfeat: add a layout\nEND_COMMIT_OVERRIDE";
  expect(checkPullRequest({ title, number: 1, body: `${pr410Line}\n\n${block}\n` })).toEqual([]);

  const [problem] = checkPullRequest({ title, number: 1, body: "BEGIN_COMMIT_OVERRIDE\nkeep the notes\nEND_COMMIT_OVERRIDE\n" });
  expect(problem).toContain("The commit-override block does not parse");
});

test("each nested commit in an override block is parsed on its own", () => {
  const nested = (message: string) => `BEGIN_COMMIT_OVERRIDE\r\nBEGIN_NESTED_COMMIT\r\n${message}\r\nEND_NESTED_COMMIT\r\nEND_COMMIT_OVERRIDE\r\n`;
  expect(checkPullRequest({ title, number: 1, body: nested("fix: include the change") })).toEqual([]);

  const problems = checkPullRequest({ title, number: 1, body: nested("include the change") });
  expect(problems).toHaveLength(1);
  expect(problems[0]).toContain("The commit-override block does not parse");
  expect(problems[0]).toContain("\n  include the change\n");
});

test.each([
  ["a marker in prose", "Avoid a stray `BEGIN_COMMIT_OVERRIDE` here.\n"],
  ["a block without an end marker", "BEGIN_COMMIT_OVERRIDE\nfix: keep the notes\n"],
  ["two blocks", "BEGIN_COMMIT_OVERRIDE\nfix: a\nEND_COMMIT_OVERRIDE\nBEGIN_COMMIT_OVERRIDE\nfix: b\nEND_COMMIT_OVERRIDE\n"],
  ["an empty block", "BEGIN_COMMIT_OVERRIDE\n\nEND_COMMIT_OVERRIDE\n"],
  ["an end marker before the begin marker", "END_COMMIT_OVERRIDE\nfix: a\nBEGIN_COMMIT_OVERRIDE\n"],
])("%s is not a valid commit-override block", (_, body) => {
  const [problem, ...rest] = checkPullRequest({ title, number: 1, body });
  expect(rest).toEqual([]);
  expect(problem).toContain("outside a single complete commit-override block");
});

test("the command reads a GitHub event payload and exits non-zero on a problem", () => {
  const root = mkdtempSync(join(tmpdir(), "huterm-pr-description-"));
  roots.push(root);
  const run = (pullRequest: object) => {
    const event = join(root, "event.json");
    writeFileSync(event, JSON.stringify({ action: "edited", pull_request: pullRequest }));
    return Bun.spawnSync(["bun", join(import.meta.dir, "pr-description.ts"), "--event", event], { env: { ...process.env, GITHUB_EVENT_PATH: "" } });
  };

  const passing = run({ title, number: 12, body: null });
  expect(passing.exitCode).toBe(0);
  expect(passing.stdout.toString()).toContain("Release Please can parse #12's squash commit");

  const failing = run({ title, number: 13, body: pr354Line });
  expect(failing.exitCode).toBe(1);
  expect(failing.stderr.toString()).toContain("Release Please would drop #13 from the release notes");
});
