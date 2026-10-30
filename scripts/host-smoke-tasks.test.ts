/**
 * Every task that can drive the host desktop must hold the host smoke lock, or
 * be listed here with the reason it need not. A new smoke or benchmark fails
 * this test until it is classified.
 */
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

type Tasks = Record<string, { run?: string | string[] }>;

/** Tasks the lock leaves alone, and why. */
const UNLOCKED: Record<string, string> = {
  "smoke:linux": "Linux only, on a private Xvfb display",
  "smoke:linux-clipboard": "Linux only, on a private Xvfb display",
  "smoke:linux-fullscreen": "Linux only, on a private Xvfb display",
  "smoke:linux-input": "Linux only, on a private Xvfb display",
  "smoke:linux-integration": "Linux only, on a private Xvfb display",
  "smoke:linux-palette": "Linux only, on a private Xvfb display",
  "smoke:linux-quake": "Linux only, on a private Xvfb display",
  "bench:renderer": "Linux only, on a private Xvfb display",
  "bench:engine": "headless Cargo benchmark",
  "bench:compression": "headless Cargo benchmark",
  "bench:links": "headless Cargo benchmark",
  "smoke:schema-editor": "drives a language server, not a window",
  "ci:smoke:build": "compiles only",
  "ci:smoke": "its ci:smoke:run step takes the lock after the builds",
  "ci:benchmarks": "its desktop benchmarks each take the lock; CI runs it on Linux",
  "vm:macos:smoke": "runs its smokes in a disposable Tart VM, not on the host display",
};

const LOCKS = /\bscripts\/(host-smoke-lock\.ts|headless\.sh)\b/;
/** Harnesses that open windows or drive input, and composites that run them. */
const DESKTOP = /\bscripts\/(check-[\w-]+\.ts|manual-[\w-]+\.ts|run-(scroll-benchmark|output-latency|renderer-bench|idle-benchmark|smoke-step)\.ts|headless\.sh)\b|\bmise run (smoke|bench|ci:smoke)[\w:-]*/;

const runText = (run: string | string[] | undefined) => [run ?? []].flat().join("\n");

/** Returns a problem for each desktop task that neither locks nor is listed, and each stale listing. */
function unclassified(tasks: Tasks, unlocked: Record<string, string>): string[] {
  const problems: string[] = [];
  const selected = new Set<string>();
  for (const [name, task] of Object.entries(tasks)) {
    const run = runText(task.run);
    if (!/^(smoke|bench|ci:smoke)/.test(name) && !DESKTOP.test(run)) continue;
    selected.add(name);
    const locked = LOCKS.test(run);
    if (name.startsWith("smoke:macos-") && !locked) problems.push(`${name} drives the macOS desktop without the host smoke lock`);
    else if (!locked && !(name in unlocked)) problems.push(`${name} must run through scripts/host-smoke-lock.ts or be listed with a reason`);
    else if (locked && name in unlocked) problems.push(`${name} takes the lock but is listed as unlocked`);
  }
  for (const name of Object.keys(unlocked)) {
    if (!selected.has(name)) problems.push(`${name} is listed as unlocked but is not a desktop task`);
  }
  return problems;
}

test("every desktop task holds the host smoke lock or says why not", () => {
  const tasks = (Bun.TOML.parse(readFileSync(join(import.meta.dir, "..", "mise.toml"), "utf8")) as { tasks: Tasks }).tasks;
  expect(unclassified(tasks, UNLOCKED)).toEqual([]);
});

test("the classification rejects an unlocked desktop task and a stale listing", () => {
  const tasks: Tasks = {
    "smoke:macos-new": { run: "bun scripts/check-new.ts target/debug/examples/new_smoke" },
    "bench:window": { run: ["cargo build", "bun scripts/run-scroll-benchmark.ts report"] },
    "release:notes": { run: "bun scripts/release-notes.ts" },
  };
  expect(unclassified(tasks, { "smoke:macos-new": "listed", "smoke:gone": "removed" })).toEqual([
    "smoke:macos-new drives the macOS desktop without the host smoke lock",
    "bench:window must run through scripts/host-smoke-lock.ts or be listed with a reason",
    "smoke:gone is listed as unlocked but is not a desktop task",
  ]);
});
