import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { markInstalledVersion, oldestNotesVersion, parseChangelog, releaseNotesHtml, renderReleaseNotesFile } from "./release-notes.ts";

const repo = "https://github.com/jimeh/huterm";
const commit = (sha: string) => `([${sha.slice(0, 7)}](${repo}/commit/${sha}))`;
const pr = (number: number) => `([#${number}](${repo}/issues/${number}))`;

const changelog = `# Changelog

## [1.2.0](${repo}/compare/v1.1.0...v1.2.0) (2026-10-11)


### ⚠ BREAKING CHANGES

* **config:** drop the \`legacy\` table ${pr(30)}

### Features

* **config:** drop the \`legacy\` table ${pr(30)} ${commit("a".repeat(40))}
* add split panes that wrap
  across two lines ${pr(31)} ${commit("b".repeat(40))}


### Bug Fixes

* keep focus after closing a tab ${pr(32)} ${commit("c".repeat(40))}


### Performance Improvements

* render fewer frames ${pr(33)} ${commit("d".repeat(40))}

## [1.1.0](${repo}/compare/v1.0.0...v1.1.0) (2026-10-01)


### Reverts

* restore the old tab bar ${pr(29)} ${commit("e".repeat(40))}
`;

test("release notes render every section, link references, and drop commits", () => {
  const html = releaseNotesHtml(changelog, "1.2.0");
  const sections = [...html.matchAll(/<section data-sparkle-version="([^"]+)">/g)].map(match => match[1]);
  expect(sections).toEqual(["1.2.0", "1.1.0"]);
  expect(html).toContain(`<h2><a href="${repo}/releases/tag/v1.2.0">Version 1.2.0</a></h2><time datetime="2026-10-11">Oct 11, 2026</time>`);
  expect(html).toContain(`<li class="breaking"><span class="tag">Important</span><span><strong>Config:</strong> drop the <code>legacy</code> table (<a href="${repo}/issues/30">#30</a>)</span></li>`);
  // The breaking change Release Please repeats under Features appears once.
  expect(html.match(/drop the <code>legacy<\/code> table/g)).toHaveLength(1);
  expect(html).toContain(`<li class="feat"><span class="tag">New</span><span>Add split panes that wrap across two lines (<a href="${repo}/issues/31">#31</a>)</span></li>`);
  expect(html).toContain(`<li class="fix"><span class="tag">Fixed</span><span>Keep focus after closing a tab`);
  expect(html).toContain(`<li class="perf"><span class="tag">Faster</span><span>Render fewer frames`);
  expect(html).toContain(`<li class="revert"><span class="tag">Reverted</span><span>Restore the old tab bar`);
  expect(html).not.toContain("/commit/");
  expect(html).not.toContain("aaaaaaa");
  // The footer follows the sections without being one, so hiding them leaves it visible.
  expect(html).toMatch(new RegExp(`</section>\\n<footer><a href="${repo}">[^<]+</a> · <a href="${repo}/releases">[^<]+</a></footer>\\n</body>`));
  expect(html).toContain(".sparkle-installed-version, .sparkle-installed-version ~ section { display: none; }");
  expect(html).not.toContain("light-dark(");
  expect(html).not.toContain("<script");
});

test("release notes escape markup and emit only https links", () => {
  const html = releaseNotesHtml(`## 0.9.0 (2026-10-11)

### Bug Fixes

* quote "names" & reject <script> tags in [docs](https://example.com/a?b=1&c="2") and [scripts](javascript:alert) or [plain](http://example.com)
`, "0.9.0");
  expect(html).toContain(`Quote &quot;names&quot; &amp; reject &lt;script&gt; tags in <a href="https://example.com/a?b=1&amp;c=%222%22">docs</a> and scripts or plain`);
  expect(html).not.toContain("javascript:");
  expect(html).not.toContain('href="http://');
});

