/**
 * Check that Release Please can parse the commit a pull request's squash merge
 * will create. Release Please drops a commit it cannot parse from the changelog
 * and the release pull request, and its workflow still passes.
 *
 * The check rebuilds the squash commit the way GitHub composes it, applies a
 * commit-override block the way Release Please does, splits the message the
 * way Release Please does, and parses each part with the same
 * `@conventional-commits/parser` version Release Please bundles. It reproduces
 * Release Please 17.6.0 (release-please-action 5.0.0): `preprocessCommitMessage`,
 * `splitMessages`, and `parseConventionalCommits` in its `src/commit.ts`.
 *
 * Usage:
 *   bun scripts/pr-description.ts [--event <path>]
 *   bun scripts/pr-description.ts --title <title> --body-file <path> [--number <n>]
 *
 * `--event` reads a JSON object with `title`, `number`, and `body`, either at
 * its top level (`gh pr view --json number,title,body`) or under
 * `pull_request` (a GitHub event payload). It defaults to `GITHUB_EVENT_PATH`.
 */
import { parser } from "@conventional-commits/parser";
import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";

export type PullRequest = { title: string; number: number; body: string };

/** GitHub wraps squash commit body lines longer than this many characters. */
export const SQUASH_WRAP_WIDTH = 72;

