# Command catalog and configurable keybindings plan

Status: implemented; the deviations that landed are recorded in place below.
Remaining native verification is the manual QA in the implementation sequence.
Terminology is defined in [CONTEXT.md](../../CONTEXT.md).

Issues: [#23: Create the command catalog and migrate existing actions][issue-23]
and [#25: Configure command keybindings, arguments, and when conditions][issue-25].
Both belong to [#10][issue-10] in the [roadmap][issue-8].

## Outcome and agreed behavior

Every existing user action becomes a catalog command with a stable ID, an
execution scope, and a typed ordered argument list. Users bind commands, with
arguments and `when` conditions, in `config.toml`. Matched shortcuts never
reach the shell; unbound keys do. Reload replaces bindings atomically and keeps
the last working configuration on any error.

The catalog is the single definition of each command. Menus, shortcuts,
programmatic callers, and the future palette ([#24][issue-24]) and CLI
([#46][issue-46]) read it rather than restating command names or arguments.
Fullscreen ([#40][issue-40]) and quake ([#41][issue-41]) register commands into
this catalog when they land.

Out of scope: the palette UI and pickers, the fullscreen state machine,
OS-global hotkey registration, command sequences, JSON Schema publication
([#33][issue-33]), and any effective-bindings listing beyond reload diagnostics.
Option/Alt Meta encoding for terminal applications is [#55][issue-55].

## Existing implementation

- `crates/huterm-gpui/src/desktop.rs` declares about thirty unit-struct GPUI
  actions through `actions!`, installs hard-coded per-platform `KeyBinding`s
  in `install_bindings`, builds the macOS menus in `install_menus`, and keeps a
  static `reserved_chord_for_platform` table that stops application shortcuts
  from reaching the shell. `TerminalView` handles clipboard, scroll, settings,
  about, fullscreen, minimize, and zoom actions under key context `Huterm`.
- `crates/huterm-gpui/src/desktop/windows.rs` handles tab and window actions
  on `WorkspaceView`, including nine `Tab1..Tab9` actions that call
  `select_index`, and registers global handlers for new window, reload, quit,
  and hide. `tab_bindings` supplies the platform-specific tab shortcuts.
- `crates/huterm-gpui/src/config.rs` parses `config.toml` with
  `deny_unknown_fields`, and `reload` returns a whole `Config` or an error.
  `windows::reload` applies a successful result to every window and otherwise
  shows the error without changing state.
- `crates/huterm-core/src/mux.rs` already exposes validated structural
  operations, including `rename_session`, `rename_workspace`, and `rename_tab`.
- `huterm-protocol` has no dependencies and `mise run architecture` enforces
  that, so catalog definitions there cannot use serde.
- GPUI 0.2.2 provides `Keystroke::parse`, a bounded `when` expression language
  through `KeyBindingContextPredicate::parse`, `App::bind_keys` and
  `App::clear_key_bindings`, and action derivation with `#[action(no_json)]`
  for actions that carry data without schemars. `KeyBinding::new` unwraps its
  predicate parse, so callers must validate predicates first.

## Design

### Catalog definitions in `huterm-protocol`

Add `command.rs` with plain Rust types:

- `CommandId`, a newtype over `&'static str` such as `select_tab`.
- `CommandScope { Runtime, Application, Window, Terminal }`.
- `ArgumentKind { Bool, Integer { min, max }, Text, Tab, Workspace, Session }`
  and `ArgumentSpec { name, kind, required }`.
- `CommandSpec { id, title, description, scope, args }`, with
  `catalog() -> &'static [CommandSpec]` and `lookup(id)`.
- `CommandValue` mirroring the argument kinds, and
  `CommandInvocation { id, args: Vec<CommandArgument> }`, where
  `CommandArgument { name, value }` pairs a declared argument name with its
  value.
- `CommandError` covering unknown command, unknown or duplicate argument name,
  missing required argument, argument type and range, `Unavailable(reason)`,
  `StaleTarget`, `ClientRequired`, `Busy`, and `Runtime(String)`.
  `validate(&CommandInvocation)` checks the static contract; scope executors
  report the rest.

Arguments are named. Every argument has a declared name, and callers supply
values by name, so any optional argument can be omitted regardless of its
position. The catalog still declares arguments in a fixed order, which is the
ordered list the issue asks for: the palette and a future CLI can map
positional input onto names using that order without the catalog or the config
format accepting positional values. Validation resolves names against the spec,
rejects unknown and duplicate names, and reports missing required arguments.
Executors read arguments through typed accessors by name. Target arguments that
are omitted are filled from invocation-time context by the client, such as the
active tab.

`ArgumentKind` carries everything a generated schema needs: integer bounds,
text, boolean, and the ID kinds. The ID kinds cannot be written in config, so
the keymap compiler rejects them and schema publication ([#33][issue-33]) omits
them from binding entries. That schema can then type-check `args` per command
by emitting one object schema per command, with typed properties, required
names, and no additional properties, and selecting it by the `command` value.

Initial commands:

| Scope | Commands |
| --- | --- |
| Application | `new_window`, `quit`, `hide`, `hide_others`, `show_all`, `reload_config` |
| Window | `open_settings`, `about`, `new_tab`, `close_tab`, `close_window`, `next_tab`, `previous_tab`, `select_tab { index }`, `toggle_fullscreen`, `minimize`, `zoom` |
| Terminal | `copy`, `paste`, `scroll_page_up`, `scroll_page_down`, `scroll_to_bottom` |
| Runtime | `rename_tab { name, tab? }`, `rename_workspace { name, workspace? }`, `rename_session { name, session? }` |

`select_tab` takes an `index` integer from 1 to 9 and keeps the current meaning
of 9 as the last tab. `toggle_fullscreen`, `minimize`, and `zoom` move from the
terminal view to window scope. `open_settings` and `about` are window scope
rather than application scope: both need a window for the prompt and status
reporting, and global action handlers receive only `App`. The renames map onto
existing Mux operations and give the tests a real ID-targeted command; the
rename UI remains deferred.

### Runtime execution in `huterm-core`

Add `commands.rs` with
`execute(&mut Mux, &CommandInvocation) -> Result<CommandOutcome, CommandError>`.
It validates the invocation, requires a target ID for runtime commands, and
maps Mux errors to `StaleTarget` or `Runtime`. Runtime commands run on the
existing background structural owner, never on the GPUI thread.
`CommandOutcome` (`Completed` or `Accepted`) lives in `huterm-protocol` beside
`CommandError`, since both core and the GPUI client return it.

### GPUI dispatch

Add `crates/huterm-gpui/src/commands.rs`:

- Three GPUI actions, `InvokeApp`, `InvokeWindow`, and `InvokeTerminal`, each
  wrapping a `CommandInvocation` and derived with `#[action(no_json)]`. Scope
  selects the action type, so GPUI's type-routed dispatch delivers each command
  to the view that owns it without handlers forwarding to each other.
- `TerminalView`, `WorkspaceView`, and the application-level handlers each
  register one `on_action` per wrapper and match on the command ID. This
  replaces the unit-struct actions and the nine `Tab1..Tab9` handlers.
- `Desktop::invoke(invocation, window)` for programmatic callers. Window and
  terminal commands without a resolvable window return `ClientRequired`.
  Commands that continue asynchronously, such as spawning or assessed close,
  return `Accepted`; later failures use the existing window status line.
- Menus are built from catalog titles and rebuilt on reload so displayed
  shortcuts follow user bindings.

Existing availability checks stay where they are: busy windows, pending
confirmation, reorder drags, exited terminals, and quit state. Each handler
maps a refused command to `Unavailable` rather than silently ignoring it.

### Configuration and keymap compilation

`config.rs` parses a `[[keybinding]]` array:

```toml
[[keybinding]]
key = "cmd-1"
command = "select_tab"
args = { index = 1 }
when = "Terminal && !confirming"
description = "Select the first tab"

[[keybinding]]
key = "cmd-w"
command = "unbind"
```

`unbind` is a reserved command ID that never appears in the catalog. It rejects
`args` and `when`, and unbinding a key with no earlier binding is a silent
no-op so one config can serve both platforms. Fields are `key`, `command`,
optional `args`, optional `when`, and optional `description`; unknown fields
are rejected like the rest of the config.

`args` is a table keyed by argument name. An array is rejected with a
diagnostic naming the expected argument names, so a positional habit from other
tools fails loudly rather than binding the wrong value.

`description` is a human-readable label for one binding, distinct from the
command's catalog title and description. It exists so the settings UI
([#35][issue-35]) and palette can show what a shortcut does, including its
arguments and context, without deriving prose from the command name. An
explicit description is used verbatim. When absent, the effective binding's
description is the command title followed by any arguments, such as
`Select Tab (index: 3)`. A blank description is rejected, matching the rule for
blank custom names. Every shipped default carries an explicit description.

Add `keymap.rs`, which compiles the platform defaults plus user entries into
GPUI `KeyBinding`s after parsing:

- Keystrokes are validated with `Keystroke::parse` and predicates with
  `KeyBindingContextPredicate::parse` before any `KeyBinding` is built.
- Arguments convert from TOML values to named `CommandArgument`s and pass
  catalog validation, including unknown names, missing required names, and
  types. ID-kind arguments (`tab`, `workspace`, `session`) are rejected in
  config with a message saying the window supplies them. Every diagnostic
  carries the 1-based entry index and key.
- Precedence: defaults first, then user entries in file order. A later entry
  wins over an earlier one for the same key and predicate. `unbind` removes
  every earlier binding for that key, defaults included, regardless of
  predicate. Duplicate user entries for the same key and predicate produce a
  non-fatal conflict diagnostic; the last entry wins.
- Parse, validation, or unknown-command errors fail the whole reload and keep
  the last working configuration and bindings.
- Defaults are a per-platform Rust table using the same entry type, so there
  is one binding format and a test proves the defaults compile and that every
  default has a non-empty description.
- The compiled keymap retains each effective binding's key, command,
  arguments, predicate source, resolved description, and whether it came from
  defaults or the user file, so later UI can list bindings without re-parsing
  the config.
- The compiled keymap exposes a reserved-key set containing every keystroke
  used by any effective binding, including every keystroke of a multi-key
  chord. That set replaces the static reservation table and is consulted by
  both `handle_keystroke` and the keystroke observer. Reservation compares the
  keystroke `key` and modifiers, never `key_char`, so `alt-r` is bindable on
  macOS even though GPUI reports its typed character as `®`.

The `when` language is GPUI's predicate syntax verbatim, documented in the
README. It is bounded, already parsed, and evaluates no user code. Wrapping it
later for a non-GPUI client remains possible because entries store the source
string.

Key contexts: `TerminalView` publishes `Terminal` plus `selection` and
`exited`; `WorkspaceView` publishes `Workspace` plus `confirming`, `reordering`,
and `fullscreen`. `Palette` is documented as reserved for [#24][issue-24].
OS-global hotkeys are not part of this matching.

### Reload

`windows::reload` already chains parsing and metric resolution; keymap
compilation joins that chain. On success it clears and rebinds the GPUI keymap,
replaces the reserved-key set, and rebuilds the menus, then shows any non-fatal
conflict diagnostics in the status line. On failure nothing changes and the
status line shows the first fatal diagnostic.

## Implementation sequence and evidence

1. Add the protocol catalog, values, and validation. Test unknown IDs, unknown
   and duplicate argument names, missing required arguments, omitted optional
   arguments in any position, wrong types, integer range, and that every
   default binding's invocation validates.
2. Add the core runtime executor for renames. Test a rename through `execute`,
   a stale tab ID after close, a foreign `RuntimeId`, and no partial change on
   failure.
3. Add the GPUI wrapper actions and migrate every handler, keeping the
   hard-coded default bindings temporarily so behavior is unchanged at this
   commit. Test `Desktop::invoke` without a window for a window command,
   `select_tab` with an out-of-range index rejected before dispatch, and a
   queued command whose tab closed before execution reporting `StaleTarget`.
   Run `mise run check` and the desktop tests.
4. Add config entries, keymap compilation, the derived reserved-key set,
   reload integration, and menu rebuild; delete the static reservation table.
   Test that defaults compile on both platforms with descriptions, that a
   user entry without a description resolves to the command title and
   arguments while a blank one is rejected, override and unbind
   precedence, conflict diagnostics, invalid key, invalid `when`, and invalid
   args each failing with the entry index, and the reserved set gaining a new
   chord and losing an unbound one. Extend the existing real-matcher test
   pattern to run the compiled keymap through GPUI's `Keymap` with macOS and
   Linux modifiers, including `alt-r` and a terminal-scoped binding that does
   not fire under a `confirming` context.
5. Update the README configuration section, `CONTEXT.md` (Command, Scope,
   Binding), and `AGENTS.md` with the reservation and predicate-validation
   rules. Run `mise run test`, then `mise run verify` before handoff.
6. Manual QA on macOS and Linux: unbind `shift-pageup` and confirm the shell
   receives it; bind a new chord to `new_tab` and confirm it does not leak;
   bind `alt-r` to a command and confirm `®` is not typed; confirm menus show
   updated shortcuts after reload; confirm a broken config keeps the previous
   bindings and shows the error.

Steps 1 and 2 may share a commit but not a separate PR; they have no consumer
until step 3. Prefer a new test failing at its intended assertion before the
implementation exists, and confirm from the runner output that it executed.
Native evidence is limited to the manual QA above because this work changes no
window management.

## Alternatives considered

One GPUI action type for every scope would force each view handler to
propagate unhandled commands upward and would make scope a runtime check rather
than a dispatch property. Per-command GPUI action types would require a second
definition of every command plus serde and schemars on the GPUI crate. A custom
`when` language would duplicate a bounded parser GPUI already ships.
Positional `args` arrays would be shorter for single-argument bindings and are
cheap to accept alongside tables, but they add a second documented form, cannot
omit an optional argument ahead of a supplied one, and are weaker to
schema-check. Positional mapping belongs with callers that are naturally
positional, such as the CLI, using the catalog's declaration order.

## Decisions

- Command targets are IDs only, defaulting to the active target when omitted.
  `Desktop::invoke` does not resolve display names; [#20][issue-20] requires
  that duplicate display names cannot make an ID-targeted command ambiguous,
  so name-to-ID resolution belongs in the palette's pickers ([#24][issue-24]).
  The rename commands have no interactive caller until then.
- Non-fatal conflict diagnostics appear in the window status line after a
  successful reload, alongside any log output. Fatal diagnostics keep using
  the existing config-error status path.

No decisions remain open.

[issue-8]: https://github.com/jimeh/huterm/issues/8
[issue-10]: https://github.com/jimeh/huterm/issues/10
[issue-20]: https://github.com/jimeh/huterm/issues/20
[issue-23]: https://github.com/jimeh/huterm/issues/23
[issue-24]: https://github.com/jimeh/huterm/issues/24
[issue-25]: https://github.com/jimeh/huterm/issues/25
[issue-33]: https://github.com/jimeh/huterm/issues/33
[issue-35]: https://github.com/jimeh/huterm/issues/35
[issue-40]: https://github.com/jimeh/huterm/issues/40
[issue-41]: https://github.com/jimeh/huterm/issues/41
[issue-46]: https://github.com/jimeh/huterm/issues/46
[issue-55]: https://github.com/jimeh/huterm/issues/55
