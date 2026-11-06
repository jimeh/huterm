import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  appendOccurrence, checkRegistry, classify, classifyLog, decideRetry, loadFlakes, longestLiteral, parseFlake,
  renderIndex, reportOccurrences, tableCell, withIndex, type Flake,
} from "./flakes.ts";

const NOW = "2026-11-06";

function entry(overrides: Record<string, string> = {}, body = "Evidence.\n"): string {
  const fields: Record<string, string> = {
    id: '"renderer-hang"',
    title: '"Renderer hang"',
    status: '"open"',
    kind: '"smoke"',
    platforms: '["linux-x86_64"]',
    retries: "1",
    review_by: '"2026-12-01"',
    ...overrides,
  };
  const selector = overrides.selector ?? 'steps = ["renderer"]';
  const signature = overrides.signature ?? 'match = ["renderer smoke timed out after \\\\d+ms"]\nreject = ["RENDERER_SMOKE prepared"]';
  const top = Object.entries(fields).filter(([key]) => !["selector", "signature"].includes(key)).map(([key, value]) => `${key} = ${value}`);
  return `+++\n${top.join("\n")}\n\n[selector]\n${selector}\n\n[signature]\n${signature}\n+++\n\n${body}`;
}

function flake(overrides: Record<string, string> = {}): Flake {
  const result = parseFlake(entry(overrides), `docs/agents/flakes/${JSON.parse(overrides.id ?? '"renderer-hang"')}.md`, NOW);
  expect(result.errors).toEqual([]);
  return result.flake!;
}

describe("parseFlake", () => {
  test("accepts a complete entry", () => {
    expect(flake().signature.reject).toEqual(["RENDERER_SMOKE prepared"]);
  });

  test("rejects entries that could retry too much or match too broadly", () => {
    const errors = (overrides: Record<string, string>) => parseFlake(entry(overrides), "docs/agents/flakes/renderer-hang.md", NOW).errors.join("\n");
    expect(errors({ retries: "2" })).toContain("at most 1 for smoke entries");
    expect(errors({ status: '"fixed"' })).toContain("a fixed entry must set retries = 0");
    expect(errors({ kind: '"bun"', retries: "1", selector: 'file = "scripts/flakes.ts"\ntests = ["x"]' })).toContain("at most 0 for bun entries");
    expect(errors({ signature: 'match = [".*"]' })).toContain("literal run of at least 12");
    expect(errors({ signature: 'match = ["(unclosed"]' })).toContain("invalid pattern");
    expect(errors({ platforms: '["windows-x86_64"]' })).toContain("platforms must list");
    expect(errors({ review_by: '"2027-06-01"' })).toContain("at most 90 days ahead");
    expect(errors({ review_by: "2026-12-01" })).toContain("quoted YYYY-MM-DD");
    expect(errors({ id: '"other"' })).toContain("must match the file name");
    expect(errors({ selector: 'binary = "huterm-core"' })).toContain("selector.binary does not apply to smoke entries");
    expect(errors({ selector: "" })).toContain("selector.steps is required");
    expect(parseFlake(entry({}, "# Title\n"), "docs/agents/flakes/renderer-hang.md", NOW).errors.join()).toContain("level-one heading");
    expect(parseFlake("no front matter", "x.md", NOW).errors.join()).toContain("missing TOML front matter");
  });

  test("measures literal runs, not metacharacters", () => {
    expect(longestLiteral("renderer smoke timed out after \\d+ms")).toBe("renderer smoke timed out after ".length);
    expect(longestLiteral("[a-z]+ .* \\d{3}")).toBeLessThan(3);
    expect(longestLiteral("w0\\.confirming=true")).toBe("w0.confirming=true".length);
    expect(longestLiteral("abc|defghijk")).toBe(8);
    expect(longestLiteral("abcdefg?h")).toBe(7);
  });
});

