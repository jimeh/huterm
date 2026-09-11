# Chunk 1: protocol catalog

Part of [the command palette plan](../command-palette.md). Read that document
first; this chunk implements its catalog contract and nothing else. Everything
below is confined to `crates/huterm-protocol`. No other crate changes here,
even where downstream code stops compiling: the later chunks own those edits,
and this crate builds and tests on its own.

Owner: Codex implementer. Dependencies: none. Blocks: every other chunk.

## Objective

Extend `huterm-protocol::command` so the catalog can express everything the
palette plan needs: listing by required context, unprompted arguments,
one-of argument groups, a two-tier validator, a strict quake profile kind,
`select_tab` with `index` or `tab`, and the new palette, text, reset, and
recent-tab commands.

## Invariants

- `huterm-protocol` keeps its empty dependency set. `mise run architecture`
  enforces it. Contexts are plain `&'static str`; no GPUI types.
- The crate denies `unsafe_code` and Clippy `all`; pedantic is warn. Keep the
  existing doc-comment style: every public item documented, `# Errors` on
  fallible functions, `#[must_use]` where the existing code uses it.
- Existing public API stays source-compatible except where this plan says
  otherwise. Existing tests keep passing unless a listed change alters them.

## Changes

### `CommandSpec`

Add a required context:

```rust
pub struct CommandSpec {
    pub id: CommandId,
    pub title: &'static str,
    pub description: &'static str,
    pub scope: CommandScope,
    /// Key context a client must have captured for this command to be
    /// listed interactively, such as `"Palette"`. `None` lists everywhere.
    pub context: Option<&'static str>,
    pub args: &'static [ArgumentSpec],
}
```

Update the private `spec` constructor. Add a second constructor
`spec_in(context, ...)` or an equivalent so ordinary entries stay short.

### `CommandScope`

Add `Palette`: "Executed by the window's open command palette." Keep the
existing variants and their docs.

### `ArgumentSpec`

Replace `required: bool` with a requirement and add a prompt flag:

```rust
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Requirement {
    /// Must be supplied.
    Always,
    /// May be omitted; the executor applies its documented default.
    Optional,
    /// Exactly one argument in the named group must be supplied.
    OneOf(&'static str),
}

pub struct ArgumentSpec {
    pub name: &'static str,
    pub kind: ArgumentKind,
    pub required: Requirement,
    /// Whether an interactive client prompts for this argument. Unprompted
    /// arguments exist for keybindings and programmatic callers only.
    pub prompt: bool,
}
```

Add `impl ArgumentSpec { pub const fn is_required(&self) -> bool }` returning
true for `Always` only, plus `pub const fn group(&self) -> Option<&'static
str>`. Update every catalog entry: existing `required: true` becomes
`Requirement::Always`, `false` becomes `Requirement::Optional`, and all
existing arguments are `prompt: true`.

### `ArgumentKind`

Add `QuakeProfile`: "Name of a configured quake profile, resolved from client
configuration; carried as `CommandValue::Text`." `CommandValue::matches`
accepts `Text` for it. `describe_kind` returns "a quake profile name".
`show_quake`, `hide_quake`, and `toggle_quake` change their `profile`
argument to this kind; it stays `Optional` and prompted.

### Validation in two tiers

Split `validate`:

```rust
/// Checks supplied arguments only: unknown names, duplicates, kinds,
/// integer ranges, and one-of exclusivity. Missing arguments are not an
/// error here, so bindings and interactive requests can carry partial
/// invocations that a client completes later.
pub fn validate_supplied(
    invocation: &CommandInvocation,
) -> Result<&'static CommandSpec, CommandError>;

/// `validate_supplied` plus completeness: every `Always` argument present
/// and exactly one member of every `OneOf` group present.
pub fn validate(
    invocation: &CommandInvocation,
) -> Result<&'static CommandSpec, CommandError>;
```

Add one error variant for exclusivity:

```rust
/// More than one argument of a one-of group was supplied.
ConflictingArguments {
    command: CommandId,
    group: &'static str,
    names: Vec<&'static str>,
},
```

Display: ``command `{command}` accepts only one of `a`, `b` `` (names in
catalog order). A group with no member supplied fails `validate` with
`MissingArgument` naming the group's first prompted member, or its first
member when none is prompted.

Add a helper the clients will use for prompting decisions:

```rust
impl CommandSpec {
    /// Arguments an interactive client must still collect: `Always`
    /// arguments not supplied, and one prompted member of each `OneOf`
    /// group with no supplied member. Returns catalog order.
    #[must_use]
    pub fn missing_prompted(
        &self,
        invocation: &CommandInvocation,
    ) -> Vec<&'static ArgumentSpec>;
}
```

### `select_tab`

Arguments become:

```rust
&[
    ArgumentSpec { name: "index", kind: ArgumentKind::Integer { min: 1, max: 9 },
                   required: Requirement::OneOf("target"), prompt: false },
    ArgumentSpec { name: "tab", kind: ArgumentKind::Tab,
                   required: Requirement::OneOf("target"), prompt: true },
]
```

Description: "Activate a tab by position from a keybinding, or choose one."
The existing `CommandInvocation::integer("index")` and `tab("tab")` readers
already cover both values.

### New commands

All with `Requirement`/`prompt` as noted, added to `ids` with doc comments
and to `CATALOG` in this order after the existing entries.

Runtime scope, no context:

