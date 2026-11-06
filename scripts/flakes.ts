/**
 * The known-flake registry: one Markdown file per flake under
 * docs/agents/flakes, with TOML front matter between `+++` lines.
 *
 * The registry decides which failures a CI runner may retry. A retry needs an
 * entry whose selector, platform, and every `match` pattern fit the failure
 * and whose `reject` patterns all miss it. Budgets stay small, every retried
 * pass is reported, and a failure that exhausts its budget still fails.
 *
 * Commands:
 *   check              validate entries, links, and the generated index
 *   index              rewrite the index block in docs/agents/known-flakes.md
 *   record <run-url>   append occurrences from a CI run's flake reports or logs
 *   classify           classify failure text from stdin (see --help)
 */
import { appendFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join, relative, resolve } from "node:path";
import { anchors } from "./agent-guides.ts";

export const PLATFORMS = ["linux-x86_64", "linux-aarch64", "macos-arm64", "macos-x86_64"] as const;
export const KINDS = ["rust", "smoke", "bun", "infra"] as const;
export const STATUSES = ["open", "fixing", "fixed"] as const;
/** Reruns an entry may allow per failure. Bun and infra entries only classify. */
export const MAX_RETRIES: Record<Kind, number> = { rust: 2, smoke: 1, bun: 0, infra: 0 };
/** Distinct entries one CI job may retry before a further match fails it. */
export const JOB_ENTRY_CAP = 2;
/** The furthest ahead a review date may be set. */
export const MAX_REVIEW_DAYS = 90;
/** A pattern must contain a literal run at least this long, so it cannot match everything. */
export const MIN_LITERAL = 12;

export type Platform = typeof PLATFORMS[number];
export type Kind = typeof KINDS[number];
export type Status = typeof STATUSES[number];

export type Selector = {
  /** rust: nextest binary ID, such as `huterm-core` or `huterm-core::pty_integration`. */
  binary?: string;
  /** rust and bun: exact test names. */
  tests?: string[];
  /** smoke: the `HUTERM_CI_SMOKE_STEP` names. */
  steps?: string[];
  /** bun: the test file, relative to the repository root. */
  file?: string;
  /** infra: the workflow step names. */
  step_names?: string[];
};

export type Occurrence = {
  date: string;
  /** Job or run URL. */
  url?: string;
  pr?: number;
  sha?: string;
  platform?: Platform;
  /** The CI runner retried this failure and the retry passed. */
  retried?: boolean;
  local?: boolean;
  note?: string;
};

export type Flake = {
  id: string;
  title: string;
  status: Status;
  kind: Kind;
  platforms: Platform[];
  retries: number;
  review_by: string;
  owner?: string;
  selector: Selector;
  signature: { match: string[]; reject: string[] };
  occurrences: Occurrence[];
  path: string;
  body: string;
};

/** A failure to classify. `text` holds every stream the runner captured. */
export type Failure = {
  kind: Kind;
  platform: Platform;
  text: string;
  binary?: string;
  test?: string;
  step?: string;
  file?: string;
  stepName?: string;
};

export const repositoryRoot = resolve(import.meta.dir, "..");
export const flakeDirectory = join("docs", "agents", "flakes");
export const indexPath = join("docs", "agents", "known-flakes.md");
const INDEX_START = "<!-- flakes:index:start -->";
const INDEX_END = "<!-- flakes:index:end -->";

/** This machine's platform name, or undefined on an unsupported host. */
export function currentPlatform(): Platform | undefined {
  const os = process.platform === "darwin" ? "macos" : process.platform === "linux" ? "linux" : undefined;
  const arch = process.arch === "x64" ? "x86_64" : process.arch === "arm64" ? (os === "macos" ? "arm64" : "aarch64") : undefined;
  const name = `${os}-${arch}`;
  return (PLATFORMS as readonly string[]).includes(name) ? name as Platform : undefined;
}

const datePattern = /^\d{4}-\d{2}-\d{2}$/;
const idPattern = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

/** The longest run of characters a regular expression matches literally. */
export function longestLiteral(pattern: string): number {
  let longest = 0;
  let run = 0;
  let inClass = false;
  for (let index = 0; index < pattern.length; index++) {
    const char = pattern[index]!;
    if (inClass) {
      if (char === "\\") index++;
      else if (char === "]") inClass = false;
      continue;
    }
    if (char === "\\") {
      const next = pattern[index + 1];
      index++;
      // An escaped punctuation character is literal; `\d`, `\s`, and similar are classes.
      if (next !== undefined && !/[A-Za-z0-9]/.test(next)) { run++; longest = Math.max(longest, run); } else run = 0;
      continue;
    }
    if (char === "[") { inClass = true; run = 0; continue; }
    if ("?*{".includes(char)) {
      // A quantifier makes the previous character optional or repeated.
      run = Math.max(0, run - 1);
      if (char === "{") while (index < pattern.length && pattern[index] !== "}") index++;
      continue;
    }
    if (".()^$+|".includes(char)) { run = 0; continue; }
    run++;
    longest = Math.max(longest, run);
  }
  return longest;
}