describe("classify", () => {
  const renderer = flake();
  const failure = { kind: "smoke" as const, platform: "linux-x86_64" as const, step: "renderer", text: "error: renderer smoke timed out after 15000ms\nRENDERER_SMOKE launching\n" };

  test("matches selector, platform, and signature", () => {
    expect(classify([renderer], failure)?.id).toBe("renderer-hang");
  });

  test("a reject pattern anywhere in the captured output vetoes the match", () => {
    expect(classify([renderer], { ...failure, text: `${failure.text}RENDERER_SMOKE prepared frame=1\n` })).toBeUndefined();
  });

  test("another step, platform, or message does not match", () => {
    expect(classify([renderer], { ...failure, step: "linux-input" })).toBeUndefined();
    expect(classify([renderer], { ...failure, platform: "linux-aarch64" })).toBeUndefined();
    expect(classify([renderer], { ...failure, text: "renderer exited 1" })).toBeUndefined();
  });

  test("a fixed entry wins over an open one that also matches", () => {
    const fixed = flake({ id: '"renderer-fixed"', status: '"fixed"', retries: "0" });
    expect(classify([renderer, fixed], failure)?.id).toBe("renderer-fixed");
  });

  test("rust entries select by binary and exact test name", () => {
    const rust = flake({ id: '"close"', kind: '"rust"', retries: "2", selector: 'binary = "huterm-gpui"\ntests = ["a::b"]', signature: 'match = ["timed out waiting for the click"]' });
    const base = { kind: "rust" as const, platform: "linux-x86_64" as const, binary: "huterm-gpui", test: "a::b", text: "timed out waiting for the click to close" };
    expect(classify([rust], base)?.id).toBe("close");
    expect(classify([rust], { ...base, test: "a::b_other" })).toBeUndefined();
    expect(classify([rust], { ...base, binary: "huterm-core" })).toBeUndefined();
  });
});

describe("decideRetry", () => {
  const base = { attempt: 1, retriedEntries: new Set<string>(), enabled: true, now: NOW };

  test("retries a matched open entry once", () => {
    expect(decideRetry(flake(), base)).toEqual({ retry: true, flake: flake(), overdue: false });
  });

  test("an exhausted budget fails", () => {
    const decision = decideRetry(flake(), { ...base, attempt: 2 });
    expect(decision.retry).toBe(false);
    expect(!decision.retry && decision.reason).toContain("may be a real regression");
  });

  test("a fixed entry's signature fails as a regression", () => {
    const decision = decideRetry(flake({ status: '"fixed"', retries: "0" }), base);
    expect(!decision.retry && decision.reason).toContain("recurred after its fix");
  });

  test("unmatched failures, classification-only entries, and local runs do not retry", () => {
    expect(decideRetry(undefined, base).retry).toBe(false);
    expect(decideRetry(flake({ retries: "0" }), base).retry).toBe(false);
    const local = decideRetry(flake(), { ...base, enabled: false });
    expect(!local.retry && local.reason).toContain("HUTERM_FLAKE_RETRY=1");
  });

  test("a third distinct entry in one job fails, but a known one may still retry", () => {
    const two = new Set(["first", "second"]);
    expect(decideRetry(flake(), { ...base, retriedEntries: two }).retry).toBe(false);
    expect(decideRetry(flake(), { ...base, retriedEntries: new Set(["renderer-hang", "second"]) }).retry).toBe(true);
  });

  test("an overdue entry still retries and says so", () => {
    expect(decideRetry(flake({ review_by: '"2026-11-01"' }), base)).toMatchObject({ retry: true, overdue: true });
  });
});

describe("registry files", () => {
  function registry(files: Record<string, string>, index = "# Known flakes\n\n<!-- flakes:index:start -->\n<!-- flakes:index:end -->\n"): string {
    const root = mkdtempSync(join(tmpdir(), "flakes-"));
    mkdirSync(join(root, "docs", "agents", "flakes"), { recursive: true });
    writeFileSync(join(root, "docs", "agents", "known-flakes.md"), index);
    for (const [name, text] of Object.entries(files)) writeFileSync(join(root, "docs", "agents", "flakes", name), text);
    return root;
  }

  test("a stale index fails and a regenerated one passes", () => {
    const root = registry({ "renderer-hang.md": entry() });
    expect(checkRegistry(root, NOW).errors.join()).toContain("index is stale");
    const path = join(root, "docs", "agents", "known-flakes.md");
    writeFileSync(path, withIndex(readFileSync(path, "utf8"), loadFlakes(root, NOW).flakes)!);
    expect(checkRegistry(root, NOW)).toEqual({ errors: [], warnings: [] });
  });

  test("broken links in entry bodies fail, and overdue entries warn", () => {
    const root = registry({ "renderer-hang.md": entry({ review_by: '"2026-11-01"' }, "See [the guide](../missing.md) and [smokes](../known-flakes.md#nowhere).\n") });
    const { errors, warnings } = checkRegistry(root, NOW);
    expect(errors.join("\n")).toContain("../missing.md does not exist");
    expect(errors.join("\n")).toContain("has no heading #nowhere");
    expect(warnings.join()).toContain("review overdue since 2026-11-01");
  });

  test("the index separates open and fixed entries", () => {
    const index = renderIndex([flake(), flake({ id: '"old-one"', status: '"fixed"', retries: "0" })]);
    expect(index.indexOf("renderer-hang")).toBeLessThan(index.indexOf("### Fixed"));
    expect(index.indexOf("old-one")).toBeGreaterThan(index.indexOf("### Fixed"));
  });
});