| id | title | description | args |
| --- | --- | --- | --- |
| `reset_tab_name` | Reset Tab Name | Clear the custom name of a tab. | `tab: Tab, Optional, prompt` |
| `reset_workspace_name` | Reset Workspace Name | Clear the custom name of a workspace. | `workspace: Workspace, Optional, prompt` |
| `reset_session_name` | Reset Session Name | Clear the custom name of a session. | `session: Session, Optional, prompt` |

Window scope, no context:

| id | title | description |
| --- | --- | --- |
| `select_recent_tab` | Switch to Last Tab | Activate the most recently used tab; repeat to toggle between two tabs. |

Palette scope, context `"Palette"`, no arguments:

| id | title | description |
| --- | --- | --- |
| `palette_select_next` | Palette: Next Item | Move the palette selection down. |
| `palette_select_previous` | Palette: Previous Item | Move the palette selection up. |
| `palette_page_down` | Palette: Page Down | Move the palette selection down one page. |
| `palette_page_up` | Palette: Page Up | Move the palette selection up one page. |
| `palette_confirm` | Palette: Confirm | Run the selected command or commit the active argument. |
| `palette_back` | Palette: Back | Return to command search, or close the palette. |
| `palette_pop` | Palette: Pop Argument | Reopen the previous argument, or return to search. |
| `palette_expand` | Palette: Edit Arguments | Open the selected command's arguments without running it. |
| `palette_next_slot` | Palette: Next Argument | Move to the next argument. |
| `palette_previous_slot` | Palette: Previous Argument | Move to the previous argument. |
| `text_delete_backward` | Text: Delete Backward | Delete the character before the cursor. |
| `text_delete_forward` | Text: Delete Forward | Delete the character after the cursor. |
| `text_delete_word_backward` | Text: Delete Word Backward | Delete the word before the cursor. |
| `text_delete_line_start` | Text: Delete to Line Start | Delete from the line start to the cursor. |
| `text_move_left` | Text: Move Left | Move the cursor left. |
| `text_move_right` | Text: Move Right | Move the cursor right. |
| `text_move_word_left` | Text: Move Word Left | Move the cursor left one word. |
| `text_move_word_right` | Text: Move Word Right | Move the cursor right one word. |
| `text_line_start` | Text: Line Start | Move the cursor to the line start. |
| `text_line_end` | Text: Line End | Move the cursor to the line end. |
| `text_select_left` | Text: Select Left | Extend the selection left. |
| `text_select_right` | Text: Select Right | Extend the selection right. |
| `text_select_all` | Text: Select All | Select all text. |
| `text_copy` | Text: Copy | Copy the selection. |
| `text_paste` | Text: Paste | Paste from the clipboard. |

The `text_*` commands are also `Palette` scope with context `"Palette"` in
this delivery. Their names describe text, not the palette, so a later text
field elsewhere can reuse them.

### Catalog tests

Extend the existing `catalog_entries_are_unique_and_described` test so every
context is either `None` or `"Palette"` (a fixed allow-list; a typo fails),
every `OneOf` group has at least two members, and every group has at least
one prompted member.

## Tests to add

In `command.rs` tests, named as listed:

- `validate_supplied_accepts_partial_invocations_and_rejects_bad_values`:
  bare `select_tab` passes `validate_supplied`; `select_tab` with
  `index = 10` fails `ArgumentRange`; with `index` as text fails
  `ArgumentType`; with both `index` and `tab` fails `ConflictingArguments`
  under both tiers; with an unknown name fails `UnknownArgument`.
- `validate_requires_exactly_one_group_member`: bare `select_tab` fails
  `validate` with `MissingArgument { name: "tab" }` (the prompted member);
  `index = 3` alone and `tab` alone both pass.
- `missing_prompted_lists_only_what_a_client_must_collect`: bare
  `select_tab` yields `["tab"]`; `select_tab` with `index` yields `[]`;
  bare `rename_tab` yields `["name"]` (not `tab`, which is optional); bare
  `toggle_quake` yields `[]`.
- `quake_profile_arguments_carry_text`: `toggle_quake` with
  `profile = Text("logs")` validates; with `Integer(1)` fails
  `ArgumentType { expected: QuakeProfile }`.
- `palette_commands_are_palette_scoped_and_context_bound`: every id starting
  with `palette_` or `text_` has `CommandScope::Palette`, context
  `Some("Palette")`, and no arguments.
- `reset_and_recent_commands_validate`: each reset command validates with
  and without its identity argument; `select_recent_tab` validates with no
  arguments and rejects any argument.
- Update `required_arguments_are_enforced_and_optional_ones_may_be_omitted`
  and `argument_types_and_integer_ranges_are_checked` for the new
  `select_tab` shape; the integer range cases still hold when `index` is
  supplied alone.

## Verification

From the repository root:

```sh
cargo test --locked -p huterm-protocol
cargo clippy --locked -p huterm-protocol --all-targets -- -D warnings
cargo fmt --check -p huterm-protocol
mise run architecture
```

Other crates will fail to compile after this change. That is expected and is
not to be fixed in this chunk. Do not touch `huterm-core`, `huterm-config`,
`huterm-gpui`, schemas, README, or `Cargo.lock`.

## Self-review checklist

Before reporting, re-read the diff against this document and confirm:

- every new `ids` constant has a doc comment and a `CATALOG` entry, and the
  catalog test passes with the allow-list;
- `validate` is implemented as `validate_supplied` plus completeness, not a
  second copy of the argument loop;
- `ConflictingArguments` lists names in catalog order;
- no `unwrap`, `expect`, or `panic` outside tests;
- rustfmt and Clippy are clean at the crate level.

Report: files changed, the test names that ran and passed (copy the
`test result:` line), and anything in this document that could not be
followed as written, with the reason.