function splitFrontMatter(text: string): { front: string; body: string } | undefined {
  const match = /^\+\+\+\n([\s\S]*?)\n\+\+\+\n?/.exec(text);
  if (!match) return undefined;
  return { front: match[1]!, body: text.slice(match[0].length) };
}

function stringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

function addDays(date: string, days: number): string {
  const value = new Date(`${date}T00:00:00Z`);
  value.setUTCDate(value.getUTCDate() + days);
  return value.toISOString().slice(0, 10);
}

/** Today's UTC date, overridable for tests. */
export function today(): string {
  return process.env.HUTERM_FLAKES_TODAY ?? new Date().toISOString().slice(0, 10);
}

/** Parse and validate one entry file. Errors are `path: message` strings. */
export function parseFlake(text: string, path: string, now = today()): { flake?: Flake; errors: string[] } {
  const errors: string[] = [];
  const fail = (message: string) => errors.push(`${path}: ${message}`);
  const parts = splitFrontMatter(text);
  if (!parts) {
    fail("missing TOML front matter between +++ lines");
    return { errors };
  }
  let data: Record<string, unknown>;
  try {
    data = Bun.TOML.parse(parts.front) as Record<string, unknown>;
  } catch (error) {
    fail(`invalid TOML: ${(error as Error).message}`);
    return { errors };
  }
  const known = new Set(["id", "title", "status", "kind", "platforms", "retries", "review_by", "owner", "selector", "signature", "occurrences"]);
  for (const key of Object.keys(data)) if (!known.has(key)) fail(`unknown key ${key}`);

  const id = data.id;
  if (typeof id !== "string" || !idPattern.test(id)) fail("id must be lowercase kebab-case");
  else if (basename(path, ".md") !== id) fail(`id ${id} must match the file name`);
  if (typeof data.title !== "string" || !data.title.trim()) fail("title is required");
  const status = data.status as Status;
  if (!STATUSES.includes(status)) fail(`status must be one of ${STATUSES.join(", ")}`);
  const kind = data.kind as Kind;
  if (!KINDS.includes(kind)) fail(`kind must be one of ${KINDS.join(", ")}`);
  const platforms = data.platforms;
  if (!stringArray(platforms) || !platforms.length || platforms.some((platform) => !(PLATFORMS as readonly string[]).includes(platform))) {
    fail(`platforms must list at least one of ${PLATFORMS.join(", ")}`);
  }
  const retries = data.retries;
  if (typeof retries !== "number" || !Number.isInteger(retries) || retries < 0) fail("retries must be a non-negative integer");
  else if (KINDS.includes(kind) && retries > MAX_RETRIES[kind]) fail(`retries may be at most ${MAX_RETRIES[kind]} for ${kind} entries`);
  else if (status === "fixed" && retries > 0) fail("a fixed entry must set retries = 0");
  const reviewBy = data.review_by;
  if (typeof reviewBy !== "string" || !datePattern.test(reviewBy)) fail("review_by must be a quoted YYYY-MM-DD date");
  else if (reviewBy > addDays(now, MAX_REVIEW_DAYS)) fail(`review_by may be at most ${MAX_REVIEW_DAYS} days ahead`);
  if (data.owner !== undefined && typeof data.owner !== "string") fail("owner must be a string");

  const selector = (data.selector ?? {}) as Selector;
  if (typeof selector !== "object" || Array.isArray(selector)) fail("selector must be a table");
  const selectorKeys: Record<Kind, (keyof Selector)[]> = {
    rust: ["binary", "tests"],
    smoke: ["steps"],
    bun: ["file", "tests"],
    infra: ["step_names"],
  };
  if (KINDS.includes(kind)) {
    for (const key of Object.keys(selector)) {
      if (!selectorKeys[kind].includes(key as keyof Selector)) fail(`selector.${key} does not apply to ${kind} entries`);
    }
    for (const key of selectorKeys[kind]) {
      const value = selector[key];
      const valid = key === "binary" || key === "file" ? typeof value === "string" && value.length > 0 : stringArray(value) && value.length > 0;
      if (!valid) fail(`selector.${key} is required for ${kind} entries`);
    }
    if (kind === "bun" && typeof selector.file === "string" && !existsSync(join(repositoryRoot, selector.file))) {
      fail(`selector.file ${selector.file} does not exist`);
    }
  }

  const signature = (data.signature ?? {}) as { match?: unknown; reject?: unknown };
  const match = signature.match;
  const reject = signature.reject ?? [];
  if (!stringArray(match) || !match.length) fail("signature.match needs at least one pattern");
  if (!stringArray(reject)) fail("signature.reject must be a list of patterns");
  for (const pattern of [...(stringArray(match) ? match : []), ...(stringArray(reject) ? reject : [])]) {
    try {
      new RegExp(pattern, "m");
    } catch (error) {
      fail(`invalid pattern ${JSON.stringify(pattern)}: ${(error as Error).message}`);
    }
  }
  if (stringArray(match) && match.length && !match.some((pattern) => longestLiteral(pattern) >= MIN_LITERAL)) {
    fail(`signature.match needs a pattern with a literal run of at least ${MIN_LITERAL} characters`);
  }

  const occurrences = (data.occurrences ?? []) as Occurrence[];
  if (!Array.isArray(occurrences)) fail("occurrences must be an array of tables");
  else occurrences.forEach((occurrence, index) => {
    const at = `occurrences[${index}]`;
    if (typeof occurrence.date !== "string" || !datePattern.test(occurrence.date)) fail(`${at}.date must be a quoted YYYY-MM-DD date`);
    if (occurrence.url !== undefined && !/^https:\/\/github\.com\//.test(occurrence.url)) fail(`${at}.url must be a GitHub URL`);
    if (occurrence.platform !== undefined && !(PLATFORMS as readonly string[]).includes(occurrence.platform)) fail(`${at}.platform is unknown`);
    for (const key of Object.keys(occurrence)) {
      if (!["date", "url", "pr", "sha", "platform", "retried", "local", "note"].includes(key)) fail(`${at}.${key} is unknown`);
    }
  });

  if (/^# /m.test(parts.body)) fail("the body must not use a level-one heading; the title comes from the front matter");
  if (errors.length) return { errors };
  return {
    errors,
    flake: {
      id: id as string,
      title: data.title as string,
      status,
      kind,
      platforms: platforms as Platform[],
      retries: retries as number,
      review_by: reviewBy as string,
      owner: data.owner as string | undefined,
      selector,
      signature: { match: match as string[], reject: reject as string[] },
      occurrences,
      path,
      body: parts.body,
    },
  };
}

/** Load every entry under `root`'s flake directory. */
export function loadFlakes(root = repositoryRoot, now = today()): { flakes: Flake[]; errors: string[] } {
  const directory = join(root, flakeDirectory);
  const flakes: Flake[] = [];
  const errors: string[] = [];
  const names = existsSync(directory) ? readdirSync(directory).filter((name) => name.endsWith(".md")).sort() : [];
  for (const name of names) {
    const path = join(flakeDirectory, name);
    const result = parseFlake(readFileSync(join(root, path), "utf8"), path, now);
    errors.push(...result.errors);
    if (result.flake) flakes.push(result.flake);
  }
  return { flakes, errors };
}

function selectorMatches(flake: Flake, failure: Failure): boolean {
  const { selector } = flake;
  switch (flake.kind) {
    case "rust":
      return selector.binary === failure.binary && !!failure.test && !!selector.tests?.includes(failure.test);
    case "smoke":
      return !!failure.step && !!selector.steps?.includes(failure.step);
    case "bun":
      return selector.file === failure.file && !!failure.test && !!selector.tests?.includes(failure.test);
    case "infra":
      return !!failure.stepName && !!selector.step_names?.includes(failure.stepName);
  }
}

/** Whether every `match` pattern and no `reject` pattern fits `text`. */
export function signatureMatches(flake: Flake, text: string): boolean {
  return flake.signature.match.every((pattern) => new RegExp(pattern, "m").test(text))
    && !flake.signature.reject.some((pattern) => new RegExp(pattern, "m").test(text));
}

/** The entry that explains `failure`, if any. */
export function classify(flakes: Flake[], failure: Failure): Flake | undefined {
  const matches = flakes.filter((flake) => flake.kind === failure.kind
    && flake.platforms.includes(failure.platform)
    && selectorMatches(flake, failure)
    && signatureMatches(flake, failure.text));
  // When a fixed entry also matches, the failure may be its recurrence, which
  // must fail rather than pass on another entry's retry.
  return matches.find((flake) => flake.status === "fixed") ?? matches[0];
}

export type RetryDecision =
  | { retry: true; flake: Flake; overdue: boolean }
  | { retry: false; flake?: Flake; reason: string };

/**
 * Whether a runner may rerun a failure. `attempt` counts this failure's runs
 * so far (1 after the first failure); `retriedEntries` holds the entries this
 * job already retried.
 */
export function decideRetry(flake: Flake | undefined, options: {
  attempt: number;
  retriedEntries: ReadonlySet<string>;
  enabled: boolean;
  now?: string;
}): RetryDecision {
  if (!flake) return { retry: false, reason: "no registered flake matches this failure" };
  const file = `docs/agents/flakes/${flake.id}.md`;
  if (flake.status === "fixed") {
    return { retry: false, flake, reason: `known signature recurred after its fix: ${flake.id} (${file}); treat it as a regression` };
  }
  if (flake.retries === 0) return { retry: false, flake, reason: `${flake.id} is registered for classification only (${file})` };
  if (!options.enabled) return { retry: false, flake, reason: `${flake.id} matches (${file}); local runs do not retry, set HUTERM_FLAKE_RETRY=1 to opt in` };
  if (options.attempt > flake.retries) {
    return { retry: false, flake, reason: `${flake.id} failed ${options.attempt} times, exhausting its ${flake.retries} retr${flake.retries === 1 ? "y" : "ies"}; a repeated failure may be a real regression (${file})` };
  }
  if (!options.retriedEntries.has(flake.id) && options.retriedEntries.size >= JOB_ENTRY_CAP) {
    return { retry: false, flake, reason: `${flake.id} matches, but this job already retried ${JOB_ENTRY_CAP} other flakes (${[...options.retriedEntries].join(", ")}); that many at once points at a regression or a broken runner` };
  }
  return { retry: true, flake, overdue: flake.review_by < (options.now ?? today()) };
}

/** Whether runners in this environment may retry: CI sets HUTERM_FLAKE_RETRY=1. */
export function retryEnabled(env: NodeJS.ProcessEnv = process.env): boolean {
  return env.HUTERM_FLAKE_RETRY === "1";
}

export type ReportRecord = {
  /** `retried`: a rerun of a matched failure; `failed`: a failure that was not retried. */
  event: "retried" | "failed";
  flake?: string;
  kind: Kind;
  selector: string;
  platform: Platform;
  attempt: number;
  /** For `retried`: whether that rerun passed. */
  passed?: boolean;
  overdue?: boolean;
  reason?: string;
  excerpt?: string;
  run?: { id?: string; attempt?: string; job?: string; sha?: string; repository?: string };
};

/** The directory that collects this job's flake reports, if any. */
export function reportDirectory(env: NodeJS.ProcessEnv = process.env): string | undefined {
  return env.HUTERM_FLAKE_REPORT_DIR || undefined;
}

function runContext(env: NodeJS.ProcessEnv): ReportRecord["run"] {
  if (!env.GITHUB_RUN_ID) return undefined;
  return { id: env.GITHUB_RUN_ID, attempt: env.GITHUB_RUN_ATTEMPT, job: env.GITHUB_JOB, sha: env.GITHUB_SHA, repository: env.GITHUB_REPOSITORY };
}

/** Append one record to the job's `flake-report.jsonl`. */
export function appendReport(record: ReportRecord, env: NodeJS.ProcessEnv = process.env): void {
  const directory = reportDirectory(env);
  if (!directory) return;
  mkdirSync(directory, { recursive: true });
  appendFileSync(join(directory, "flake-report.jsonl"), `${JSON.stringify({ ...record, run: record.run ?? runContext(env) })}\n`);
}

/** The records already in the job's report. */
export function readReport(env: NodeJS.ProcessEnv = process.env): ReportRecord[] {
  const directory = reportDirectory(env);
  if (!directory) return [];
  const path = join(directory, "flake-report.jsonl");
  if (!existsSync(path)) return [];
  return readFileSync(path, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line) as ReportRecord);
}

/** Entries this job has already retried, from the shared report. */
export function retriedEntries(env: NodeJS.ProcessEnv = process.env): Set<string> {
  return new Set(readReport(env).filter((record) => record.event === "retried" && record.flake).map((record) => record.flake!));
}

/** Print a GitHub annotation in Actions and a plain line elsewhere. */
export function annotate(level: "warning" | "error" | "notice", title: string, message: string, env: NodeJS.ProcessEnv = process.env): void {
  if (env.GITHUB_ACTIONS === "true") {
    const escape = (value: string) => value.replace(/%/g, "%25").replace(/\r/g, "%0D").replace(/\n/g, "%0A");
    console.log(`::${level} title=${escape(title).replace(/[:,]/g, " ")}::${escape(message)}`);
  } else {
    console.error(`${level}: ${title}: ${message}`);
  }
}

/** Step summary files that already have this process's table header. */
const summaryHeaders = new Set<string>();

/** Escape text for one Markdown table cell. */
export function tableCell(text: string): string {
  return text.replace(/\\/g, "\\\\").replace(/\|/g, "\\|").replace(/\n/g, " ");
}

/** Add a row to this step's summary table of flake retries. */
export function summarize(record: ReportRecord, env: NodeJS.ProcessEnv = process.env): void {
  const path = env.GITHUB_STEP_SUMMARY;
  if (!path) return;
  // Actions gives every step its own summary file, so one header per process and file suffices.
  const header = summaryHeaders.has(path)
    ? ""
    : "\n### Known flake retries\n\n| Flake | Selector | Attempt | Result | Note |\n| --- | --- | --- | --- | --- |\n";
  summaryHeaders.add(path);
  const result = record.event === "retried" ? (record.passed ? "rerun passed" : "rerun failed") : "failed";
  const note = [record.overdue ? "review overdue" : "", record.reason ?? ""].filter(Boolean).join("; ");
  appendFileSync(path, `${header}| ${tableCell(record.flake ?? "none")} | \`${tableCell(record.selector)}\` | ${record.attempt} | ${result} | ${tableCell(note)} |\n`);
}

/** Report a rerun of a matched failure in every channel: annotation, summary, and report. */
export function reportRetry(record: ReportRecord & { event: "retried"; flake: string }, env: NodeJS.ProcessEnv = process.env): void {
  appendReport(record, env);
  summarize(record, env);
  const overdue = record.overdue ? " Its review date has passed: review overdue." : "";
  const result = record.passed ? "passed on rerun" : "failed again on rerun";
  annotate(record.passed ? "warning" : "error", `Known flake ${record.flake}`, `${record.selector} ${result} (attempt ${record.attempt + 1}). See docs/agents/flakes/${record.flake}.md.${overdue}`, env);
}

/** Report a failure the runner did not retry, with the registry's reason. */
export function reportFailure(record: ReportRecord & { event: "failed" }, env: NodeJS.ProcessEnv = process.env): void {
  appendReport(record, env);
  if (record.flake) summarize(record, env);
  if (record.reason) console.error(`flakes: ${record.selector}: ${record.reason}`);
}

// ---------------------------------------------------------------------------
// Index

function occurrenceSummary(flake: Flake): { count: string; last: string } {
  const ci = flake.occurrences.filter((occurrence) => !occurrence.local).length;
  const local = flake.occurrences.length - ci;
  const parts = [ci ? `${ci} CI` : "", local ? `${local} local` : ""].filter(Boolean);
  const last = flake.occurrences.map((occurrence) => occurrence.date).sort().at(-1) ?? "never";
  return { count: parts.join(", ") || "none recorded", last };
}

/** The generated index block, without its markers. */
export function renderIndex(flakes: Flake[]): string {
  const table = (rows: Flake[]) => [
    "| Flake | Kind | Platforms | Seen | Last | Retries | Review by |",
    "| --- | --- | --- | --- | --- | --- | --- |",
    ...rows.map((flake) => {
      const { count, last } = occurrenceSummary(flake);
      const status = flake.status === "fixing" ? ` (fixing${flake.owner ? ` in ${flake.owner}` : ""})` : "";
      return `| [${flake.title}](flakes/${flake.id}.md)${status} | ${flake.kind} | ${flake.platforms.join(", ")} | ${count} | ${last} | ${flake.retries} | ${flake.review_by} |`;
    }),
  ].join("\n");
  const byLastSeen = (a: Flake, b: Flake) => occurrenceSummary(b).last.localeCompare(occurrenceSummary(a).last) || a.id.localeCompare(b.id);
  const open = flakes.filter((flake) => flake.status !== "fixed").sort(byLastSeen);
  const fixed = flakes.filter((flake) => flake.status === "fixed").sort(byLastSeen);
  return [
    "### Open",
    "",
    open.length ? table(open) : "None.",
    "",
    "### Fixed, under observation",
    "",
    "A failure that matches one of these fails CI as a regression. Remove an",
    "entry once its review date passes without a recurrence.",
    "",
    fixed.length ? table(fixed) : "None.",
  ].join("\n");
}

/** `text` with its index block replaced, or undefined when the markers are missing. */
export function withIndex(text: string, flakes: Flake[]): string | undefined {
  const start = text.indexOf(INDEX_START);
  const end = text.indexOf(INDEX_END);
  if (start < 0 || end < start) return undefined;
  return `${text.slice(0, start + INDEX_START.length)}\n\n${renderIndex(flakes)}\n\n${text.slice(end)}`;
}

// ---------------------------------------------------------------------------
// Check

function relativeLinks(markdown: string): { line: number; target: string }[] {
  const result: { line: number; target: string }[] = [];
  let fence = false;
  markdown.split("\n").forEach((line, index) => {
    if (/^\s*(```|~~~)/.test(line)) fence = !fence;
    if (fence) return;
    for (const match of line.matchAll(/\]\(([^()\s]+)\)/g)) {
      if (!/^[a-z][a-z0-9+.-]*:/i.test(match[1]!)) result.push({ line: index + 1, target: match[1]! });
    }
  });
  return result;
}

/** Validation errors and warnings for the registry under `root`. */
export function checkRegistry(root = repositoryRoot, now = today()): { errors: string[]; warnings: string[] } {
  const { flakes, errors } = loadFlakes(root, now);
  const warnings: string[] = [];
  const ids = new Set<string>();
  for (const flake of flakes) {
    if (ids.has(flake.id)) errors.push(`${flake.path}: duplicate id ${flake.id}`);
    ids.add(flake.id);
    if (flake.review_by < now) {
      warnings.push(`${flake.path}: review overdue since ${flake.review_by}; ${flake.status === "fixed" ? "remove the entry if it has not recurred" : "review the entry and set a new review_by"}`);
    }
    const file = join(root, flake.path);
    for (const { line, target } of relativeLinks(readFileSync(file, "utf8"))) {
      const [pathPart, anchor] = target.split("#") as [string, string | undefined];
      const destination = pathPart ? resolve(dirname(file), pathPart) : file;
      if (!existsSync(destination)) errors.push(`${flake.path}:${line}: link target ${target} does not exist`);
      else if (anchor && destination.endsWith(".md") && !anchors(readFileSync(destination, "utf8")).has(anchor)) {
        errors.push(`${flake.path}:${line}: link target ${target} has no heading #${anchor}`);
      }
    }
  }
  const index = join(root, indexPath);
  const text = existsSync(index) ? readFileSync(index, "utf8") : "";
  const expected = withIndex(text, flakes);
  if (expected === undefined) errors.push(`${indexPath}: missing ${INDEX_START} and ${INDEX_END} markers`);
  else if (expected !== text && !errors.length) errors.push(`${indexPath}: the index is stale; run \`mise run flakes:index\``);
  return { errors, warnings };
}

// ---------------------------------------------------------------------------
// Record

/** Platform named by a CI job label such as "Checks (Linux aarch64)". */
export function platformFromJobName(name: string): Platform | undefined {
  if (/Linux x86_64/i.test(name)) return "linux-x86_64";
  if (/Linux aarch64/i.test(name)) return "linux-aarch64";
  if (/macOS arm64/i.test(name)) return "macos-arm64";
  if (/Ghostty ReleaseSafe|Policy|Scroll benchmarks/i.test(name)) return "linux-x86_64";
  if (/macOS/i.test(name)) return "macos-arm64";
  return undefined;
}

/**
 * The parts of a job log that belong to failures: a nextest `FAIL` or
 * `TIMEOUT` block and libtest `---- name stdout ----` block for each failed
 * Rust test, the output before each Bun `(fail)` line, and each workflow step
 * that reported an `##[error]`. Each part names what failed, so a passing
 * test or step whose name appears elsewhere in the log never matches.
 */
export function failureBlocks(log: string): { name: string; text: string }[] {
  const lines = log.replace(/\x1b\[[0-9;]*[A-Za-z]/g, "").split("\n").map((line) => line.replace(/^\d{4}-\d{2}-\d{2}T[\d:.]+Z /, ""));
  const blocks: { name: string; text: string }[] = [];
  const nextest = /^\s*(PASS|FAIL|TIMEOUT|SIGSEGV|SIGABRT|SIGKILL|SIGTERM|SKIP|SLOW|LEAK|TRY \d+ FAIL)\s+\[[^\]]*\]\s+(?:\([^)]*\)\s+)?(\S+)\s+(\S+)\s*$/;
  let bunStart = 0;
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index]!;
    const result = nextest.exec(line);
    if (result && !["PASS", "SKIP", "SLOW", "LEAK"].includes(result[1]!)) {
      let end = index + 1;
      while (end < lines.length && !nextest.test(lines[end]!) && !/^\s*Summary \[/.test(lines[end]!)) end++;
      blocks.push({ name: `${result[2]} ${result[3]}`, text: lines.slice(index, end).join("\n") });
    }
    const libtest = /^---- (\S+) std(?:out|err) ----$/.exec(line.trim());
    if (libtest) {
      let end = index + 1;
      while (end < lines.length && !/^---- \S+ std(?:out|err) ----$|^failures:$/.test(lines[end]!.trim())) end++;
      blocks.push({ name: libtest[1]!, text: lines.slice(index, end).join("\n") });
    }
    const bun = /^\((pass|fail|skip|todo)\) (.*?)(?: \[[\d.]+m?s\])?$/.exec(line.trim());
    if (bun) {
      if (bun[1] === "fail") blocks.push({ name: bun[2]!, text: lines.slice(bunStart, index + 1).join("\n") });
      bunStart = index + 1;
    }
  }
  // Workflow steps: each `##[group]Run` header starts one; keep those that errored.
  let header = "";
  let section: string[] = [];
  const flush = () => {
    if (section.some((line) => line.includes("##[error]"))) blocks.push({ name: header, text: section.join("\n") });
  };
  for (const line of lines) {
    const step = /##\[group\]Run (.*)$/.exec(line);
    if (step) {
      flush();
      header = step[1]!;
      section = [];
    }
    section.push(line);
  }
  flush();
  return blocks;
}

