# Configuration schemas and release assets

Status: agreed direction, implementation pending. Written on 2026-09-09 for
[issue #33](https://github.com/jimeh/huterm/issues/33).

## Outcome

Provide editor completion and validation for Huterm's supported TOML config
and standalone theme files. Generate schemas from the application's serialized
definitions, commit reproducible output, and publish those exact files as GitHub
release assets. Keybinding argument validation must depend on the selected
command and derive from the existing command catalog.

Keep the config format, accepted legacy forms, startup fallback behavior, and
reload semantics unchanged. Schema validation supplements runtime validation;
it cannot prove that a theme exists on disk or that a GPUI predicate parses.

## Settled design

- Add a small `huterm-config` crate for serialized definitions and portable
  validation. It must not depend on GPUI, core, Alacritty, or Ghostty.
- Keep GPUI resolution, native modifier matching, and keystroke/context parsing
  in the desktop crate. Preserve the dependency-free protocol crate; consume
  its command catalog without adding schema dependencies there.
- Use Schemars on the types the application deserializes, with explicit
  constraints and descriptions for rules that derives cannot infer.
- Commit `schemas/huterm.schema.json` and
  `schemas/huterm-theme.schema.json`. Each file must resolve its references
  internally, without downloading another schema.
- Publish stable asset filenames `huterm.schema.json` and
  `huterm-theme.schema.json`, unchanged from the exact release checkout.
- Document both latest-release and tag-specific URLs. Do not introduce separate
  schema hosting or release-time version stamping.

The extraction makes schema generation independent of desktop/native builds.
Avoid a second set of manually maintained config definitions solely for schema
generation. Keep schema tooling dependencies behind an explicit feature or
development entry point so normal application builds do not need the generator.

## Implementation sequence

### 1. Extract the serialized configuration contract

Start with [config.rs](../../crates/huterm-gpui/src/config.rs) and
[themes.rs](../../crates/huterm-gpui/src/themes.rs). Move raw config structures,
portable enums, and reusable structural validation into `huterm-config`.
Keep filesystem resolution and desktop integration in their existing owners
unless moving a helper is necessary to share a validation rule.

Separate selected-theme input from named theme definitions where their accepted
fields differ. Selected `[theme]` permits `name` and color overrides, but rejects
`extends`. Entries in `[themes.<name>]` and standalone theme files use `extends`
and reject `name`. Standalone files retain their `[theme]` wrapper.

Preserve platform-dependent behavior explicitly. Font family defaults to Menlo
on macOS and monospace on Linux; link modifiers default to Cmd and Ctrl
respectively. Portable parsing may accept explicit platform context, but must
not silently replace these defaults with generator-host defaults. Move the
GPUI-specific portion of modifier matching back to the desktop integration.

Wire the new workspace member into internal version declarations, release
version updates, and architecture checks. Follow the repository's pinned
dependency and release-age policy for Schemars and schema validation tooling.

Run focused config, theme, and keymap tests during extraction. Preserve useful
diagnostics, unknown-field rejection, default config creation, engine fallback,
and reload failure behavior before adding schema generation.

### 2. Generate config and theme schemas

Generate the deserialization contract with an explicitly selected JSON Schema
dialect. Start with Draft 7 for editor compatibility and verify the selected
TOML editor supports the command alternatives. Do not depend on Schemars'
implicit dialect default.

Include the following constraints and documentation:

| Input | Schema behavior |
| --- | --- |
| Config tables | Match optional fields, enum spellings, and unknown-field rejection |
| `font.family` | Reject blank text; describe platform defaults |
| `font.size` | Inclusive range 6 through 96 |
| Window padding | Inclusive range 0 through 256 logical points |
| Colors | Exactly six hexadecimal digits after `#` |
| Legacy `ansi` palette | Exactly 16 valid colors; preserve named color overrides |
| Theme names | Match the existing letters, digits, hyphen, and underscore rules |
| Selected and named themes | Enforce the different `name` and `extends` contracts |
| `terminal.engine` | `alacritty` or `ghostty`, default `alacritty` |
| Link modifiers | Valid nonempty modifier combinations without duplicates |
| Keybindings | Command-specific alternatives described below |

Describe platform-specific modifier restrictions, including the macOS rejection
of Ctrl, as runtime checks in the shared schema. Avoid claiming a single
cross-platform schema can determine the machine on which a config will run.
Likewise, describe both platform defaults without emitting a host-dependent
`default` annotation. Keep invariant defaults tied to the application definitions.

Both engines ship in every build. Document that engine reload changes newly
created terminals only. Unknown engine names remain errors. Issue #33's older
unavailable-build-feature case no longer applies; do not add optional-engine
machinery to satisfy that stale wording.

Use a stable latest-release URL as each schema's `$id`, following Airplan's
approach. Keep all internal references local. Tag-specific download URLs still
select the schema bytes associated with that application release.

### 3. Generate command-specific keybinding arguments

Read [the command catalog](../../crates/huterm-protocol/src/command.rs) to
generate a `oneOf` alternative per command. Each alternative fixes `command`
with `const`, constrains its `args` properties and required arguments, and
rejects unknown arguments. Reuse shared keybinding fields without allowing
unknown fields through schema composition.

Map catalog booleans, text, and bounded integers to schema types and limits.
Exclude runtime-supplied tab, workspace, and session identities from
configurable arguments and required fields. Derive descriptions from the
catalog where available. Do not maintain another command-name or argument list.

For example, this binding validates:

```toml
[[keybinding]]
key = "cmd-1"
command = "select_tab"
args = { index = 1 }
```

For `select_tab`, require integer `index` between 1 and 9. Reject missing
arguments, `index = 0`, `index = 10`, string values, and unknown argument names.
Changing `command` must select the new command's argument contract.

Preserve omission and empty-table behavior for commands without required
arguments. Treat `unbind` as an explicit config-only alternative: reject
nonempty arguments and any `when`, but preserve the currently accepted
`args = {}` form. Reject unknown commands and blank descriptions. Validate
simple key structure without reimplementing GPUI's keystroke or predicate parser
in JSON Schema.

Prove argument completion in a schema-aware TOML editor as well as validation.
An automated validator accepting `oneOf` does not establish useful editor
completion or field-specific diagnostics. If editor behavior requires a
different schema arrangement, preserve these same command constraints.

### 4. Add generation and drift checks

Expose `mise run schema:generate` and `mise run schema:check`. Generation writes
the two committed files; checking generates in memory or temporary storage and
fails on any byte difference without rewriting the working tree.

Use stable ordering and formatting. Exclude timestamps, absolute paths,
generator-host defaults, and release-specific rewriting. Ensure optional TOML
fields do not gain misleading JSON-only null suggestions.

Include drift checks in normal verification and CI. Run generation checks on
Linux and macOS and compare both against the same committed output. The focused
generation task must work without preparing Ghostty sources or compiling GPUI.

### 5. Integrate release assets

Extend [release-macos.ts](../../scripts/release-macos.ts) and
[the release workflow](../../.github/workflows/release.yml). Use one explicit
asset inventory for the app archive, both schemas, and `SHA256SUMS`.

Check schema drift against the selected release checkout before expensive
signing and notarization work. Copy the committed schemas into the final dist
directory after its cleanup, then include the archive and both schemas in
`SHA256SUMS`. Check the schema bytes against the checkout as well as verifying
their checksums.

Preserve exact local filenames, nonempty files, and remote asset name, size,
upload-state, and digest verification. Missing or altered schemas must block
publication just like a missing or altered app archive. Keep the release draft
on failure. Do not add a separate upload after publication.

Include both schemas in the non-publishing verification artifact. Preserve its
existing behavior: no tag or GitHub Release is required or inspected. Keep
source-SHA validation and signing/notarization guards unchanged.

### 6. Document editor setup

Document the schema header in README config examples and newly created default
configs, using the latest-release URL:

```toml
#:schema https://github.com/jimeh/huterm/releases/latest/download/huterm.schema.json
```

Standalone theme examples use `huterm-theme.schema.json`. Document replacing
`latest/download` with `download/vX.Y.Z` to match an older installed version.
Explain that latest follows the newest published release and can describe
settings absent from an older installation. Development builds can associate
the checked-in schema by local path.

Use a comment directive rather than introducing a `$schema` config field.
Document Taplo/Even Better TOML association, validation limits, and the fact
that release URLs become usable only after a release containing those assets
is published. Preserve existing config files when adding the starter header.

## Verification and acceptance

- Validate representative TOML documents after conversion to the schema
  validator's data model. Cover an empty default config, both engines, inline
  and standalone themes, inheritance declarations, legacy palettes, and
  command-specific bindings. Validate bundled theme files too.
- Pair schema checks with application parsing or keymap compilation for the
  behavior under test. Cover unknown fields, numeric boundaries, malformed
  colors, incorrect palette lengths, wrong theme fields, argument types and
  ranges, missing required arguments, runtime IDs, and `unbind` restrictions.
- Confirm invalid arguments produce diagnostics identifying the relevant
  command or argument. Exercise actual editor completion after selecting a
  command, including switching commands with an existing `args` table.
- Preserve runtime-only checks for missing themes, inheritance cycles,
  platform restrictions, and GPUI syntax. Retain startup and reload regression
  coverage, including keeping the last valid config and existing engine
  instances on reload errors.
- Verify generation byte equality across repeated runs and both supported
  platforms. Confirm the drift check fails after an intentional stale-output
  change and passes after restoration.
- Extend focused release tests for schema copies, all checksum entries,
  missing/extra/altered assets, remote mismatches, and non-publishing artifact
  inclusion. Tests must demonstrate failures before publication.
- Run the relevant focused tests during implementation, then
  `mise run verify` and the dependency license audit before handoff. Report
  native or editor checks that cannot run on the available host.
- During an authorized release, retrieve tag-specific and latest schema URLs
  and compare their content with the expected release files. Do not claim
  published-URL acceptance from local tests or an Actions artifact alone.

For new behavioral tests, prefer observing failure at the intended assertion
before the fix and confirm the runner collected them. Use existing tests as
regression evidence without treating their green status as proof of schema
coverage. No release publication is authorized by this plan document.

## Reference implementations

- [Treeboot release workflow](https://github.com/jimeh/treeboot/blob/main/.github/workflows/release.yml)
  publishes a committed schema as `config.schema.json`, with checksums.
- [Airplan release workflow](https://github.com/jimeh/airplan/blob/main/.github/workflows/release.yml)
  includes `airplan.schema.json` in its verified release asset inventory.
- [Airplan schema generation](https://github.com/jimeh/airplan/blob/main/airplan/schema.go)
  documents the stable `$id` and avoidance of release-time version stamping.
- [Schemars generation settings](https://docs.rs/schemars/latest/schemars/generate/struct.SchemaSettings.html)
  support explicit dialect and deserialization-contract selection.
- [Taplo directives](https://taplo.tamasfe.dev/configuration/directives.html)
  define the `#:schema` header association.

## Unresolved questions

None blocking implementation. Confirm the proposed schema composition against
the TOML editor during implementation; completion quality is an acceptance
requirement, not an assumption about `oneOf` support.
