import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const repoRoot = join(import.meta.dir, "..");
const read = (file: string) => readFileSync(join(repoRoot, file), "utf8");
const toml = (file: string) => Bun.TOML.parse(read(file)) as Record<string, any>;
const yaml = (file: string) => Bun.YAML.parse(read(file)) as Record<string, any>;

test("every dependency cooldown uses the same release age in days", () => {
  const mise = toml("mise.toml");
  const dependabot = yaml(".github/dependabot.yml") as { updates: { "package-ecosystem": string; cooldown?: { "default-days"?: number } }[] };
  const days: Record<string, unknown> = {
    "mise.toml settings.minimum_release_age": Number(/^(\d+)d$/.exec(mise.settings.minimum_release_age)?.[1]),
    "mise.toml tools:update --minimum-release-age": Number(/--minimum-release-age (\d+)d\b/.exec(mise.tasks["tools:update"].run)?.[1]),
    "bunfig.toml install.minimumReleaseAge": toml("bunfig.toml").install.minimumReleaseAge / 86_400,
    ".pinact.yaml min_age.value": yaml(".pinact.yaml").min_age.value,
    "zizmor.yml dependabot-cooldown days": yaml("zizmor.yml").rules["dependabot-cooldown"].config.days,
  };
  for (const update of dependabot.updates) {
    days[`.github/dependabot.yml ${update["package-ecosystem"]} cooldown`] = update.cooldown?.["default-days"];
  }
  const expected = days["mise.toml settings.minimum_release_age"];
  expect(Number.isInteger(expected)).toBe(true);
  expect(days).toEqual(Object.fromEntries(Object.keys(days).map((key) => [key, expected])));
});

test("Mise pins the same Rust toolchain as rust-toolchain.toml", () => {
  const toolchain = toml("rust-toolchain.toml").toolchain as { channel: string; components: string[]; profile: string };
  const rust = toml("mise.toml").tools.rust as { version: string; components: string[]; profile: string };
  const locked = (toml("mise.lock").tools.rust as { version: string }[]).map(({ version }) => version);
  expect({ version: rust.version, components: [...rust.components].sort(), profile: rust.profile })
    .toEqual({ version: toolchain.channel, components: [...toolchain.components].sort(), profile: toolchain.profile });
  expect(locked).toEqual([toolchain.channel]);
});