/**
 * Entries that a failed job's log points at. Used when a run has no flake
 * report, for example a failure before the runner could classify it. An entry
 * matches a failure block that names its selector, and its signature must fit
 * that block alone.
 */
export function classifyLog(flakes: Flake[], log: string, platform: Platform, failedSteps: readonly string[] = []): Flake[] {
  const blocks = failureBlocks(log);
  return flakes.filter((flake) => {
    if (!flake.platforms.includes(platform)) return false;
    return blocks.some((block) => {
      const named = flake.kind === "rust" ? (flake.selector.tests ?? []).some((test) => block.name === `${flake.selector.binary} ${test}` || block.name === test)
        : flake.kind === "smoke" ? (flake.selector.steps ?? []).some((step) => block.name.includes(`HUTERM_CI_SMOKE_STEP=${step} `))
          : flake.kind === "bun" ? (flake.selector.tests ?? []).some((test) => block.name.includes(test))
            // Logs do not name `uses:` steps, so infrastructure entries rely on the job's failed step names.
            : (flake.selector.step_names ?? []).some((name) => failedSteps.includes(name));
      return named && signatureMatches(flake, block.text);
    });
  });
}

/**
 * `text` with `occurrence` appended to its front matter, or unchanged when an
 * occurrence with the same URL is already recorded.
 */
