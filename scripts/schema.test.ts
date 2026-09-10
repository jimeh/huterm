import { expect, test } from "bun:test";
import Ajv from "ajv";
import { readdir } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dir, "..");
const ajv = new Ajv({ allErrors: true, strict: false, validateFormats: false });
const config = ajv.compile(await Bun.file(resolve(root, "schemas/huterm.schema.json")).json());
const theme = ajv.compile(await Bun.file(resolve(root, "schemas/huterm-theme.schema.json")).json());
const fixtures = await Bun.file(resolve(root, "schemas/fixtures.json")).json() as { name: string; toml: string; valid: boolean; theme?: boolean }[];
for (const fixture of fixtures) {
  test(`schema: ${fixture.name}`, () => {
    const validator = fixture.theme ? theme : config;
    expect(validator(Bun.TOML.parse(fixture.toml)), JSON.stringify(validator.errors)).toBe(fixture.valid);
  });
}
test("schemas validate every bundled theme", async () => {
  const directory = resolve(root, "crates/huterm-gpui/themes");
  let count = 0;
  for (const name of await readdir(directory)) {
    if (!name.endsWith(".toml")) continue;
    expect(theme(Bun.TOML.parse(await Bun.file(resolve(directory, name)).text())), name).toBe(true);
    count++;
  }
  expect(count).toBeGreaterThan(0);
});
test("invalid select_tab arguments identify index and its boundary", () => {
  expect(config({ keybinding: [{ key: "cmd-1", command: "select_tab", args: { index: 10 } }] })).toBe(false);
  expect(config.errors?.some(error => error.instancePath === "/keybinding/0/args/index" && error.keyword === "maximum")).toBe(true);
});
test("schemas contain only local references and no JSON null suggestions", async () => {
  for (const name of ["huterm.schema.json", "huterm-theme.schema.json"]) {
    const document = await Bun.file(resolve(root, "schemas", name)).text();
    expect(document).not.toMatch(/"\$ref":\s*"[^#]/);
    expect(document).not.toMatch(/"null"/);
  }
});
test("invalid quake settings and global profile arguments identify their fields", () => {
  expect(config({ quake: { profiles: { default: { width: 0 } } } })).toBe(false);
  expect(config.errors?.some(error => error.instancePath === "/quake/profiles/default/width" && error.keyword === "exclusiveMinimum")).toBe(true);
  expect(config({ global_keybinding: [{ key: "ctrl-shift-f12", command: "toggle_quake", args: { profile: 2 } }] })).toBe(false);
  expect(config.errors?.some(error => error.instancePath === "/global_keybinding/0/args/profile" && error.keyword === "type")).toBe(true);
});
