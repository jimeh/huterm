# Command palette and argument pickers

Status: revised. This supersedes the first iteration on PR 91, which built the
runtime plumbing but shipped an unusable interaction model. The plumbing stays;
the interaction model, argument entry, search, and presentation described here
replace it. PR 91 does not merge until this contract is met.

Deliver [#24: searchable command palette with argument pickers][issue-24] as
one GPUI client PR on the command catalog and scoped execution paths from
[#23][issue-23]. The palette searches commands, explains availability, collects
typed arguments inline, captures its invocation target, and calls the same
validated operation as menus, keybindings, and programmatic callers.

Move commands remain with [#32][issue-32]. They will register with the catalog
later and get the argument model described here for free.

## Outcome and user contract

Users open the palette with `cmd-shift-p` on macOS or `ctrl-shift-p` on Linux,
or through View > Open Command Palette. Opening captures the current window,
session, workspace, tab, terminal, and key-binding contexts before focus moves
into the palette.

### One rule

Enter always does the most likely thing. Tab opens the next thing the user
might want to change. Every stage follows this:

- On a command row, Enter runs the command when it has no arguments, when every
  argument is optional, or when each required argument resolves to exactly one
  value. Otherwise Enter opens the first slot that needs input.
- On a command row, Tab opens the slots even when Enter would have run, so
  optional arguments can be set.
- In a slot, Enter commits the value. If nothing else is required, the command
  runs immediately. Prefilled optional slots are accepted as they are.
- In a slot, Tab and Shift-Tab move between slots without committing the flow.
- In a slot, Backspace on an empty field pops the previous chip and reopens it.
  On the first slot it returns to command search with the query intact.
- Escape goes back one level: slot to search, search to closed. Clicking the
  scrim closes.

### Command search

Search is fuzzy: `tfs` finds Toggle Fullscreen. Results rank by match quality,
then by this window's recent commands, then by process-wide command frequency,
then by catalog order. With an empty query the list shows recently used
commands first, then frequently used, then the rest alphabetically.

Only commands relevant to the captured context are listed; palette and text
editing commands require the `Palette` context and never appear. Each row shows
title, description, key-cap shortcuts relevant to the captured context, and a
small scope tag. Rows with arguments show a badge: a dashed
`⇥ options` badge when Enter would run immediately, or `↩ N required` when
Enter opens slots. Unavailable commands stay in the list dimmed, with the
reason replacing the description. Enter on one shows the reason inline and
keeps the palette open. Rows respond to hover and click; hover highlights
without moving the keyboard selection.

Dismissing the palette retains the query for a configurable period, fifteen
seconds by default, and reopening within it shows that query selected so typing
replaces it and Enter reuses it. A partially entered argument stage is never
retained.

### Argument slots

Choosing a command with arguments turns the input line into the invocation
being built, read left to right: a command chip, committed argument chips, one
active slot with a label and a per-argument placeholder, and any prefilled
optional chips after it. Only one slot is a live text field at a time.

| Argument | Slot | Beneath the line |
| --- | --- | --- |
| `Text` | Free text; placeholder names the argument or, for renames, shows the current name | Note about the prefilled target, with a Tab hint |
| bounded `Integer` | Digits; placeholder shows the range | Note with the range; inline error on Enter when invalid |
| `Tab` of `select_tab` | Fuzzy filter | Tab picker in most-recently-used order with position, title, and `cmd-N` where representable |
| `Bool` | Read-only value | Two-row picker, true / false |
| `Session`, `Workspace`, `Tab` | Fuzzy filter | Picker with label and ancestry; captured target preselected |
| `QuakeProfile` | Fuzzy filter | Picker of configured profiles with geometry and live window state; `default` preselected |

Identity and profile values always render as labels, never IDs or Debug output.
Pickers are strict: only listed values can be committed, and they filter with
the same fuzzy matcher as command search.

Arguments carry a `prompt` flag. Unprompted arguments exist for keybindings
and programmatic callers and never appear as slots; `select_tab`'s `index` is
one. An argument requirement is `Always`, `Optional`, or `OneOf(group)`, where
exactly one member of the group must be supplied. The palette treats a group
with no supplied member as required and prompts for its prompted member.

Required arguments block execution. Optional scalar arguments start unset so the
executor keeps its documented default. Optional identity arguments start as
prefilled chips holding the captured target. A required picker whose domain
has exactly one value is filled and skipped, and its chip carries no Tab hint.
A filter that narrows a larger domain to one row never skips; the user still
confirms. Picker selection follows the selected identity, not a row index,
across fuzzy reordering and hierarchy refresh; when that identity disappears
the selection resets to the best match. Arguments
supplied by an internal request are prefilled and skipped; a request with every
required argument executes without presenting slots.

Validation messages are written for people: "Index must be between 1 and 9",
not the protocol error's Display text.

### Interactive prompting

Invoking a command interactively without its required prompted arguments
opens the palette directly in that command's slot stage, following Emacs's
`interactive` prompting. "Required" means an `Always` argument or a `OneOf`
group with no member supplied; optional arguments never prompt, so a bare
`toggle_quake` still runs with the default profile. Keybindings, menu items,
and the palette itself are interactive callers; supplied arguments become
chips and the first unsatisfied slot is active. Non-interactive callers, such
as future IPC or CLI paths, still get `MissingArgument`.

This needs validation in two tiers. Keybinding compilation and
`CommandAction::new` currently run the full `validate`, which rejects an
argument-free `select_tab` binding before it could ever prompt. Split the
protocol validator: `validate_supplied` rejects unknown, duplicate, mistyped,
out-of-range, and mutually exclusive arguments; `validate` adds completeness.
Bindings, menus, and interactive requests use the first; execution uses the
second immediately before side effects. Configured runtime identities remain
forbidden in keybindings, as today. The keybinding schema therefore expresses
`select_tab` as "not both `index` and `tab`" with the existing range on
`index`, not as a `oneOf` that would reject the argument-free form.

Anything the palette dispatches counts toward its history, including a prompt
reached from a keybinding or menu. Commands that complete without the palette
do not.

A stage entered this way has no search stage beneath it. `palette_back` and
`palette_pop` from its first slot abort and close the palette, because that is
the level the user came from. Menu items for such commands end with an
ellipsis: Select Tab…, Rename Tab….

`select_tab` bound without an `index` is therefore a fuzzy tab switcher, and
`rename_tab` bound without a `name` is the rename prompt that was previously
deferred. Neither gets a new default binding in this delivery.

### Errors and focus

Validation and availability problems stay inline in the palette before it
closes. Once a command is dispatched, the palette is gone; any later failure
reaches the originating window's status line. The palette never reopens itself
to show an error.

Cancellation and completion restore terminal focus when the captured terminal
still exists and remains appropriate. Commands that navigate, create, or close
views own their final focus.

### Modality

The palette is modal within its window. Search input, editing shortcuts,
navigation keys, pointer events, and wheel events never reach the terminal
beneath it. Other windows and their terminals remain usable.

## Scope

This delivery includes:

- the interaction model above, replacing the two-stage argument view;
- fuzzy search with recency and frequency ranking, in memory only;
- mouse hover, click, and wheel support in the list and pickers;
- every palette keyboard operation as a catalog command with `Palette` scope,
  default bindings in the keymap tables, and user overrides through the
  existing `[[keybinding]]` system;
- a required context on each catalog command that decides palette listing;
- retained query with `palette.retain_query` and
  `palette.retain_query_seconds` configuration;
- a strict `QuakeProfile` argument kind used by `show_quake`, `hide_quake`,
  and `toggle_quake`;
- argument `prompt` flags and `OneOf` group requirements in the catalog;
- `select_tab` accepting either `index` or a `tab` identity;
- interactive prompting for missing arguments from keybindings and menus;
- new catalog commands `reset_tab_name`, `reset_workspace_name`,
  `reset_session_name`, and `select_recent_tab`, with a per-window tab
  activation history;
- `copy` unavailable without a selection;
- the visual restyle: rounded selection with accent bar, hover state, key-cap
  shortcuts, scope tags, footer key hints, and a lighter scrim;
- the retained foundations from the first iteration: captured targets,
  hierarchy pickers, availability, shortcut discovery, input isolation, and
  originating-window error reporting; and
- updated unit tests, native smokes, schema fixtures, and user documentation.

Out of scope: editing presets such as Emacs or Vim modes (a future named
keybinding collection feature, not a palette concern), persisted command
history, move and pane commands, command sequences, server or CLI invocation,
a TUI palette, settings UI, configurable palette appearance, shortcut glyph
rendering, accessibility beyond what GPUI 0.2.2 exposes, and any GPUI
dependency migration.

## State of the branch

The first iteration landed the runtime and safety work and it holds: catalog
command and bindings, atomically installed keymap snapshot with predicate
evaluation, availability queries beside each executor, captured targets and
scoped-ID pickers, terminal input isolation, focus restoration, and deferred
failure reporting to the originating window. Native smokes prove those.

Its user-facing layer is what this revision replaces:

- No mouse. Rows have no click handlers, and the window's capture-phase mouse
  handler stops propagation whenever a palette exists, so children never see
  hover.
- Argument mode reuses the search box unchanged, placeholder included. The only
  cue is a header row and a highlighted argument name.
- Identity values render as `TabId { runtime: RuntimeId(1), value: 3 }`.
- Rename needs a second Enter to accept an already-defaulted tab.
- Bool toggles on Up/Down with no affordance; optional text shows an empty row.
- Escape from arguments clears the search query.
- Diagnostics are raw `CommandError` Display strings.
- The palette closes on Enter and reopens itself on synchronous failure.
- Search is prefix and substring only; empty-query order is catalog order.
- Rows use full-width hairlines, a `›` text prefix, and `{:?}` of the scope
  enum. The scrim hides the terminal, including the tab being renamed.
- `WorkspaceView` recomputes availability and mutates the palette entity inside
  `render` on every frame.

## Design

### Ownership

Unchanged from the first iteration. `huterm-protocol` owns the catalog and
argument kinds and stays dependency-free. `huterm-core` executes runtime
commands and maps missing identities to `StaleTarget`. `desktop/palette.rs`
owns the palette state machine, captured target, search, argument drafts, and
events. `WorkspaceView` opens and removes the overlay, supplies runtime data,
dispatches, chooses focus return, and writes status. `ui/text_field.rs` and
`ui/picker.rs` stay free of command and runtime knowledge.

Move the per-render availability refresh out of `render`. Recompute it when
the palette opens, when the hierarchy snapshot arrives, and from the window
refresh pump after tab, selection, quake, or busy-state changes, then hand the
palette an immutable map. Hierarchy names and quake profile state refresh at
the same points; the palette never polls.

### State machine

```rust
enum PaletteStage {
    Commands(CommandSearch),
    Slots(SlotEditor),
}
```

`CommandSearch` owns the query, ranked result indices, keyboard selection, and
hover index. `SlotEditor` owns the `CommandSpec`, an ordered `Vec<Slot>`, the
active slot index, the active slot's text, picker state, and the current
inline error. Each `Slot` records its `ArgumentSpec`, value, and whether it is
committed, prefilled, sole-option, or explicitly supplied.

Transitions: `commit` validates the active slot, marks it committed, then runs
if every required slot has a value, otherwise advances to the next slot that
needs input. `pop` reopens the nearest earlier committed slot or returns to
`Commands` with the query intact. `edit(i)` reopens slot `i` with its committed
scalar text, or an empty filter for pickers. `advance(±1)` moves without
committing. All of this is testable without a window.

### Search and ranking

Use `nucleo-matcher` for matching. It is MPL-2.0, which `deny.toml` allows, and
it handles Unicode normalisation, case, word-boundary and consecutive-match
bonuses that a hand-rolled scorer gets wrong. Verify the release-age policy and
`mise run license` when adding it. Match the title primarily; also match the
stable ID and description at lower weight so `reload` still finds Reload
Configuration.

Match title, stable ID, and description separately. Field priority is
lexicographic: any title match outranks any ID-only match, which outranks any
description-only match; within a field, the matcher score orders results. A
strong description hit therefore never outranks a weak title hit. Ranking
composes three sources: the matcher score, a per-window recency list held by
`WorkspaceView`, and a process-wide frequency map on the `Desktop` global.
Both record palette dispatches only, at the moment the executor accepts the
invocation; executions that complete without the palette do not count,
matching Emacs's extended-command history. Both histories are in memory.
Define them behind a small trait so a persisted store can replace them later
without touching search. Persistence is deferred: Huterm writes no state
today, and a durable history needs a state directory, atomic writes, and
multi-instance merging.

### Argument kinds

Add `ArgumentKind::QuakeProfile` to the protocol. Like the identity kinds it
declares a domain the client resolves; its value stays `CommandValue::Text`, so
keybinding `args = { profile = "logs" }`, the schema, and the quake executors
are unchanged. `validate` accepts `Text` for it. The palette lists profiles from
the loaded config, annotated with geometry and the quake window registry's
state for each: visible, hidden with a tab count, or not yet summoned. The
existing "unknown quake profile" error stays for non-palette callers.

`select_tab`'s position contract cannot name every tab: position 9 means the
last tab, and positions above 9 fail validation, so a picker over positions
would select the wrong tab or be unable to run with ten or more tabs. Instead
the command gains a second argument: `index` (bounded integer, unprompted) and
`tab` (identity, prompted) form a `OneOf` group. Existing
`args = { index = 3 }` bindings are unchanged. Supplying both is rejected at
validation. The executor activates a supplied `tab` when it belongs to this
window and reports `StaleTarget` otherwise; `index` keeps its current path.
The palette prompts only for `tab`, through the identity picker with rows
showing position, title, and the `cmd-N` shortcut where one exists.

`validate` learns the group requirement; the schema and the two-tier
validation are described under interactive prompting. Move commands from #32
are the expected second user of both the group and the `prompt` flag.

Identity pickers keep the hierarchy snapshot from the first iteration. The
single-option skip and Enter-runs decisions depend on that snapshot. When Enter
arrives before it, record a pending intent tagged with the palette generation,
show a brief pending state in the slot line, and complete the action when the
snapshot lands. Any further input, whether typing, picker or list
navigation, chip clicks, `palette_back`, `palette_pop`, close, or a snapshot
failure clears the intent; a failed snapshot shows its error inline. Each
palette opening is a new generation, so a stale completion can never execute.

The snapshot carries each record's custom name, so reset-name availability for
the captured or chosen target is answered from it without touching Mux. Core
still validates at execution.

### New catalog commands

- `select_tab` picker: lists every tab in this window ordered by the
  activation history, most recent first, with the active tab last. Initial
  selection is the first row, so Enter goes to the most recently used other
  tab; with one tab that row is the active tab. Fuzzy filtering reorders by
  match score as everywhere else.
- `reset_tab_name`, `reset_workspace_name`, `reset_session_name`: Runtime
  scope, one optional identity argument each, unavailable when the target has
  no custom name. Core already clears an override on `None`.
- `select_recent_tab` ("Switch to Last Tab"): Window scope, no arguments,
  default `ctrl-tab` on both platforms. `WorkspaceView` keeps an ordered
  activation history updated on every tab activation and pruned on close.
  Because activation updates the history, repeating the command toggles between
  the two most recent tabs.

`copy` becomes unavailable without a terminal selection, with "no selection"
as the reason.

### Mouse

Remove the palette clause from the window's capture-phase mouse-move handler.
The overlay blocks motion, buttons, and wheel from reaching the terminal by
itself. Rows and picker items take `on_click`, hover styling, and a pointer
cursor. Clicking a chip reopens that slot. Clicking the scrim cancels. The list
uses GPUI's scrollable container so the wheel scrolls it.

### Palette and text commands

Everything the keyboard can do in the palette is a catalog command, following
Emacs: only commands are bindable, and the extended command list is the set of
commands relevant to the current context. The hardcoded `Palette` and
`PaletteText` GPUI bindings on the branch are removed.

Add `CommandScope::Palette`, executed by the window's open palette through a
new `InvokePalette` wrapper action bound in the `Palette` context, mirroring
the existing per-scope wrappers. Availability is "this window has an open
palette", so an unconditional user binding is harmless when it is closed.

Navigation and slot commands: `palette_select_next`,
`palette_select_previous`, `palette_page_down`, `palette_page_up`,
`palette_confirm`, `palette_back` (Escape: slot to search, search to closed),
`palette_pop` (reopen the previous committed chip, or return to search from
the first slot), `palette_expand`, `palette_next_slot`,
`palette_previous_slot`.

Text editing commands, named by what they do to text so a later text field
elsewhere reuses them: `text_delete_backward`,
`text_delete_forward`, `text_delete_word_backward`, `text_delete_line_start`,
`text_move_left`, `text_move_right`, `text_move_word_left`,
`text_move_word_right`, `text_line_start`, `text_line_end`,
`text_select_left`, `text_select_right`, `text_select_all`, `text_copy`,
`text_paste`.

Both families share the single `Palette` context; `PaletteText` is removed
from the field, the README table, and the fixtures. With one context, a user
override of any palette or text command competes with the default at equal
depth and wins by the existing ordering rule. Two contexts would let a deeper
default silently beat a shallower user override.

A command's required context is compiled into every binding for it: the
keymap ANDs the context into the binding's predicate, so a user entry needs no
`when` and cannot reserve a stroke from the terminal even if written without
one. That is what makes a stray `ctrl-c = text_copy` harmless: it is
conditional, and conditional single-key bindings reserve nothing. Default
bindings move into the platform keymap tables and appear in the README table
like any other command. Extend the real-matcher tests to prove a user `up`
bound to `text_line_start` beats the default `palette_select_previous`, that
a user `ctrl-c` to `text_copy` leaves terminal `ctrl-c` unreserved, and that
reload reinstalls everything in one operation. Defaults include `up`/`down`, `enter`,
`escape`, `tab`/`shift-tab`, `pageup`/`pagedown`, `backspace`/`delete`,
`left`/`right`, `alt-left`/`alt-right`, `home`/`end`, `alt-backspace`,
`cmd-backspace` on macOS, `cmd-a`/`ctrl-a` select-all, and the platform copy
and paste chords already present.

Backspace on an empty field is palette behaviour, not text editing: the
palette checks that the field is empty before dispatching
`text_delete_backward`, and runs `palette_pop` instead. "Nothing was deleted"
is not the test, because Backspace at the start of non-empty text also deletes
nothing and must leave the draft alone. Printable input is not a command; it
arrives through the field's input handler as today.

Tab from a text slot validates and commits its text when non-empty, then
moves; an invalid value shows the inline error and stays. An empty text slot
is left uncommitted. Tab from a picker commits the highlighted row. Tab on
the last slot and Shift-Tab on the first do nothing.
Reopening a committed optional chip, clearing its text, and committing unsets
it.

### Listing by required context

Add a required context to `CommandSpec`, held as a string so the protocol
crate stays dependency-neutral: `None` for ordinary commands, `"Palette"` for
the commands above, and available for future contexts such as a confirmation
dialog. The GPUI client parses it with the same
`KeyBindingContextPredicate` path the keymap uses and a catalog test parses
every entry so a typo fails CI.

The palette lists a command only when the context stack captured before it
opened satisfies the requirement. Context mismatch means the command cannot
structurally apply here, so it is hidden; a state problem such as no selection
means dimmed with a reason. `open_command_palette` is listed like any other
command; confirming it is handled inside the palette as a no-op that keeps the
query and selection, before the close-and-dispatch path, because that path
removes the overlay first and would otherwise create a second palette with a
new target. Its hand-rolled filter goes away.

### Retained query

`WorkspaceView` stores the last query and dismissal instant on cancellation
only; execution clears it. On open, if `palette.retain_query` is on and the
elapsed time is within `palette.retain_query_seconds`, the field starts with
that query selected. The options are read on each open, so a reload applies to
the next opening. Seconds are bounded to 0 through 3600.
Both options live in a new `[palette]` config section with schema entries and
fixture cases.

### Presentation

Panel width 620 logical points clamped to the viewport, top-anchored, rounded,
with a shadow. Scrim at roughly 55% so the terminal beneath stays legible.
Rows: rounded selection using the theme selection colour with a 3-point accent
bar, a lighter hover fill, title, muted description, and right-aligned key-cap
shortcuts plus a small uppercase scope tag. Slot line: command chip in the
accent tint, value chips in the chip fill, prefilled chips dashed with a `⇥`
hint, slot label in muted text followed by the field. Footer with the live key
hints for the current stage. Errors in a tinted band under the line; notes in a
muted band. All derived from theme foreground, background, and selection
colours; no new configuration. Invalidate colours on theme reload.

Reference: the interactive mockup shared during review, which the mise task
does not depend on.

## Configuration

```toml
[palette]
retain_query = true
retain_query_seconds = 15

[[keybinding]]
key = "ctrl-tab"
command = "select_recent_tab"

[[keybinding]]
key = "ctrl-n"
command = "palette_select_next"   # context implied by the command
```

Regenerate both schemas with `mise run schema:generate` and extend
`schemas/fixtures.json` for the new section, the `QuakeProfile` argument, the
`Palette` scope, and every new command ID.

## Work breakdown

The implementation is split into chunks with explicit ownership. Each chunk
has its own plan under `docs/plans/command-palette/`, written so an
implementer needs nothing beyond it and this document.

| Chunk | Plan | Owner | Depends on |
| --- | --- | --- | --- |
| 1 | [Protocol catalog](command-palette/01-protocol-catalog.md) | Codex | none |
| 2 | [Keymap and dispatch](command-palette/02-keymap-dispatch.md) | Codex | 1 |
| 3 | [Configuration and schema](command-palette/03-config-schema.md) | Codex | 1 |
| 4 | [Search and ranking](command-palette/04-search-ranking.md) | Codex | 1 |
| 5 | [Executors and window integration](command-palette/05-executors-window-integration.md) | Codex | 1 to 4 |
| 6 | [Slot model and palette UI](command-palette/06-slot-model-palette-ui.md) | this session | 1 to 5 |
| 7 | [Smokes and documentation](command-palette/07-smokes-docs.md) | Codex | 6 |

Chunks 2, 3, and 4 run in parallel in separate worktrees; they touch
disjoint files. Chunk 5 must land before chunk 6 starts because both edit
`windows.rs`. Each Codex chunk ends with the implementer's own self-review
against its plan, then an integration pass by this session before it lands
on the PR branch. After chunk 7 the owner decides whether and how to run
a final review of the complete diff; it does not start automatically.

## Implementation sequence

1. Catalog: required context on `CommandSpec`, `prompt` flag and `OneOf`
   requirement on `ArgumentSpec`, `select_tab`'s `tab` argument,
   `CommandScope::Palette`, the palette and text commands, `QuakeProfile`,
   reset commands, `select_recent_tab`, default bindings, schema, fixtures,
   README tables. Prove validation, group exclusivity, context parsing, and
   default matching.
2. Slot model: replace `ArgumentEditor` with `SlotEditor`, single-option
   skip, pop and edit transitions, human validation messages, tab-index
   presentation. Pure tests first.
3. Search: add `nucleo-matcher`, the ranking pipeline, recency and frequency
   sources, retained query. Pure tests with a targeted perturbation on the
   ranking order.
4. Mouse and keys: capture-handler change, row and chip handlers, wheel,
   `InvokePalette` dispatch replacing the hardcoded component bindings,
   matcher tests for binding precedence and user overrides.
5. Availability and execution: interactive prompting from `run_command`,
   `select_tab` identity execution, `copy` selection check, reset-name
   availability, status-line error routing, availability refresh outside
   `render`, pending-snapshot handling.
6. Presentation: slot line, chips, badges, footer, scrim, theme invalidation.
7. Quake profile picker with live state; MRU tab history and Switch to Last
   Tab.
8. Smokes, docs, `mise run verify`, manual visual pass.

Steps 2 and 3 are independent after 1. Steps 4 through 7 build on 2.

## Testing and verification

### Pure and focused tests

- Ranking: fuzzy hit beats frequent weak match; recency beats frequency;
  empty query orders recent, frequent, alphabetical; ties stable.
- Slots: Enter runs on all-optional and single-option commands; required slot
  blocks; pop reopens the previous chip and differs from back; pop from the
  first slot restores the query; edit reopens with text; Tab commits valid
  text, stays on invalid text, and leaves empty slots uncommitted; explicit
  arguments skipped; prefilled identity written explicitly into the
  invocation; the `select_tab` picker is in MRU order and shows
  `cmd-N` only for representable positions with ten tabs in the fixture;
  strict pickers reject unlisted values and filter fuzzily; messages are the
  human forms.
- Prompting: an interactive `select_tab` with no arguments opens the slot
  stage; back and pop from its first slot close the palette; a
  non-interactive invocation still fails with `MissingArgument`; supplied
  arguments become chips; a malformed supplied argument fails before any
  prompt opens; an argument-free `select_tab` binding passes schema,
  compilation, and action construction; a bare `toggle_quake` runs without
  prompting.
- Picker: tabs are ordered by activation history with the active tab last;
  one tab lists and preselects the active tab; two tabs preselect the other
  one and still open the picker; a filter narrowing to one row does not
  execute; selection follows identity across reordering.
- Ranking: a strong description match stays below a weak title match.
- Pending Enter is cancelled by picker navigation and by `palette_pop`.
- Pending snapshot: Enter before the snapshot executes once when it lands;
  an edit, back, close, or snapshot failure cancels it; a stale generation
  never executes.
- Catalog: every required context parses; palette and text commands are
  `Palette` scoped and hidden from a captured terminal context; ordinary
  commands remain listed; `select_tab` accepts `index` or `tab` and rejects
  none or both; `QuakeProfile` accepts `Text` values; reset commands
  validate; `select_recent_tab` is argument-free and Window scoped.
- Text field: word and line deletion, Backspace at the start of non-empty
  text leaves it unchanged, existing grapheme and composition tests.
- Keymap: default palette bindings dispatch through `InvokePalette`; a user
  `up` bound to `text_line_start` beats the default; a user `ctrl-c` bound to
  `text_copy` without `when` is compiled conditional and reserves nothing;
  reload reinstalls everything in one operation.
- Errors: a synchronous dispatch failure lands in the status line and the
  palette does not reopen.
- Retained query: within and beyond the window, and disabled.
- History: activation order, pruning on close, toggle between two tabs.
- Execute palette rename and reset invocations through core; stale target
  still changes nothing.

Use a targeted perturbation for the ranking test and one slot transition test.

### Native smoke coverage

Extend `smoke:macos-palette` and `smoke:linux-palette`:

- open, fuzzy search `tfs`, Enter runs Toggle Fullscreen (or another safe
  argument-free command);
- Toggle Quake from the list runs with the default profile; Tab opens the
  profile picker; picker lists configured profiles;
- rename with one Enter after typing the name, then a second rename that
  types the name, presses Tab to enter the tab picker, chooses another tab,
  and presses Enter; core rename results match;
- a keybinding to `select_tab` without arguments opens the tab picker; fuzzy
  text selects a tab; Escape from it closes the palette with a shell
  acknowledgement; a keybinding with `index` still selects directly;
- hover a row and confirm the keyboard selection did not move; click a row
  and observe execution; wheel over the list and confirm nothing reaches the
  PTY; click the scrim and observe cancellation with a shell acknowledgement;
- Backspace-on-empty returns to search with the query intact; dismiss and
  reopen within the retention window shows the query;
- `copy` unavailable without a selection and available with one;
- reset name after rename, and unavailable before;
- Switch to Last Tab toggles between two tabs;
- existing cases: input isolation, modal routing, stale target, deferred
  failure in the originating window, macOS composition.

Keep the smokes wired through `ci:smoke:build`, `ci:smoke:run`, and
`.github/workflows/ci.yml`.

### Manual checks

Narrow and half-height quake windows for clipping; long tab titles in pickers;
close confirmation arriving while the palette is open; palette inside a quake
window with `hide_on_focus_loss`; theme reload with the palette open; window
deactivation and reactivation. Convert any reproduction into a smoke where the
harness can express it.

## Settled decisions

- One rule: Enter does the most likely thing, Tab opens the next choice.
- One active slot at a time; chips for everything else.
- Fuzzy matching through `nucleo-matcher`; in-memory recency and frequency.
- Unavailable commands stay visible, dimmed, with a reason. This includes
  `copy` without a selection.
- Strict pickers. Labels only, never IDs.
- Execution errors go to the status line; the palette never reopens itself.
- Every palette keyboard operation is a catalog command with `Palette`
  scope; listing is decided by a required context on the spec, following
  Emacs's interactive-command model. No editing presets in this delivery.
- Reset commands rather than empty-name-clears.
- `select_tab` takes `index` or `tab`, exactly one; `index` is unprompted.
  Interactive invocation without arguments prompts through the palette with
  tabs in most-recently-used order, and aborting that prompt closes the
  palette. `select_recent_tab` is a separate
  command. No new default binding for the switcher yet.
- One `Palette` context; required contexts are compiled into bindings.
- Persisted history and editing presets are deferred, with their extension points
  defined here.

## Open questions

None.

[issue-23]: https://github.com/jimeh/huterm/issues/23
[issue-24]: https://github.com/jimeh/huterm/issues/24
[issue-32]: https://github.com/jimeh/huterm/issues/32
