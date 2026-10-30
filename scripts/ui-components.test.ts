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

test("chrome text sizes follow the interface scale", () => {
  // `[font] ui_size` scales chrome through the window's rem size, so a text
  // size or line height in fixed points would ignore it. The terminal grid
  // sizes its text from `[font] size`, not a literal, and the grid size
  // panel is the one overlay that keeps its points.
  const sources = [
    ...rustFiles(crate),
    join(repoRoot, "crates/huterm-gpui/src/desktop.rs"),
    ...rustFiles(join(repoRoot, "crates/huterm-gpui/src/desktop")),
  ];
  const fixed = /\.(text_size|line_height)\(\s*px\(/g;
  const offenders: string[] = [];
  for (const file of sources) {
    const relative = file.slice(repoRoot.length + 1);
    if (/(\/tests?\.rs|_tests\.rs|\/window_tests\/)/.test(relative)) continue;
    const source = readFileSync(file, "utf8");
    const panel = relative.endsWith("huterm-gpui/src/desktop.rs") ? source.indexOf("fn render_size_panel(") : -1;
    const panelEnd = panel < 0 ? -1 : source.indexOf("\n    }\n", panel);
    const lineOf = (offset: number) => source.slice(0, offset).split("\n").length;
    for (const match of source.matchAll(fixed)) {
      const at = match.index ?? 0;
      if (at > panel && at < panelEnd) continue;
      offenders.push(`${relative}:${lineOf(at)}`);
    }
  }
  expect(offenders).toEqual([]);
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

test("Settings draws focus rings only through huterm-ui", () => {
  // Migration 3a moved Settings' buttons, links, switches, toggle groups,
  // and chips onto `huterm-ui`, whose rings follow `focus::ring`, 6a the
  // theme cards, the sidebar, the keybinding command rows, and the command
  // search results, and 8a the key recorders and their Add buttons. A GPUI
  // focus style fails here.
  const remaining: Record<string, number> = {};
  const style = /\.(focus|in_focus|focus_visible)\(\s*(move\s+)?\|/g;
  const window = join(repoRoot, "crates/huterm-gpui/src/desktop/windows");
  const files = [join(window, "settings_window.rs"), ...rustFiles(join(window, "settings_window"))];
  const counts: Record<string, number> = {};
  for (const file of files) {
    const count = [...readFileSync(file, "utf8").matchAll(style)].length;
    if (count > 0) counts[file.slice(repoRoot.length + 1)] = count;
  }
  expect(counts).toEqual(remaining);
});

test("terminal-window files whose buttons moved to Button draw only the rings they keep", () => {
  // Migration 3b moved these files' buttons onto `Button`, which draws its
  // ring through `focus::ring` or `focus::current_ring`. Two rings stay on
  // purpose: the Licenses search field's, until its `Input` migration, and
  // a focused toast's, part of the app-specific notice stack. Lower a count
  // when its ring moves; a new ring fails here.
  const remaining: Record<string, number> = {
    "crates/huterm-gpui/src/desktop/licenses.rs": 1,
    "crates/huterm-gpui/src/desktop/notices.rs": 1,
  };
  const files = [
    "crates/huterm-gpui/src/desktop/about.rs",
    "crates/huterm-gpui/src/desktop/close_dialog.rs",
    "crates/huterm-gpui/src/desktop/licenses.rs",
    "crates/huterm-gpui/src/desktop/notices.rs",
    "crates/huterm-gpui/src/desktop/windows.rs",
    "crates/huterm-gpui/src/desktop/windows/tab_bar.rs",
  ];
  const patterns = [/\.(focus|in_focus|focus_visible)\(\s*(move\s+)?\|/g, /\bfocus_ring\b/g];
  const counts: Record<string, number> = {};
  for (const relative of files) {
    const source = readFileSync(join(repoRoot, relative), "utf8");
    const count = patterns.reduce((total, pattern) => total + [...source.matchAll(pattern)].length, 0);
    if (count > 0) counts[relative] = count;
  }
  expect(counts).toEqual(remaining);
});

test("Settings fields draw a frame or sit in an input group", () => {
  // Migration 4a moved Settings' fields onto framed and grouped chrome,
  // whose rings follow `focus::ring`. A bare field leaves its frame, and
  // its ring, to hand-drawn styles.
  const window = join(repoRoot, "crates/huterm-gpui/src/desktop/windows");
  const files = [join(window, "settings_window.rs"), ...rustFiles(join(window, "settings_window"))];
  const offenders = files
    .filter((file) => readFileSync(file, "utf8").includes("Chrome::Bare"))
    .map((file) => file.slice(repoRoot.length + 1));
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
    new RegExp(`text_color\\(\\s*${owner}\\.${raw}\\s*,?\\s*\\)`, "g"),
    new RegExp(`small\\([^;]*?,\\s*swatch\\.${raw}\\s*,?\\s*\\)`, "g"),
    new RegExp(`color:\\s*Some\\(\\s*${owner}\\.${raw}\\s*,?\\s*\\)`, "g"),
    new RegExp(`\\.element\\(\\s*px\\([^)]*\\),\\s*${owner}\\.${raw}\\s*,?\\s*\\)`, "g"),
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
