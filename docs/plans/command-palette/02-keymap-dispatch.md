# Chunk 2: keymap and dispatch

Part of [the command palette plan](../command-palette.md). Implements the
"Palette and text commands" and "Listing by required context" sections at the
keymap and action layer. Requires [chunk 1](01-protocol-catalog.md) merged.

Owner: Codex implementer. Dependencies: chunk 1. Runs in parallel with
chunks 3 and 4; touches none of their files.

## Objective

Make every palette keyboard operation a rebindable catalog command:
a `Palette` scope wrapper action, required contexts compiled into bindings,
default palette bindings in the keymap tables, two-tier validation for
bindings and actions, and removal of the hardcoded component bindings.

Files: `crates/huterm-gpui/src/commands.rs`, `crates/huterm-gpui/src/keymap.rs`,
`crates/huterm-gpui/src/desktop.rs` (only `bind_keymap`),
`crates/huterm-gpui/src/desktop/palette.rs` (only the `bindings()` function
and the `actions!` macro), `crates/huterm-gpui/src/ui/text_field.rs` (only
the `bindings()` function and the `actions!` macro), and the README binding
and context tables.

## Invariants (from AGENTS.md and the keymap module)

- `KeyBinding::new` unwraps its predicate parse; validate predicates with
  `KeyBindingContextPredicate::parse` first. `compile_entries` already does.
- Conditional single-key bindings reserve nothing. `ReservedKeys::claim`
  implements this; do not change it.
- The compiled keymap lists unconditional user bindings first, defaults next,
  conditional user bindings last (`Resolved::dispatch_rank`). Equal-depth
  dispatch ties go to the latest binding; GPUI menus show the earliest.
- Runtime identities (`Tab`, `Workspace`, `Session`) cannot be set in config.
  `resolve_command` rejects them; keep that.
- Reload must reinstall command bindings, component bindings, menus, and the
  keymap snapshot in one operation. `bind_keymap` is that operation.

## Changes

### `commands.rs`

Add the wrapper action and scope mapping:

```rust
/// Palette-scope command handled by the window's open command palette.
#[derive(Action, Clone, Debug, PartialEq)]
#[action(namespace = huterm, no_json)]
pub(crate) struct InvokePalette(pub(crate) CommandInvocation);

pub(crate) enum CommandAction {
    App(InvokeApp),
    Window(InvokeWindow),
    Terminal(InvokeTerminal),
    Palette(InvokePalette),
}
```

`CommandAction::new` calls `validate_supplied`, not `validate`. Update its
doc comment: bindings and menus carry possibly partial invocations that the
window completes or prompts for; executors run `validate` before side
effects. `invoke`/`invoke_with` keep panicking on failure, now of
`validate_supplied`. `CommandAction::invocation`, `into_boxed`,
`menu_item`, and `execution_owner` (or whatever the existing scope-routing
helper is called) gain the `Palette` arm. Palette actions route to the
window, which forwards to its palette; there is no separate global handler.

`select_tab_slot` stays as is; the window (chunk 5) decides between `index`
and `tab`.

### `keymap.rs`

`resolve_command` runs `validate_supplied`. It then ANDs the command's
required context into the binding predicate:

- If `spec.context` is `Some(context)` and the entry has no `when`, the
  predicate becomes `context`.
- If both exist, the predicate becomes `(context) && (when)` built from the
  parsed sources; keep `EffectiveBinding.when` as the user's original text
  and add `EffectiveBinding.context: Option<&'static str>` so display code
  can show both.
- The binding counts as conditional for reservation and dispatch ranking
  whenever a predicate exists, including one derived from the context alone.

Move the predicate construction so it happens once in `compile_entries`
after `resolve_command` returns the spec; the cleanest shape is for
`resolve_command` to return the spec alongside the action and effective
binding.

`bound_command` (test helper) learns `InvokePalette`.

### Default bindings

Append to `defaults(platform)` after the `select_tab` entries. Descriptions
are the catalog titles without the "Palette: " / "Text: " prefix. No `when`
is written; the context comes from the command.

Both platforms:

| key | command |
| --- | --- |
| `down` | `palette_select_next` |
| `up` | `palette_select_previous` |
| `pagedown` | `palette_page_down` |
| `pageup` | `palette_page_up` |
| `enter` | `palette_confirm` |
| `escape` | `palette_back` |
| `tab` | `palette_expand` |
| `shift-tab` | `palette_previous_slot` |
| `ctrl-n` | `palette_select_next` |
| `ctrl-p` | `palette_select_previous` |
| `backspace` | `text_delete_backward` |
| `delete` | `text_delete_forward` |
| `left` | `text_move_left` |
| `right` | `text_move_right` |
| `alt-left` | `text_move_word_left` |
| `alt-right` | `text_move_word_right` |
| `home` | `text_line_start` |
| `end` | `text_line_end` |
| `shift-left` | `text_select_left` |
| `shift-right` | `text_select_right` |
| `alt-backspace` | `text_delete_word_backward` |

`tab` binds `palette_expand`; the palette (chunk 6) treats it as
`palette_next_slot` while in a slot. `palette_pop` has no default key: the
palette invokes it internally when Backspace lands on an empty field.

macOS only:

