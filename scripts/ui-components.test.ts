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
  // painter that gates it on keyboard focus; GPUI's own `.focus(` style
  // would show the ring after a pointer press too.
  const offenders: string[] = [];
  for (const file of rustFiles(crate)) {
    const relative = file.slice(repoRoot.length + 1);
    const lines = readFileSync(file, "utf8").split("\n");
    lines.forEach((line, index) => {
      const location = `${relative}:${index + 1}`;
      if (/\.focus_ring\(\)/.test(line) && !relative.endsWith("src/focus.rs")) offenders.push(location);
      if (/\.focus\(\s*(move\s+)?\|/.test(line)) offenders.push(location);
    });
  }
  expect(offenders).toEqual([]);
});
