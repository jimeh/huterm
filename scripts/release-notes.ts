/**
 * Render the HTML release notes Sparkle's update window shows from Release
 * Please's `CHANGELOG.md`.
 *
 * The appcast offers only the newest release, so its notes list every release
 * back to `oldestNotesVersion`, each in a section named by its version. Sparkle
 * 2.5+ adds the `sparkle-installed-version` class to the section matching the
 * installed `CFBundleVersion`, and the stylesheet hides that section and every
 * older one, so the window lists exactly the releases the update brings.
 */
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";

const repoRoot = resolve(import.meta.dir, "..");
const defaultChangelog = join(repoRoot, "CHANGELOG.md");

export const repositoryUrl = "https://github.com/jimeh/huterm";

/** The first release that shipped Sparkle; older releases never see these notes. */
export const oldestNotesVersion = "0.6.0";

/** The sections Release Please writes, in its order, and the tag each item gets. */
const kinds = {
  breaking: { heading: "⚠ BREAKING CHANGES", tag: "Important" },
  feat: { heading: "Features", tag: "New" },
  fix: { heading: "Bug Fixes", tag: "Fixed" },
  perf: { heading: "Performance Improvements", tag: "Faster" },
  revert: { heading: "Reverts", tag: "Reverted" },
} as const;

export type SectionKind = keyof typeof kinds;

export interface ReleaseSection {
  kind: SectionKind;
  items: string[];
}

export interface Release {
  version: string;
  date: string;
  sections: ReleaseSection[];
}

/** A section whose heading has no tag, kept aside while its release is read. */
interface UntaggedSection {
  release: number;
  line: number;
  title: string;
  items: string[];
}

const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

