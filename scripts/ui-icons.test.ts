import { afterEach, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkUiIcons, generateUiIcons, iconNames } from "./ui-icons.ts";

const directories: string[] = [];
afterEach(() => {
  for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true });
});

function fixture(): { root: string; packageRoot: string; destination: string } {
  const root = mkdtempSync(join(tmpdir(), "huterm-ui-icons-test-"));
  directories.push(root);
  const packageRoot = join(root, "lucide-static");
  const destination = join(root, "crates/huterm-gpui/assets/icons");
  mkdirSync(join(packageRoot, "icons"), { recursive: true });
  mkdirSync(destination, { recursive: true });
  for (const name of iconNames) writeFileSync(join(packageRoot, "icons", `${name}.svg`), `<svg>${name}</svg>\n`);
  return { root, packageRoot, destination };
}

test("generation copies every selected icon byte for byte and removes stale entries", () => {
  const { root, packageRoot, destination } = fixture();
  writeFileSync(join(destination, "stale.svg"), "stale");
  mkdirSync(join(destination, "stale-directory"));
  generateUiIcons(root, packageRoot);
  expect(readdirSync(destination).sort()).toEqual(iconNames.map(name => `${name}.svg`).sort());
  for (const name of iconNames) {
    expect(readFileSync(join(destination, `${name}.svg`))).toEqual(readFileSync(join(packageRoot, "icons", `${name}.svg`)));
  }
});

test("check accepts the committed Lucide icon bytes", () => {
  checkUiIcons();
});

test("check rejects a missing icon", () => {
  const { root, packageRoot, destination } = fixture();
  generateUiIcons(root, packageRoot);
  rmSync(join(destination, "x.svg"));
  expect(() => checkUiIcons(root, packageRoot)).toThrow("missing UI icons: x.svg");
});

test("check rejects an extra icon", () => {
  const { root, packageRoot, destination } = fixture();
  generateUiIcons(root, packageRoot);
  writeFileSync(join(destination, "extra.svg"), "extra");
  expect(() => checkUiIcons(root, packageRoot)).toThrow("unexpected UI icons: extra.svg");
});

test("check rejects changed bytes without replacing them", () => {
  const { root, packageRoot, destination } = fixture();
  generateUiIcons(root, packageRoot);
  const changed = Buffer.from("changed bytes");
  writeFileSync(join(destination, "plus.svg"), changed);
  expect(() => checkUiIcons(root, packageRoot)).toThrow("UI icon differs from lucide-static: plus.svg");
  expect(readFileSync(join(destination, "plus.svg"))).toEqual(changed);
});
