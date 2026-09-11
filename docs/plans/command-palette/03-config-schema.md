# Chunk 3: configuration and schema

Part of [the command palette plan](../command-palette.md). Implements the
`[palette]` configuration section and regenerates the schemas for chunk 1's
catalog changes. Requires [chunk 1](01-protocol-catalog.md) merged.

Owner: Codex implementer. Dependencies: chunk 1. Runs in parallel with
chunks 2 and 4; touches none of their files.

## Objective

Add `palette.retain_query` and `palette.retain_query_seconds`, thread them
into the GPUI `Config`, teach schema generation about `QuakeProfile`,
unprompted arguments, `OneOf` groups, and the new commands, and extend the
shared fixtures.

Files: `crates/huterm-config/src/lib.rs`, `crates/huterm-config/src/schema.rs`,
`crates/huterm-gpui/src/config.rs`, `schemas/huterm.schema.json`,
`schemas/fixtures.json`, and the README configuration documentation. Do not
edit `windows.rs`, `palette.rs`, `keymap.rs`, or `commands.rs`.

## Invariants

- `huterm-config` stays independent of GPUI and terminal engines.
- Schemas are generated, never hand-edited: `mise run schema:generate`
  writes both files; `mise run schema:check` compares bytes.
- Taplo skips properties beside a schema's `allOf` during completion, so
  common keybinding fields stay in the first `allOf` member and command
  constraints in later `if`/`then` members (`schema.rs` already does this).
- Runtime identities are omitted from keybinding argument schemas; keep
  that and treat `QuakeProfile` as a string.
- Release packaging verifies schema bytes against the checkout; do not stamp
  versions.

## Changes

### `huterm-config`

Add to `RawConfig`:

```rust
#[serde(default)]
pub palette: PaletteConfig,
```

```rust
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct PaletteConfig {
    /// Keep the search text after the palette is cancelled, so reopening
    /// within `retain_query_seconds` restores it selected.
    pub retain_query: bool,
    /// How long a cancelled palette's search text is retained, 0 to 3600.
    pub retain_query_seconds: u32,
}
```

Defaults: `true` and `15`. `validate_values` rejects
`retain_query_seconds > 3600` with
`ConfigError::Invalid("palette.retain_query_seconds must be between 0 and 3600")`.
Document both fields with doc comments; schemars turns them into
descriptions.

### `huterm-gpui/src/config.rs`

Add `pub(super) palette: huterm_config::PaletteConfig` to `Config`, default
from `PaletteConfig::default()`, and copy `raw.palette` across in the raw
to config conversion. Nothing reads it yet; chunk 6 does.

### `schema.rs`

In `keybindings`:

- `ArgumentKind::QuakeProfile` maps to `{"type":"string"}` with description
  "Configured quake profile name; defaults to `default`."
- Arguments with `prompt: false` are still schema properties: they are
  exactly the ones meant for config.
- `required` is built from `Requirement::Always` only. For each `OneOf`
  group with more than one config-representable member, add to the
  alternative's `args` schema a `not` clause that forbids supplying every
  member together: `{"not": {"required": [members...]}}`. For `select_tab`
  the group's config-representable members are `index` alone (`tab` is a
  runtime identity), so no clause is emitted and a bare binding validates.
  Emit the clause generically so a future group with two config-representable
  members gets it.
- The `required: ["key","command","args"]` promotion happens only when
  `required` is non-empty.
- Palette-scope commands appear in the local keybinding enum like any other
  command. The global keybinding enum stays restricted to the three quake
  commands.

Run `mise run schema:generate` and commit both regenerated files.

### `schemas/fixtures.json`

Add cases (all `theme: false`):

| name | toml | valid |
| --- | --- | --- |
| palette defaults | `[palette]` | true |
| palette retain off | `[palette]\nretain_query = false` | true |
| palette retain 3600 | `[palette]\nretain_query_seconds = 3600` | true |
| palette retain 3601 | `[palette]\nretain_query_seconds = 3601` | false |
| palette unknown key | `[palette]\nvim = true` | false |
| select_tab bare binding | `[[keybinding]]\nkey = "cmd-shift-o"\ncommand = "select_tab"` | true |
| select_tab index binding | `[[keybinding]]\nkey = "cmd-1"\ncommand = "select_tab"\nargs = { index = 1 }` | true |
| select_tab tab in config | `[[keybinding]]\nkey = "cmd-1"\ncommand = "select_tab"\nargs = { tab = 1 }` | false |
| palette command binding | `[[keybinding]]\nkey = "ctrl-n"\ncommand = "palette_select_next"` | true |
| palette command with when | `[[keybinding]]\nkey = "ctrl-n"\ncommand = "palette_select_next"\nwhen = "!confirming"` | true |
| quake profile text | `[[keybinding]]\nkey = "f5"\ncommand = "toggle_quake"\nargs = { profile = "logs" }` | true |
| quake profile integer | `[[keybinding]]\nkey = "f5"\ncommand = "toggle_quake"\nargs = { profile = 1 }` | false |
| reset tab name binding | `[[keybinding]]\nkey = "f6"\ncommand = "reset_tab_name"` | true |
| global palette command | `[[global_keybinding]]\nkey = "ctrl-alt-n"\ncommand = "palette_select_next"` | false |

Check how the existing fixture runner decides validity for keybinding cases
(`scripts/` and the `huterm-config` tests) and match its expectations; if a
case needs the application-side keymap compiler to reject it rather than the
schema, note that in the fixture's existing mechanism rather than inventing a
new field.

### README

In the configuration section, document `[palette]` with both keys, defaults,
and the 0 to 3600 bound, near the existing `[window]` documentation.

## Tests

- `huterm-config`: `palette_defaults_and_bounds` (defaults, 3600 accepted,
  3601 rejected with the exact message, unknown key rejected).
- Schema tests in `huterm-config` (there is an existing fixture-driven test;
  extend it rather than adding a parallel runner) cover the new fixture
  cases.
- `mise run schema:check` passes after regeneration.

## Verification

```sh
cargo test --locked -p huterm-config --features schema
mise run schema:generate && mise run schema:check
mise run check:scripts
cargo clippy --locked -p huterm-config --features schema --all-targets -- -D warnings
cargo clippy --locked -p huterm-gpui --all-targets -- -D warnings
mise run lint:docs:files -- README.md
```

## Self-review checklist

- Both schema files are regenerated output, byte-identical to a fresh
  `schema:generate`.
- The `select_tab` keybinding schema accepts a bare binding and rejects
  `tab`.
- No `PaletteText` reference is introduced anywhere.
- `deny_unknown_fields` is on `PaletteConfig`.

Report: files changed, test names and results, fixture count before and
after, and deviations with reasons.
