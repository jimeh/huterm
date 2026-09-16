/** Copy the pinned Lucide UI icons byte for byte and verify committed copies. */
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const repository = resolve(import.meta.dir, "..");
const iconDirectory = "crates/huterm-gpui/assets/icons";

export const iconNames = [
  "x",
  "plus",
  "chevron-left",
  "chevron-right",
  "circle-alert",
] as const;

function sourceFile(packageRoot: string, name: string): string {
  return join(packageRoot, "icons", `${name}.svg`);
}

function destinationFile(root: string, name: string): string {
  return join(root, iconDirectory, `${name}.svg`);
}

export function generateUiIcons(
  root = repository,
  packageRoot = join(root, "node_modules/lucide-static"),
): void {
  const destination = join(root, iconDirectory);
  mkdirSync(destination, { recursive: true });
  const expected = new Set(iconNames.map(name => `${name}.svg`));
  // Read every source before touching the committed set, so a missing icon
  // leaves the directory as it was.
  const sources = iconNames.map(name => [name, readFileSync(sourceFile(packageRoot, name))] as const);
  for (const entry of readdirSync(destination)) {
    if (!expected.has(entry)) rmSync(join(destination, entry), { recursive: true, force: true });
  }
  for (const [name, bytes] of sources) writeFileSync(destinationFile(root, name), bytes);
}

export function checkUiIcons(
  root = repository,
  packageRoot = join(root, "node_modules/lucide-static"),
): void {
  const destination = join(root, iconDirectory);
  const actual = existsSync(destination) ? readdirSync(destination).filter(entry => entry !== ".DS_Store").sort() : [];
  const expected = iconNames.map(name => `${name}.svg`).sort();
  const missing = expected.filter(file => !actual.includes(file));
  if (missing.length > 0) throw new Error(`missing UI icons: ${missing.join(", ")}`);
  const extra = actual.filter(file => !expected.includes(file));
  if (extra.length > 0) throw new Error(`unexpected UI icons: ${extra.join(", ")}`);
  for (const name of iconNames) {
    if (!readFileSync(destinationFile(root, name)).equals(readFileSync(sourceFile(packageRoot, name)))) {
      throw new Error(`UI icon differs from lucide-static: ${name}.svg`);
    }
  }
}

if (import.meta.main) {
  try {
    const [mode, ...rest] = Bun.argv.slice(2);
    if (rest.length > 0) throw new Error("expected generate or check");
    if (mode === "generate") {
      generateUiIcons();
      console.log(`Generated ${iconNames.length} Lucide UI icons`);
    } else if (mode === "check") {
      checkUiIcons();
      console.log(`Verified ${iconNames.length} Lucide UI icons`);
    } else {
      throw new Error("expected generate or check");
    }
  } catch (error) {
    console.error(`UI icons failed: ${error instanceof Error ? error.message : error}`);
    process.exitCode = 1;
  }
}
