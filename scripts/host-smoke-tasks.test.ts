/**
 * Every desktop harness a mise task runs on the host must run under the host
 * smoke lock. A new smoke or benchmark fails this test until its harness
 * command is wrapped, or until it is listed here with the reason it need not be.
 */
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

type Tasks = Record<string, { run?: string | string[] }>;

/** Tasks that run a desktop harness on the host without the lock, and why. */
const UNLOCKED: Record<string, string> = {};

/** Harnesses that open windows or drive input on the display they run against. */
const HARNESS = /\bscripts\/(check-(?!scroll-benchmark\.ts)[\w-]+\.ts|manual-[\w-]+\.ts|run-(scroll-benchmark|output-latency|renderer-bench|idle-benchmark|smoke-step)\.ts)/g;
const WRAPPER = /\bscripts\/(host-smoke-lock\.ts|headless\.sh)\b/;
const COMPOSITE = /\bmise run ([\w:-]+)/g;

const runText = (run: string | string[] | undefined) => [run ?? []].flat().join("\n");

/** The command line holding `index`, with backslash continuations joined, up to `index`. */
function lineBefore(text: string, index: number): string {
  const start = text.lastIndexOf("\n", index - 1) + 1;
  let line = text.slice(start, index);
  for (let end = start - 1; end > 0 && text[end - 1] === "\\"; ) {
    const previous = text.lastIndexOf("\n", end - 1) + 1;
    line = text.slice(previous, end - 1) + line;
    end = previous - 1;
  }
  return line;
}

/** The simple command holding `index`, up to `index`: the line after its last unquoted `;`, `&`, `|`, or parenthesis. */
function commandBefore(text: string, index: number): string {
  const line = lineBefore(text, index);
  let quote = "";
  let start = 0;
  for (let at = 0; at < line.length; at++) {
    const char = line[at]!;
    if (quote) {
      if (char === quote) quote = "";
      else if (char === "\\" && quote === '"') at++;
    } else if (char === "'" || char === '"') quote = char;
    else if (char === "\\") at++;
    else if (";&|()".includes(char)) start = at + 1;
  }
  return line.slice(start);
}

/** Xvfb gives a harness a private display: on the same command, or inside its quoted `sh -c` script. */
function underXvfb(text: string, index: number): boolean {
  if (commandBefore(text, index).includes("xvfb-run")) return true;
  const xvfb = text.lastIndexOf("xvfb-run", index);
  return xvfb >= 0 && (text.slice(xvfb, index).match(/'/g)?.length ?? 0) % 2 === 1;
}

/** A task's harness commands that run on the host display without the lock. */
function unlockedHarnesses(name: string, tasks: Tasks, seen = new Set<string>()): string[] {
  seen.add(name);
  const text = runText(tasks[name]?.run);
  // A task that reruns itself under the wrapper holds the lock for all of its commands.
  if (new RegExp(`host-smoke-lock\\.ts mise run ${name}(?![\\w:-])`).test(text)) return [];
  const found: string[] = [];
  for (const match of text.matchAll(HARNESS)) {
    if (!WRAPPER.test(commandBefore(text, match.index)) && !underXvfb(text, match.index)) found.push(match[0]);
  }
  for (const match of text.matchAll(COMPOSITE)) {
    const target = match[1]!;
    if (!seen.has(target) && unlockedHarnesses(target, tasks, seen).length > 0) found.push(`mise run ${target}`);
  }
  return found;
}

/** Returns a problem for each task with an unlocked harness that is not listed, and for each stale listing. */
function unclassified(tasks: Tasks, unlocked: Record<string, string>): string[] {
  const problems: string[] = [];
  for (const name of Object.keys(tasks)) {
    const found = unlockedHarnesses(name, tasks);
    if (name.startsWith("smoke:macos-") && !WRAPPER.test(runText(tasks[name]!.run))) problems.push(`${name} drives the macOS desktop without the host smoke lock`);
    else if (found.length > 0 && (name.startsWith("smoke:macos-") || !(name in unlocked))) problems.push(`${name} runs ${found.join(", ")} without the host smoke lock`);
    else if (found.length === 0 && name in unlocked) problems.push(`${name} is listed as unlocked but runs no unlocked desktop harness`);
  }
  for (const name of Object.keys(unlocked)) if (!(name in tasks)) problems.push(`${name} is listed as unlocked but is not a task`);
  return problems;
}

test("every desktop harness a task runs on the host holds the host smoke lock", () => {
  const tasks = (Bun.TOML.parse(readFileSync(join(import.meta.dir, "..", "mise.toml"), "utf8")) as { tasks: Tasks }).tasks;
  expect(unclassified(tasks, UNLOCKED)).toEqual([]);
});

test("the classification checks each harness command, not each task", () => {
  const tasks: Tasks = {
    "smoke:macos-new": { run: "cargo build\nbun scripts/check-new.ts target/debug/examples/new_smoke" },
    "smoke:macos-added": { run: "bun scripts/host-smoke-lock.ts bun scripts/check-one.ts a\nbun scripts/check-two.ts b" },
    "smoke:macos-wrapped": { run: "cargo build\nbun scripts/host-smoke-lock.ts bun scripts/check-one.ts a" },
    "smoke:macos-chained": { run: "bun scripts/host-smoke-lock.ts bun scripts/check-one.ts a; bun scripts/check-two.ts b && bun scripts/check-three.ts c" },
    "smoke:macos-quoted": { run: "cargo build && bun scripts/host-smoke-lock.ts sh -c 'true; bun scripts/check-one.ts a'" },
    "smoke:linux-new": { run: "xvfb-run -a -s '-screen 0 1x1x24' \\\n  bun scripts/check-new.ts a\nxvfb-run sh -c '\n  bun scripts/run-scroll-benchmark.ts \"$1\"\n' sh x" },
    "bench:window": { run: ["cargo build", "bun scripts/run-scroll-benchmark.ts report"] },
    "bench:headless": { run: ["bash scripts/headless.sh bun scripts/run-output-latency.ts a"] },
    "ci:all": { run: "mise run bench:window\nmise run bench:headless" },
    "ci:self": { run: "[ -n \"$HELD\" ] || exec bun scripts/host-smoke-lock.ts mise run ci:self\nbun scripts/check-one.ts a" },
    "release:notes": { run: "bun scripts/release-notes.ts" },
  };
  expect(unclassified(tasks, { "smoke:macos-new": "listed", "release:notes": "stale", "smoke:gone": "removed" })).toEqual([
    "smoke:macos-new drives the macOS desktop without the host smoke lock",
    "smoke:macos-added runs scripts/check-two.ts without the host smoke lock",
    "smoke:macos-chained runs scripts/check-two.ts, scripts/check-three.ts without the host smoke lock",
    "bench:window runs scripts/run-scroll-benchmark.ts without the host smoke lock",
    "ci:all runs mise run bench:window without the host smoke lock",
    "release:notes is listed as unlocked but runs no unlocked desktop harness",
    "smoke:gone is listed as unlocked but is not a task",
  ]);
});