const overrideBegin = "BEGIN_COMMIT_OVERRIDE";
const overrideEnd = "END_COMMIT_OVERRIDE";
const fence = /^\s*(`{3,}|~{3,})(.*)$/;

function wrapLine(line: string): string[] {
  if (line.length <= SQUASH_WRAP_WIDTH) return [line];
  const lines: string[] = [];
  let current = "";
  for (const word of line.trim().split(/\s+/)) {
    if (current === "") current = word;
    else if (current.length + 1 + word.length <= SQUASH_WRAP_WIDTH) current += ` ${word}`;
    else {
      lines.push(current);
      current = word;
    }
  }
  lines.push(current);
  return lines;
}

/**
 * The squash commit body GitHub composes from a description. GitHub keeps a
 * line of up to 72 characters as written. Outside a code fence it rewraps a
 * longer line greedily at 72 columns, dropping its indentation and collapsing
 * whitespace runs; a single word longer than that stays on its own line.
 * GitHub documents none of this; it reproduces this repository's squash
 * commits whose descriptions were not edited after merging.
 */
export function squashBody(description: string): string {
  // The open fence's marker. As in CommonMark, only a run of the same
  // character at least as long, with nothing after it, closes the fence. No
  // squash commit here has shown how GitHub treats nested or tilde fences.
  let open: string | undefined;
  const lines: string[] = [];
  for (const line of description.replace(/\r\n?/g, "\n").split("\n")) {
    const marker = fence.exec(line);
    if (open === undefined && marker && !(marker[1]![0] === "`" && marker[2]!.includes("`"))) {
      open = marker[1]!;
      lines.push(line);
    } else if (open !== undefined) {
      if (marker && marker[1]![0] === open[0] && marker[1]!.length >= open.length && !marker[2]!.trim()) open = undefined;
      lines.push(line);
    } else {
      lines.push(...wrapLine(line));
    }
  }
  return lines.join("\n").replace(/\s+$/, "");
}

/** The squash commit message GitHub composes from a pull request. */
export function squashMessage({ title, number, body }: PullRequest): string {
  const composed = squashBody(body);
  return composed.trim() ? `${title} (#${number})\n\n${composed}` : `${title} (#${number})`;
}

/** Release Please's `splitMessages`: nested commits and blank-line-separated conventional commits. */
function splitMessages(message: string): string[] {
  const [first = "", ...parts] = message.split("BEGIN_NESTED_COMMIT");
  const messages = [first];
  for (const part of parts) {
    const [nested = "", ...rest] = part.split("END_NESTED_COMMIT");
    messages.push(nested);
    messages[0] += rest.join("END_NESTED_COMMIT");
  }
  const conventional = messages[0]!.split(
    /\r?\n\r?\n(?=(?:feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(?:\(.*?\))?: )/,
  );
  return [...conventional.filter(Boolean), ...messages.slice(1)];
}

function parseProblems(message: string, source: string): string[] {
  const problems: string[] = [];
  for (const part of splitMessages(message)) {
    try {
      parser(part);
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      const position = /at (\d+):(\d+)/.exec(reason);
      // The parser trims its input before counting lines.
      const lineNumber = position ? Number(position[1]) : 1;
      const line = part.trim().split("\n")[lineNumber - 1] ?? "";
      const hint = lineNumber === 1
        ? "The first line must be a conventional commit header such as `fix: describe the change`."
        : "A line that starts with a word directly followed by `(` must close it with `)` before another " +
          "`(` and before the line ends. GitHub rewraps description lines longer than " +
          `${SQUASH_WRAP_WIDTH} characters, which can move any of their words to a line start, so ` +
          "reword nested or unclosed call syntax such as `foo(bar(` rather than moving it.";
      problems.push(`${source} does not parse (${reason}):\n  ${line}\n${hint}`);
    }
  }
  return problems;
}

function markerLines(body: string, marker: string): number {
  return body.split(/\r?\n/).filter((line) => line.trim() === marker).length;
}

/** Problems that would make Release Please drop or misread this pull request's commit. */
export function checkPullRequest(pullRequest: PullRequest): string[] {
  const body = pullRequest.body;
  const begins = body.split(overrideBegin).length - 1;
  if (begins === 0) {
    return parseProblems(squashMessage(pullRequest), "The squash commit message");
  }

  const ends = body.split(overrideEnd).length - 1;
  const override = (body.split(overrideBegin)[1] ?? "").split(overrideEnd)[0]!.trim();
  if (begins !== 1 || ends !== 1 || markerLines(body, overrideBegin) !== 1 || markerLines(body, overrideEnd) !== 1 ||
    body.indexOf(overrideEnd) < body.indexOf(overrideBegin) || !override) {
    return [
      `The description contains ${overrideBegin} outside a single complete commit-override block. ` +
      "Release Please treats any occurrence, even in backticks, as an override and replaces the commit " +
      `message with the text after it. Use exactly one ${overrideBegin} line, the replacement commit ` +
      `messages, and one ${overrideEnd} line; in prose, call it "a commit-override block".`,
    ];
  }
  return parseProblems(override, "The commit-override block");
}

function readPullRequest(argv: string[]): PullRequest {
  const { values } = parseArgs({
    args: argv,
    options: {
      event: { type: "string" },
      title: { type: "string" },
      "body-file": { type: "string" },
      number: { type: "string" },
    },
  });
  if (values.title !== undefined) {
    if (values["body-file"] === undefined) throw new Error("--title needs --body-file");
    return { title: values.title, number: Number(values.number ?? 1), body: readFileSync(values["body-file"], "utf8") };
  }
  const path = values.event ?? process.env.GITHUB_EVENT_PATH;
  if (!path) throw new Error("pass --event <path>, --title with --body-file, or set GITHUB_EVENT_PATH");
  const json = JSON.parse(readFileSync(path, "utf8")) as Partial<PullRequest> & { pull_request?: Partial<PullRequest> };
  const pullRequest = json.pull_request ?? json;
  if (typeof pullRequest.title !== "string" || typeof pullRequest.number !== "number") {
    throw new Error(`${path} has no pull request title and number`);
  }
  return { title: pullRequest.title, number: pullRequest.number, body: pullRequest.body ?? "" };
}

if (import.meta.main) {
  const pullRequest = readPullRequest(Bun.argv.slice(2));
  const problems = checkPullRequest(pullRequest);
  for (const problem of problems) console.error(`${problem}\n`);
  if (problems.length) {
    console.error(
      `pr-description: Release Please would drop #${pullRequest.number} from the release notes. ` +
      "Edit the title or description; see docs/agents/releases.md#release-notes.",
    );
    process.exit(1);
  }
  console.log(`pr-description: Release Please can parse #${pullRequest.number}'s squash commit`);
}