test("release notes stop at the first Sparkle release but always include the newest", () => {
  const releases = (versions: string[]) => versions.map(version => `## [${version}](${repo}/compare) (2026-09-12)\n\n### Features\n\n* change ${version}\n`).join("\n");
  expect(oldestNotesVersion).toBe("0.6.0");
  expect(parseChangelog(releases(["0.7.0", "0.6.0", "0.5.0"])).map(release => release.version)).toEqual(["0.7.0", "0.6.0"]);
  expect(parseChangelog(releases(["0.4.0", "0.3.0"])).map(release => release.version)).toEqual(["0.4.0"]);
});

test("release notes tolerate a type section that only repeats breaking changes", () => {
  const html = releaseNotesHtml(`## 1.0.0 (2026-10-11)

### ⚠ BREAKING CHANGES

* require macOS 15 ${pr(40)}

### Build System

* require macOS 15 ${pr(40)} ${commit("f".repeat(40))}
`, "1.0.0");
  expect(html.match(/Require macOS 15/g)).toHaveLength(1);
});

test("release notes keep a multi-line breaking change that continues unindented", () => {
  const html = releaseNotesHtml(`## 1.0.0 (2026-10-11)

### ⚠ BREAKING CHANGES

* \`[window] tabs\` moved to \`[tabs]\`.
Configs that still use it fail to parse.
`, "1.0.0");
  expect(html).toContain("moved to <code>[tabs]</code>. Configs that still use it fail to parse.</span></li>");
});

test("release notes reject content they would otherwise drop", () => {
  const release = "## 1.0.0 (2026-10-11)\n\n### Features\n\n* add tabs\n";
  expect(() => parseChangelog(`${release}\nSome trailing prose\n`)).toThrow("CHANGELOG.md line 7 is not a release heading, section heading, or list item");
  // Only a breaking change footer continues unindented.
  expect(() => parseChangelog(`${release}Some trailing prose\n`)).toThrow("CHANGELOG.md line 6 is not a release heading, section heading, or list item");
  expect(() => parseChangelog(`${release}## 0.9.0\n\n### Features\n\n* older change\n`)).toThrow("CHANGELOG.md line 6 is not a release heading, section heading, or list item");
  expect(() => parseChangelog(`# Changelog\n\n### Security\n\n* patch tabs\n\n${release}`)).toThrow("CHANGELOG.md line 3 is not a release heading, section heading, or list item");
  expect(() => parseChangelog(`${release}\n### Documentation\n\n* explain tabs\n`)).toThrow("CHANGELOG.md line 7 has an unknown section Documentation");
  // A type section may repeat breaking changes, but any other item in it would be dropped.
  expect(() => parseChangelog(`## 1.0.0 (2026-10-11)\n\n### ⚠ BREAKING CHANGES\n\n* require macOS 15 ${pr(40)}\n\n### Build System\n\n* require macOS 15 ${pr(40)}\n* bump the SDK ${pr(41)}\n`)).toThrow("CHANGELOG.md line 7 has an unknown section Build System");
  expect(() => parseChangelog("## 1.0.0 (2026-02-30)\n")).toThrow("CHANGELOG.md line 1 has an invalid date 2026-02-30");
  expect(() => releaseNotesHtml(release, "1.0.1")).toThrow("CHANGELOG.md must begin with release 1.0.1");
});

test("installed-version marking matches Sparkle's section class", () => {
  const html = markInstalledVersion(releaseNotesHtml(changelog, "1.2.0"), "1.1.0");
  expect(html).toContain('<section class="sparkle-installed-version" data-sparkle-version="1.1.0">');
  expect(() => markInstalledVersion(html, "0.1.0")).toThrow("no section for 0.1.0");
});

test("the repository CHANGELOG.md renders for the workspace version", async () => {
  const manifest = await readFile(resolve(import.meta.dir, "../Cargo.toml"), "utf8");
  const version = /^version = "([^"]+)"$/m.exec(manifest)?.[1];
  if (!version) throw new Error("root Cargo package version is missing");
  const html = await renderReleaseNotesFile(version);
  expect(html).toContain(`<section data-sparkle-version="${version}">`);
  expect(html).toContain(`<section data-sparkle-version="${oldestNotesVersion}">`);
});