describe("recording", () => {
  test("appending an occurrence is idempotent per URL and keeps the entry valid", () => {
    const occurrence = { date: "2026-11-06", url: "https://github.com/jimeh/huterm/actions/runs/1/job/2", pr: 7, platform: "linux-x86_64" as const };
    const once = appendOccurrence(entry(), occurrence);
    expect(appendOccurrence(once, occurrence)).toBe(once);
    const parsed = parseFlake(once, "docs/agents/flakes/renderer-hang.md", NOW);
    expect(parsed.errors).toEqual([]);
    expect(parsed.flake?.occurrences).toEqual([occurrence]);
    expect(parsed.flake?.body).toBe("\nEvidence.\n");
  });

  test("a job log matches only a failed step that names the selector", () => {
    const step = (name: string, body: string) => `2026-11-05T01:02:03.0000000Z ##[group]Run HUTERM_CI_SMOKE_STEP=${name} mise run ci:smoke:step\n${body}\n`;
    const failed = step("renderer", "error: renderer smoke timed out after 15000ms\n##[error]Process completed with exit code 1.");
    expect(classifyLog([flake()], failed, "linux-x86_64").map((item) => item.id)).toEqual(["renderer-hang"]);
    expect(classifyLog([flake()], failed.replace("=renderer", "=linux-input"), "linux-x86_64")).toEqual([]);
    // The same text in a step that passed, or beside a failure in another step, is no match.
    expect(classifyLog([flake()], step("renderer", "renderer smoke timed out after 15000ms") + step("linux-input", "##[error]exit 1"), "linux-x86_64")).toEqual([]);
  });

  test("an infrastructure entry needs one of its steps to have failed", () => {
    const infra = flake({
      id: '"api-403"', kind: '"infra"', retries: "0",
      selector: 'step_names = ["Install job tools with Mise"]',
      signature: 'match = ["API rate limit exceeded"]',
    });
    const log = "##[group]Run mise run audit:scripts\nerror: API rate limit exceeded\n##[error]Process completed with exit code 1.\n";
    expect(classifyLog([infra], log, "linux-x86_64", ["Audit scripting dependencies"])).toEqual([]);
    // A candidate only: `record` asks for confirmation, since logs do not name the step that printed the message.
    expect(classifyLog([infra], log, "linux-x86_64", ["Install job tools with Mise"]).map((item) => item.id)).toEqual(["api-403"]);
  });

  test("a job log matches a Rust test only inside its failure block", () => {
    const rust = flake({
      id: '"close"', kind: '"rust"', retries: "2",
      selector: 'binary = "huterm-gpui"\ntests = ["window_tests::close"]',
      signature: 'match = ["timed out waiting for the click"]',
    });
    const passing = "        PASS [   0.8s] (1/3) huterm-gpui window_tests::close\n";
    const other = "        FAIL [   1.0s] (2/3) huterm-gpui window_tests::other\n  stderr: timed out waiting for the click\n";
    expect(classifyLog([rust], passing + other, "linux-x86_64")).toEqual([]);
    const failing = "        FAIL [  10.1s] (3/3) huterm-gpui window_tests::close\n  stderr: timed out waiting for the click to close\n     Summary [ 12s]\n";
    expect(classifyLog([rust], passing + failing, "linux-x86_64").map((item) => item.id)).toEqual(["close"]);
  });
});

describe("report records", () => {
  const job = { name: "Rust smoke (Linux x86_64)", html_url: "https://github.com/jimeh/huterm/actions/runs/9/job/1", run_attempt: 1 };
  const record = (passed: boolean) => ({
    event: "retried" as const, flake: "renderer-hang", kind: "smoke" as const, selector: "smoke:renderer", platform: "linux-x86_64" as const,
    attempt: 1, passed, run: { id: "9", attempt: "1", job: "smoke" },
  });

  test("a failure whose later rerun passed is one retried occurrence", () => {
    const occurrences = reportOccurrences([record(false), record(true)], [job], [flake()], { repository: "jimeh/huterm", runId: "9", date: "2026-11-06" });
    expect(occurrences.map(({ flake: entry, occurrence }) => [entry.id, occurrence.url, occurrence.retried])).toEqual([["renderer-hang", job.html_url, true]]);
  });

  test("summary cells escape backslashes and pipes", () => {
    expect(tableCell("a\\|b|c\nd")).toBe("a\\\\\\|b\\|c d");
  });
});

test("the repository registry is valid and indexed", () => {
  const { errors } = checkRegistry(undefined, NOW);
  expect(errors).toEqual([]);
});