export function appendOccurrence(text: string, occurrence: Occurrence): string {
  const parts = splitFrontMatter(text);
  if (!parts) throw new Error("entry has no front matter");
  const existing = (Bun.TOML.parse(parts.front) as { occurrences?: Occurrence[] }).occurrences ?? [];
  if (occurrence.url && existing.some((item) => item.url === occurrence.url)) return text;
  const lines = ["", "[[occurrences]]"];
  for (const [key, value] of Object.entries(occurrence)) {
    if (value === undefined) continue;
    lines.push(`${key} = ${typeof value === "string" ? JSON.stringify(value) : String(value)}`);
  }
  return `+++\n${parts.front.replace(/\s+$/, "")}\n${lines.join("\n")}\n+++\n${parts.body}`;
}

/** Workflow job keys (GITHUB_JOB) and the start of their display names. */
const JOB_NAMES: Record<string, string> = { checks: "Checks", smoke: "Rust smoke", releasesafe: "Ghostty ReleaseSafe" };

export type GhJob = {
  id: number; name: string; conclusion: string | null; html_url: string; head_sha: string; started_at: string; run_attempt: number;
  steps?: { name: string; conclusion: string | null }[];
};

async function gh(args: string[]): Promise<string> {
  const proc = Bun.spawn(["gh", ...args], { stdout: "pipe", stderr: "pipe" });
  const [stdout, stderr, code] = await Promise.all([new Response(proc.stdout).text(), new Response(proc.stderr).text(), proc.exited]);
  if (code !== 0) throw new Error(`gh ${args.join(" ")}: ${stderr.trim()}`);
  return stdout;
}