| key | command |
| --- | --- |
| `cmd-a` | `text_select_all` |
| `cmd-c` | `text_copy` |
| `cmd-v` | `text_paste` |
| `ctrl-a` | `text_line_start` |
| `ctrl-e` | `text_line_end` |
| `cmd-left` | `text_line_start` |
| `cmd-right` | `text_line_end` |
| `cmd-backspace` | `text_delete_line_start` |

Linux only:

| key | command |
| --- | --- |
| `ctrl-a` | `text_select_all` |
| `ctrl-c` | `text_copy` |
| `ctrl-shift-c` | `text_copy` |
| `ctrl-v` | `text_paste` |
| `ctrl-shift-v` | `text_paste` |

`ctrl-tab` currently binds `next_tab`; leave it. `select_recent_tab` gets
no default in this chunk (chunk 5 may revisit after the plan's owner
decides).

### Remove hardcoded component bindings

Delete `bindings()` and the `actions!` declarations from
`desktop/palette.rs` and `ui/text_field.rs`, and their `cx.bind_keys` calls
in `bind_keymap`. The `on_action` handlers that referenced those action
types will no longer compile; replace each with a single
`on_action(cx.listener(Self::invoke_palette))` stub that matches on the
invocation id and calls the existing method (`up`, `down`, `confirm`,
`cancel` in the palette; `backspace`, `left`, and so on in the field).
Keep those methods' bodies unchanged; chunk 6 rewrites them. The point of
this chunk is that dispatch reaches the same code through `InvokePalette`.

Where the field previously received `Copy`/`Paste`/`SelectAll` directly,
the palette now receives `InvokePalette` for `text_*` ids and forwards to
the field entity with `input.update(cx, |field, cx| field.run(id, cx))`.
Add a `pub(crate) fn run(&mut self, id: CommandId, window, cx)` on
`TextField` that matches the `text_*` ids to the existing methods and
returns `Result<(), CommandError>` with `UnknownCommand` for anything else.

### README

Update the key-context table: remove the `PaletteText` row; reword
`Palette` as "The command palette is open and has focus." Add the new
commands to the command table with scope `Palette` and a note that their
context is implied. Add a "Palette" subsection to the default bindings
table listing the keys above. State that user bindings for these commands
need no `when`.

## Tests

In `keymap.rs` tests:

- Replace `palette_text_bindings_outrank_terminal_bindings` with
  `palette_defaults_dispatch_through_invoke_palette`: compile macOS defaults
  only (no extra bindings), build a `Keymap`, and with contexts
  `["Workspace palette", "Palette"]` assert `cmd-c` resolves to
  `InvokePalette` carrying `text_copy`, `down` to `palette_select_next`,
  `enter` to `palette_confirm`. With contexts `["Workspace", "Terminal"]`
  assert `cmd-c` resolves to `InvokeTerminal` carrying `copy` and `down`
  resolves to nothing.
- `user_palette_override_wins_at_equal_depth`: user entry `up` →
  `text_line_start` with no `when`; in the palette context `up` resolves to
  `text_line_start`, not `palette_select_previous`.
- `palette_bindings_never_reserve_terminal_strokes`: user entry `ctrl-c` →
  `text_copy` with no `when`; `reserved.is_reserved(ctrl-c)` is false, and
  `effective` shows `context == Some("Palette")` and `when == None`.
- `context_and_when_combine`: user entry `ctrl-n` → `palette_select_next`
  with `when = "!confirming"`; the effective predicate matches
  `["Workspace palette", "Palette"]` and not
  `["Workspace palette confirming", "Palette"]`.
- `partial_bindings_compile_but_bad_values_fail`: user entry `cmd-shift-o`
  → `select_tab` with no args compiles; with `index = 12` fails with the
  range message; with `tab = 1` fails with the runtime-identity message.
- Keep `conditional_bindings_reserve_only_multi_key_starting_strokes` and
  `unconditional_user_bindings_lead_menus_and_conditional_ones_win_ties`
  passing.

In `commands.rs` tests: `command_action_accepts_partial_invocations`
(`select_tab` with no args builds `Window`; `palette_confirm` builds
`Palette`; `select_tab` with both args fails `ConflictingArguments`).

## Verification

```sh
cargo test --locked -p huterm-gpui keymap
cargo test --locked -p huterm-gpui commands
cargo clippy --locked -p huterm-gpui --all-targets -- -D warnings
cargo fmt --check
mise run lint:docs:files -- README.md
```

The whole `huterm-gpui` crate must compile. If chunk 1's catalog change
broke other call sites that are not listed here, fix only the minimal
compile errors and list each in the report; do not implement chunk 5
behaviour.

## Self-review checklist

- `validate` is no longer called from `CommandAction::new`,
  `invoke_with`, or `resolve_command`; `validate_supplied` is.
- A binding to a `Palette`-context command is conditional even without
  `when`, and `ReservedKeys` reflects that.
- `bind_keymap` no longer calls `palette::bindings()` or
  `text_field::bindings()`, and both functions are gone.
- The macOS and Linux default tables both compile (`compile_defaults` for
  each platform is exercised by the existing defaults test).
- README tables match the catalog exactly; no command appears in one and
  not the other.

Report: files changed, test names that ran (copy the `test result:` lines),
compile errors fixed outside the listed files, and deviations with reasons.