// `## [0.17.0](compare link) (2026-10-08)`, or `## 0.1.0 (2026-09-09)` for the
// first release, which has nothing to compare with.
const releaseHeading = /^## (?:\[(\d+\.\d+\.\d+)\]\([^)]*\)|(\d+\.\d+\.\d+)) \((\d{4}-\d{2}-\d{2})\)$/;
const commitReference = /\s*\(\[[0-9a-f]{7,40}\]\([^)\s]*\)\)/g;
// The pull request and commit links Release Please ends an item with, and its
// `closes` list of issues.
const trailingReference = /^([\s\S]*?)(?:\s*\(\[(?:#\d+|[0-9a-f]{7,40})\]\([^)]*\)\)|,\s*closes(?:[,\s]+\[[^\]]*#\d+\]\([^)]*\))+)$/;
const closesList = /,\s*closes(?:[,\s]+\[[^\]]*#\d+\]\([^)]*\))+$/;

function versionParts(version: string): number[] {
  return version.split(".").map(Number);
}

function isIncluded(version: string): boolean {
  const [major = 0, minor = 0, patch = 0] = versionParts(version);
  const [floorMajor = 0, floorMinor = 0, floorPatch = 0] = versionParts(oldestNotesVersion);
  if (major !== floorMajor) return major > floorMajor;
  if (minor !== floorMinor) return minor > floorMinor;
  return patch >= floorPatch;
}

/** `2026-10-11` as `Oct 11, 2026`, or undefined for a date that does not exist. */
function displayDate(date: string): string | undefined {
  const [year = 0, month = 0, day = 0] = date.split("-").map(Number);
  const parsed = new Date(Date.UTC(year, month - 1, day));
  if (parsed.getUTCFullYear() !== year || parsed.getUTCMonth() !== month - 1 || parsed.getUTCDate() !== day) return undefined;
  return `${months[month - 1]} ${day}, ${year}`;
}

function withoutReferences(item: string): string {
  let text = item;
  for (let match = trailingReference.exec(text); match; match = trailingReference.exec(text)) text = match[1]!;
  return text;
}

/**
 * An item's text and pull request numbers. Release Please lists a breaking
 * change again under its type with the commit link added, so the commit is
 * left out.
 */
function identity(item: string): string {
  const pullRequests = [...item.matchAll(/\(\[#(\d+)\]\(/g)].map(match => match[1]);
  return `${withoutReferences(item)} #${pullRequests.join(",")}`;
}

/**
 * Reads the releases Release Please wrote, newest first, down to
 * `oldestNotesVersion`. The newest is always read, even when older. A line,
 * section, or date it does not recognize fails rather than leaving something
 * out of the notes.
 */
export function parseChangelog(changelog: string): Release[] {
  const releases: Release[] = [];
  const untagged: UntaggedSection[] = [];
  let current: UntaggedSection | ReleaseSection | undefined;
  // Set on the line after a breaking change or its continuation, where a
  // multi-line `BREAKING CHANGE:` footer continues unindented.
  let inFooter = false;
  for (const [index, rawLine] of changelog.split("\n").entries()) {
    const line = rawLine.replace(/\r$/, "");
    const lineNumber = index + 1;
    const continuesFooter = inFooter;
    inFooter = false;
    // Only blank lines and the `# Changelog` title may precede the first release.
    if (line.trim() === "" || (releases.length === 0 && line.startsWith("# "))) continue;
    const heading = releaseHeading.exec(line);
    if (heading) {
      const version = heading[1] ?? heading[2]!;
      if (releases.length > 0 && !isIncluded(version)) break;
      const date = heading[3]!;
      if (!displayDate(date)) throw new Error(`CHANGELOG.md line ${lineNumber} has an invalid date ${date}`);
      releases.push({ version, date, sections: [] });
      current = undefined;
      continue;
    }
    const release = releases.at(-1);
    const sectionHeading = /^### (.+)$/.exec(line);
    if (release && sectionHeading) {
      const title = sectionHeading[1]!.trim();
      const kind = (Object.keys(kinds) as SectionKind[]).find(candidate => kinds[candidate].heading === title);
      if (kind) {
        current = { kind, items: [] };
        release.sections.push(current);
      } else {
        current = { release: releases.length - 1, line: lineNumber, title, items: [] };
        untagged.push(current);
      }
      continue;
    }
    const item = /^\* (.+)$/.exec(line);
    if (current && item) {
      current.items.push(item[1]!.trim());
      inFooter = isBreaking(current);
      continue;
    }
    // A wrapped item continues on an indented line in any section. Only a
    // breaking change footer continues unindented, and never with a heading.
    if (current && current.items.length > 0 && (line.startsWith(" ") || (continuesFooter && !line.startsWith("#")))) {
      current.items[current.items.length - 1] += ` ${line.trim()}`;
      inFooter = isBreaking(current);
      continue;
    }
    throw new Error(`CHANGELOG.md line ${lineNumber} is not a release heading, section heading, or list item`);
  }
  mergeDuplicates(releases, untagged);
  return releases;
}

function isBreaking(section: UntaggedSection | ReleaseSection): boolean {
  return "kind" in section && section.kind === "breaking";
}

/**
 * Release Please lists a breaking change again under its type, so each release
 * keeps it once, as Important, and removes the type's copy. A `closes` list
 * only that copy carries moves to the breaking change. A type Release Please
 * otherwise leaves out, such as `build!`, has no section in the notes, so a
 * section with an unknown heading is accepted only when every item in it
 * repeats a breaking change. Any other item would be dropped, so it fails.
 */
function mergeDuplicates(releases: Release[], untagged: UntaggedSection[]): void {
  releases.forEach((release, releaseIndex) => {
    const breaking = new Map<string, { section: ReleaseSection; index: number }>();
    for (const section of release.sections.filter(isBreaking)) {
      section.items.forEach((item, index) => breaking.set(identity(item), { section, index }));
    }
    const merge = (item: string): boolean => {
      const original = breaking.get(identity(item));
      if (!original) return false;
      const closes = closesList.exec(item.replace(commitReference, ""))?.[0];
      const kept = original.section.items[original.index]!;
      if (closes && !closesList.test(kept)) original.section.items[original.index] = kept + closes;
      return true;
    };
    for (const section of release.sections) {
      if (!isBreaking(section)) section.items = section.items.filter(item => !merge(item));
    }
    for (const section of untagged.filter(candidate => candidate.release === releaseIndex)) {
      if (section.items.length === 0 || !section.items.every(merge)) {
        throw new Error(`CHANGELOG.md line ${section.line} has an unknown section ${section.title}`);
      }
    }
  });
}

export function escapeHtml(text: string): string {
  return text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

/**
 * Commit subjects start in lowercase. The first letter after any leading bold
 * or link syntax is capitalized, so a scope such as `**deps:**` is too. Code is
 * left as written.
 */
function capitalized(text: string): string {
  return text.replace(/^([*[]*)(\p{Ll})/u, (_, prefix: string, letter: string) => `${prefix}${letter.toUpperCase()}`);
}

function httpsUrl(value: string): string | undefined {
  try {
    const url = new URL(value);
    return url.protocol === "https:" ? url.href : undefined;
  } catch {
    return undefined;
  }
}

function strong(escaped: string): string {
  return escaped.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
}

/** Escapes prose and renders its bold text and https links. Other links keep only their text. */
function prose(markdown: string): string {
  let html = "";
  let offset = 0;
  for (const match of markdown.matchAll(/\[([^\]]*)\]\(([^)\s]*)\)/g)) {
    html += strong(escapeHtml(markdown.slice(offset, match.index)));
    const text = strong(escapeHtml(match[1]!));
    const url = httpsUrl(match[2]!);
    html += url ? `<a href="${escapeHtml(url)}">${text}</a>` : text;
    offset = match.index + match[0].length;
  }
  return html + strong(escapeHtml(markdown.slice(offset)));
}

/** Renders the inline Markdown Release Please writes: code spans, bold scopes, and links. */
function inlineHtml(markdown: string): string {
  const parts = markdown.split("`");
  // An unpaired backtick opens no code span.
  if (parts.length % 2 === 0) return prose(markdown);
  return parts.map((part, index) => index % 2 === 0 ? prose(part) : `<code>${escapeHtml(part)}</code>`).join("");
}

const stylesheet = `:root {
  color-scheme: light dark;
  --label: rgb(0 0 0 / 0.85);
  --secondary: rgb(0 0 0 / 0.55);
  --separator: rgb(0 0 0 / 0.1);
  --code: rgb(0 0 0 / 0.05);
  --blue: #0064e1;
  --green: #1f7a37;
  --orange: #b8500a;
  --purple: #8944ab;
  --gray: #6e6e73;
}
@media (prefers-color-scheme: dark) {
  :root {
    --label: rgb(255 255 255 / 0.86);
    --secondary: rgb(255 255 255 / 0.58);
    --separator: rgb(255 255 255 / 0.11);
    --code: rgb(255 255 255 / 0.08);
    --blue: #409cff;
    --green: #30d158;
    --orange: #ffa634;
    --purple: #da8fff;
    --gray: #aeaeb2;
  }
}
html { background: transparent; }
body { margin: 0; padding: 12px 14px 14px; font: 13px/1.42 -apple-system, BlinkMacSystemFont, system-ui, sans-serif; color: var(--label); }
a { color: var(--blue); text-decoration: none; }
a:hover { text-decoration: underline; }
code { font: 11.5px ui-monospace, SFMono-Regular, Menlo, monospace; padding: 0 3px; border-radius: 4px; background: var(--code); }
section { padding: 10px 0 11px; border-top: 1px solid var(--separator); }
section:first-child { border-top: 0; padding-top: 2px; }
header { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; margin-bottom: 6px; }
h2 { font-size: 14px; font-weight: 600; letter-spacing: -0.005em; margin: 0; }
h2 a { color: inherit; }
time { color: var(--secondary); font-size: 12px; font-variant-numeric: tabular-nums; white-space: nowrap; }
ul { margin: 0; padding: 0; list-style: none; }
li { display: flex; align-items: baseline; gap: 8px; margin: 5px 0; }
.tag {
  flex: none; width: 62px; padding: 1px 0; border-radius: 5px; text-align: center;
  font-size: 10px; font-weight: 600; letter-spacing: 0.02em;
  color: var(--kind); background: color-mix(in srgb, var(--kind) 14%, transparent);
}
.breaking { --kind: var(--orange); }
.feat { --kind: var(--blue); }
.fix { --kind: var(--green); }
.perf { --kind: var(--purple); }
.revert { --kind: var(--gray); }
footer { padding-top: 10px; border-top: 1px solid var(--separator); color: var(--secondary); font-size: 12px; }
.sparkle-installed-version, .sparkle-installed-version ~ section { display: none; }`;

/**
 * One section per release: its linked version and date on one row, then its
 * changes in Release Please's order, each tagged with its kind. The footer is not a section, so it stays visible however many sections
 * Sparkle hides.
 */
export function renderReleaseNotes(releases: readonly Release[]): string {
  let html = `<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
${stylesheet}
</style>
</head>
<body>
`;
  for (const release of releases) {
    const version = escapeHtml(release.version);
    html += `<section data-sparkle-version="${version}">\n`;
    html += `<header><h2><a href="${repositoryUrl}/releases/tag/v${version}">Version ${version}</a></h2>`;
    html += `<time datetime="${escapeHtml(release.date)}">${escapeHtml(displayDate(release.date) ?? release.date)}</time></header>\n`;
    html += "<ul>\n";
    for (const section of release.sections) {
      for (const item of section.items) {
        const text = inlineHtml(capitalized(item.replace(commitReference, "")));
        html += `<li class="${section.kind}"><span class="tag">${kinds[section.kind].tag}</span><span>${text}</span></li>\n`;
      }
    }
    html += "</ul>\n</section>\n";
  }
  html += `<footer><a href="${repositoryUrl}">Huterm on GitHub</a> · <a href="${repositoryUrl}/releases">All releases</a></footer>\n`;
  return `${html}</body>\n</html>\n`;
}

/** Renders the notes for `version`, which must be the changelog's newest release. */
export function releaseNotesHtml(changelog: string, version: string): string {
  const releases = parseChangelog(changelog);
  if (releases[0]?.version !== version) throw new Error(`CHANGELOG.md must begin with release ${version}`);
  return renderReleaseNotes(releases);
}

export async function renderReleaseNotesFile(version: string, changelogPath = defaultChangelog): Promise<string> {
  return releaseNotesHtml(await readFile(changelogPath, "utf8"), version);
}

/** Emulates Sparkle marking the installed version's section. */
export function markInstalledVersion(html: string, version: string): string {
  const section = `<section data-sparkle-version="${escapeHtml(version)}">`;
  if (!html.includes(section)) throw new Error(`release notes have no section for ${version}`);
  return html.replace(section, `<section class="sparkle-installed-version" data-sparkle-version="${escapeHtml(version)}">`);
}

function previewOptions(args: string[]): Record<"version" | "installed" | "output", string | undefined> {
  const options: Record<"version" | "installed" | "output", string | undefined> = { version: undefined, installed: undefined, output: undefined };
  for (let index = 0; index < args.length; index += 2) {
    const name = args[index]!.replace(/^--/, "");
    const value = args[index + 1];
    if (!(name in options) || !args[index]!.startsWith("--")) throw new Error(`unexpected argument ${args[index]}; expected --version, --installed, or --output`);
    if (!value) throw new Error(`${args[index]} requires a value`);
    options[name as keyof typeof options] = value;
  }
  return options;
}

/** Writes the notes Sparkle would show, optionally as seen from an installed version. */
async function preview(args: string[]): Promise<void> {
  const options = previewOptions(args);
  const changelog = await readFile(defaultChangelog, "utf8");
  const version = options.version ?? parseChangelog(changelog)[0]?.version;
  if (!version) throw new Error("CHANGELOG.md has no releases");
  let html = releaseNotesHtml(changelog, version);
  if (options.installed) html = markInstalledVersion(html, options.installed);
  const output = resolve(options.output ?? join(repoRoot, "target/release-notes-preview.html"));
  await mkdir(dirname(output), { recursive: true });
  await writeFile(output, html);
  console.log(`wrote release notes for ${version}${options.installed ? ` as seen from ${options.installed}` : ""} to ${output}`);
}

if (import.meta.main) {
  try {
    await preview(Bun.argv.slice(2));
  } catch (error) {
    console.error(`release notes failed: ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
  }
}