/**
 * One occurrence per flake and job from a run's flake report records. A
 * failure that failed one rerun and passed the next counts as retried.
 */
export function reportOccurrences(
  records: ReportRecord[],
  jobs: Pick<GhJob, "name" | "html_url" | "run_attempt">[],
  flakes: Flake[],
  context: { repository: string; runId: string; date: string; pr?: number; sha?: string; onlyJob?: boolean },
): { flake: Flake; occurrence: Occurrence }[] {
  const byKey = new Map<string, { flake: Flake; occurrence: Occurrence }>();
  for (const item of records) {
    const flake = flakes.find((candidate) => candidate.id === item.flake);
    if (!flake) continue;
    const prefix = JOB_NAMES[item.run?.job ?? ""];
    const job = jobs.find((candidate) => !!prefix && candidate.name.startsWith(prefix)
      && platformFromJobName(candidate.name) === item.platform && String(candidate.run_attempt) === item.run?.attempt);
    if (context.onlyJob && !job) continue;
    const url = job?.html_url ?? `https://github.com/${context.repository}/actions/runs/${context.runId}`;
    const passed = item.event === "retried" && item.passed === true;
    const key = `${flake.id} ${url}`;
    const existing = byKey.get(key);
    if (existing) existing.occurrence.retried ||= passed;
    else byKey.set(key, { flake, occurrence: { date: context.date, url, pr: context.pr, sha: context.sha, platform: item.platform, retried: passed } });
  }
  return [...byKey.values()];
}

