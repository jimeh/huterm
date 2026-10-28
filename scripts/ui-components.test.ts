/**
 * Checks the `huterm-ui` crate's conventions that Cargo cannot: every
 * component file has a gallery entry, and components draw their focus ring
 * only through `focus::ring`, which shows it for keyboard focus alone.
 */
import { expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const repoRoot = join(import.meta.dir, "..");
const crate = join(repoRoot, "crates/huterm-ui/src");
const componentsDir = "crates/huterm-ui/src/components";
const galleryFile = "crates/huterm-gpui/src/desktop/gallery.rs";

function rustFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return rustFiles(path);
    return entry.name.endsWith(".rs") ? [path] : [];
  });
}

test("every huterm-ui component has a gallery entry, and every entry a component", () => {
  const components = readdirSync(join(repoRoot, componentsDir))
    .filter((name) => name.endsWith(".rs") && name !== "mod.rs")
    .map((name) => name.slice(0, -".rs".length))
    .sort();
  const gallery = readFileSync(join(repoRoot, galleryFile), "utf8");
  const entries = [...gallery.matchAll(/\bentry\(\s*"([a-z_]+)"/g)].map((match) => match[1]).sort();
  expect(components.length).toBeGreaterThan(0);
  expect(entries).toEqual(components);
});

test("huterm-ui draws focus rings only through focus::ring", () => {
  // `Tokens::focus_ring` defines the shadow and `focus::ring` is the one
  // painter that gates it on keyboard focus. GPUI's own focus styles
  // (`.focus(`, `.in_focus(`, `.focus_visible(`) would show a ring after a
  // pointer press or lose it on a pointer move. Matching runs over the whole
  // file, so a style split across lines is caught too.
  const forbidden = [/\.(focus|in_focus|focus_visible)\(\s*(move\s+)?\|/g];
  const ringOutsidePainter = /\bfocus_ring\b/g;
  const offenders: string[] = [];
  for (const file of rustFiles(crate)) {
    const relative = file.slice(repoRoot.length + 1);
    const source = readFileSync(file, "utf8");
    const lineOf = (offset: number) => source.slice(0, offset).split("\n").length;
    const patterns = relative.endsWith("src/focus.rs") || relative.endsWith("src/tokens.rs") ? forbidden : [...forbidden, ringOutsidePainter];
    for (const pattern of patterns) {
      for (const match of source.matchAll(pattern)) offenders.push(`${relative}:${lineOf(match.index ?? 0)}`);
    }
  }
  expect(offenders).toEqual([]);
});

test("text draws with corrected tokens, never raw theme colors", () => {
  // `Tokens` corrects arbitrary themes so enabled text reads on its fill:
  // `accent_text()`, `warning_text()`, `danger_text()`, `on(fill)`, and
  // `muted`. Raw `accent`, `warning`, `danger`, or `bg` as a text color
  // skips that, and `dim` is for disabled text only, chosen in a branch.
  // This catches direct uses; a color picked inside an `if` still needs a
  // reviewer, and the pair table in `desktop/overlay.rs` checks the tokens.
  const raw = "(?:accent|warning|danger|bg|dim)";
  const owner = "(?:swatch|tokens|t)";
  const forbidden = [
    new RegExp(`text_color\\(\\s*${owner}\\.${raw}\\s*\\)`, "g"),
    new RegExp(`small\\([^;]*?,\\s*swatch\\.${raw}\\s*\\)`, "g"),
    new RegExp(`color:\\s*Some\\(\\s*${owner}\\.${raw}\\s*\\)`, "g"),
    new RegExp(`\\.element\\(\\s*px\\([^)]*\\),\\s*${owner}\\.${raw}\\s*\\)`, "g"),
  ];
  const offenders: string[] = [];
  for (const directory of ["crates/huterm-gpui/src", "crates/huterm-ui/src"]) {
    for (const file of rustFiles(join(repoRoot, directory))) {
      const relative = file.slice(repoRoot.length + 1);
      const source = readFileSync(file, "utf8");
      const lineOf = (offset: number) => source.slice(0, offset).split("\n").length;
      for (const pattern of forbidden) {
        for (const match of source.matchAll(pattern)) offenders.push(`${relative}:${lineOf(match.index ?? 0)}`);
      }
    }
  }
  expect(offenders).toEqual([]);
});
