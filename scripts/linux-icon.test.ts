import { afterEach, expect, test } from "bun:test";
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { checkLinuxIcon, generateLinuxIcon, pngDimensions } from "./linux-icon.ts";

const repository = resolve(import.meta.dir, "..");
const directories: string[] = [];
afterEach(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });

function fixture(): string {
  const root = mkdtempSync(join(tmpdir(), "huterm-linux-icon-test-"));
  directories.push(root);
  cpSync(join(repository, "assets"), join(root, "assets"), { recursive: true });
  cpSync(join(repository, "scripts/linux-icon.ts"), join(root, "scripts/linux-icon.ts"), { recursive: true });
  return root;
}

test("committed Linux icon is a verified 512-pixel derivative", () => {
  const root = fixture();
  expect(() => checkLinuxIcon(root)).not.toThrow();
  expect(pngDimensions(readFileSync(join(root, "assets/Huterm-512.png")))).toEqual({ width: 512, height: 512 });
});

test("generation is deterministic and check rejects changed inputs", () => {
  const root = fixture();
  const before = readFileSync(join(root, "assets/Huterm-512.png"));
  generateLinuxIcon(root);
  expect(readFileSync(join(root, "assets/Huterm-512.png"))).toEqual(before);
  writeFileSync(join(root, "assets/Huterm.png"), "changed");
  expect(() => checkLinuxIcon(root)).toThrow("Linux icon input changed");
});