/** Record every flake a CI run hit, from its flake reports or else its failed job logs. */
export async function record(url: string, root = repositoryRoot): Promise<string[]> {
  const parsed = /github\.com\/([^/]+\/[^/]+)\/actions\/runs\/(\d+)(?:\/job\/(\d+))?/.exec(url);
  if (!parsed) throw new Error(`expected a GitHub Actions run or job URL, received ${url}`);
  const [, repository, runId, jobId] = parsed as unknown as [string, string, string, string | undefined];
  const { flakes, errors } = loadFlakes(root);
  if (errors.length) throw new Error(`fix the registry first:\n${errors.join("\n")}`);
  const run = JSON.parse(await gh(["api", `repos/${repository}/actions/runs/${runId}`])) as { head_sha: string; created_at: string; pull_requests?: { number: number }[]; run_attempt: number };
  // A failure retried by rerunning the job lives in an earlier attempt, so read every attempt's jobs.
  const jobs = (JSON.parse(await gh(["api", "--paginate", "--slurp", `repos/${repository}/actions/runs/${runId}/jobs?filter=all&per_page=100`])) as { jobs: GhJob[] }[])
    .flatMap((page) => page.jobs)
    .filter((job) => !jobId || String(job.id) === jobId);
  const date = run.created_at.slice(0, 10);
  // `pull_requests` is empty once the branch is gone, so fall back to the commit's PRs.
  const pr = run.pull_requests?.[0]?.number
    ?? (JSON.parse(await gh(["api", `repos/${repository}/commits/${run.head_sha}/pulls`]).catch(() => "[]")) as { number: number }[])[0]?.number;
  const sha = run.head_sha.slice(0, 8);
  const notes: string[] = [];
  const add = (flake: Flake, occurrence: Occurrence) => {
    const file = join(root, flake.path);
    const before = readFileSync(file, "utf8");
    const after = appendOccurrence(before, occurrence);
    if (after === before) {
      notes.push(`${flake.id}: already recorded ${occurrence.url}`);
      return;
    }
    writeFileSync(file, after);
    notes.push(`${flake.id}: recorded ${occurrence.url}`);
  };

  // Retried passes leave no failed job, so read them from the flake reports.
  const reports = join(root, "target", "flake-reports", runId);
  try {
    await gh(["run", "download", runId, "--repo", repository, "--pattern", "flake-report-*", "--dir", reports]);
  } catch (error) {
    if (!/no valid artifacts|no artifact matches/i.test((error as Error).message)) notes.push(`no flake reports downloaded: ${(error as Error).message}`);
  }
  const records: ReportRecord[] = [];
  if (existsSync(reports)) {
    for (const artifact of readdirSync(reports)) {
      const path = join(reports, artifact, "flake-report.jsonl");
      if (!existsSync(path)) continue;
      records.push(...readFileSync(path, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line) as ReportRecord));
    }
  }
  const fromReports = reportOccurrences(records, jobs, flakes, { repository, runId, date, pr, sha, onlyJob: !!jobId });
  const reported = new Set(fromReports.map(({ flake, occurrence }) => `${flake.id} ${occurrence.url}`));
  for (const { flake, occurrence } of fromReports) add(flake, occurrence);

  for (const job of jobs.filter((candidate) => candidate.conclusion === "failure")) {
    const platform = platformFromJobName(job.name);
    if (!platform) {
      notes.push(`${job.name}: unknown platform, skipped`);
      continue;
    }
    // Job logs carry the runner's color codes; failureBlocks strips them.
    const log = await gh(["api", "--allow-escape-sequences", `repos/${repository}/actions/jobs/${job.id}/logs`]);
    // A job timeout cancels the running step rather than failing it.
    const failedSteps = (job.steps ?? []).filter((step) => step.conclusion === "failure" || step.conclusion === "cancelled").map((step) => step.name);
    const matches = classifyLog(flakes, log, platform, failedSteps).filter((flake) => !reported.has(`${flake.id} ${job.html_url}`));
    if (!matches.length && ![...reported].some((key) => key.endsWith(` ${job.html_url}`))) {
      notes.push(`${job.name}: no registered flake matches; if it is a new flake, add an entry under ${flakeDirectory}`);
    }
    for (const flake of matches) {
      // Logs do not say which step printed a line, so an infrastructure match
      // may pair one step's failure with another's message: confirm it by hand.
      if (flake.kind === "infra") notes.push(`${job.name}: may be ${flake.id}; check the failed step, then add an occurrence to ${flake.path} by hand`);
      else add(flake, { date, url: job.html_url, pr, sha, platform });
    }
  }
  return notes;
}

// ---------------------------------------------------------------------------
// CLI

function usage(): never {
  console.error([
    "usage: bun scripts/flakes.ts check | index | record <run-or-job-url>",
    "       bun scripts/flakes.ts classify --kind <kind> [--binary B --test T | --step S | --file F --test T | --step-name N] [--platform P] < failure-text",
  ].join("\n"));
  process.exit(2);
}

if (import.meta.main) {
  const [command, ...args] = Bun.argv.slice(2);
  if (command === "check") {
    const { errors, warnings } = checkRegistry();
    for (const warning of warnings) annotate("warning", "Known flake review", warning);
    for (const error of errors) console.error(error);
    if (errors.length) {
      console.error(`flakes: ${errors.length} problem(s)`);
      process.exit(1);
    }
    console.log(`flakes: ${loadFlakes().flakes.length} entries valid and indexed`);
  } else if (command === "index") {
    const { flakes, errors } = loadFlakes();
    if (errors.length) {
      for (const error of errors) console.error(error);
      process.exit(1);
    }
    const path = join(repositoryRoot, indexPath);
    const updated = withIndex(readFileSync(path, "utf8"), flakes);
    if (updated === undefined) throw new Error(`${indexPath}: missing index markers`);
    writeFileSync(path, updated);
    console.log(`flakes: indexed ${flakes.length} entries in ${relative(process.cwd(), path)}`);
  } else if (command === "record") {
    if (!args[0]) usage();
    for (const note of await record(args[0])) console.log(note);
  } else if (command === "classify") {
    const option = (name: string) => {
      const index = args.indexOf(`--${name}`);
      return index >= 0 ? args[index + 1] : undefined;
    };
    const kind = option("kind") as Kind | undefined;
    const platform = (option("platform") ?? currentPlatform()) as Platform | undefined;
    if (!kind || !KINDS.includes(kind) || !platform) usage();
    const text = await new Response(Bun.stdin.stream()).text();
    const flake = classify(loadFlakes().flakes, {
      kind, platform, text,
      binary: option("binary"), test: option("test"), step: option("step"), file: option("file"), stepName: option("step-name"),
    });
    if (!flake) {
      console.log("no registered flake matches");
      process.exit(1);
    }
    console.log(`${flake.id} (${flake.status}): ${flake.path}`);
  } else {
    usage();
  }
}
