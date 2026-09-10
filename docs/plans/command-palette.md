# Command palette and argument pickers

Status: proposed.

Deliver [#24: searchable command palette with argument pickers][issue-24] as
one GPUI client PR. Build it on the command catalog and scoped execution paths
that landed in [#23][issue-23]. The palette searches commands, explains current
availability, collects typed arguments, captures its invocation target, and
then calls the same validated operation as menus, keybindings, and programmatic
callers.

The issue mentions palette-driven move and rename commands because it assumed
the epic's original delivery order. Huterm has since taken a more practical
order. Rename commands exist in the catalog and will prove shared execution in
this delivery. Move commands remain with [#32][issue-32] and will become palette
actions by registering with the catalog later.

## Outcome and user contract

Users open the palette with `cmd-shift-p` on macOS or `ctrl-shift-p` on Linux.
The View menu also exposes Open Command Palette with the effective shortcut.
Opening it captures the current window, session, workspace, tab, terminal, and
keybinding contexts before focus moves into the palette.

The initial view contains a focused search field and every applicable catalog
command except `open_command_palette` itself. Search is case-insensitive and
matches whitespace-separated terms across the command title, stable ID, and
description. Every term must match. Results rank title prefixes, title word
matches, stable-ID matches, and description matches in that order, retaining
catalog order for ties. This is deterministic and sufficient for the current
catalog without adding a fuzzy-search dependency.

Each result shows its title, description, scope, relevant shortcuts, and
availability. Unavailable commands remain visible with a reason. Enter on an
unavailable row does not close the palette or execute the command.

A command without declared arguments executes when selected. A command with
arguments opens an argument view containing every declared argument in catalog
order:

| Argument kind | Editor |
| --- | --- |
| `Bool` | `true` / `false` choice |
| bounded `Integer` | Text field with range validation |
| `Text` | Single-line text field |
| `Session` | Searchable session picker |
| `Workspace` | Searchable workspace picker with session ancestry |
| `Tab` | Searchable tab picker with workspace and session ancestry |

Required arguments must have a value before execution. Optional scalar
arguments start unset so the executor retains its documented default. Optional
identity arguments start with the target captured when the palette opened. A
user can replace that target through the corresponding picker. Arguments
supplied explicitly by an internal or future programmatic palette request are
prefilled and skipped during keyboard progression. A request that already
contains every required argument may execute without presenting the argument
view.

Escape always cancels the current palette without running a command. Escape
from an argument view returns to command search first; Escape from command
search closes the overlay. Completion closes the palette before dispatch.
Cancellation and ordinary completion restore terminal focus when the captured
terminal still exists and remains appropriate. Commands that navigate, create,
or close views retain ownership of their final focus.

The palette is modal within its window. Search input, editing shortcuts,
navigation keys, pointer events, and scroll events never reach the terminal
beneath it. Other windows and their terminals remain usable.

## Scope

This delivery includes:

- an `open_command_palette` window command, default bindings, and View menu
  item;
- deterministic command search over catalog metadata;
- current command availability with visible refusal reasons;
- relevant shortcut display from the effective keymap;
- a reusable single-line GPUI text field with native composition support;
- command and identity picker presentation;
- typed argument collection and validation;
- captured invocation targets and stale-target handling;
- focus restoration and complete terminal input isolation;
- originating-window reporting for synchronous and asynchronous failures;
- focused unit tests and native macOS/Linux smoke coverage; and
- user documentation for opening and operating the palette.

Move commands, pane commands and `ArgumentKind::Pane`, command sequences,
server or CLI invocation, a TUI palette, settings UI, command-history ranking,
usage-based ranking, configurable palette appearance, and a GPUI dependency
migration are out of scope. The implementation should leave direct extension
points for new catalog commands and argument kinds without defining unused
models for them now.

## Current foundations and gaps

[`huterm-protocol::command`][protocol-command] is the single catalog definition.
It supplies stable command IDs, presentation metadata, execution scope, typed
ordered arguments, invocation values, validation, outcomes, and errors. Its
empty dependency set remains unchanged. Palette presentation and GPUI types do
not move into protocol.

[`huterm-core::commands`][core-commands] executes runtime rename invocations.
Clients must fill omitted targets before entering core. Core validates the
scoped ID under the Mux lock and maps missing or foreign identities to
`CommandError::StaleTarget`. The palette must preserve this path instead of
calling Mux rename or future move methods directly.

[`huterm-gpui::commands`][gpui-commands] maps catalog scopes onto GPUI actions.
`Desktop::invoke`, `WorkspaceView::run_command`, and
`TerminalView::run_command` already provide the common execution path. Runtime
commands run on the structural worker and report later failures through their
originating `WorkspaceView`.

[`keymap::CompiledKeymap`][keymap] retains effective binding keys, commands,
arguments, `when` source, descriptions, and origins for later UI. The current
install path consumes the compiled keymap, binds its GPUI actions, and keeps
only the reserved terminal-key set. The palette needs an atomically installed
read-only snapshot of both reserved keys and effective bindings. It also needs
the parsed `when` predicate so it can evaluate shortcuts against the context
captured before palette focus.

Command availability currently exists only within executing methods. Busy
structural work, close confirmation, tab reorder, terminal exit, quitting,
fullscreen ownership, quake state, and platform support are checked at
different owners. The palette cannot infer those rules from scope or catalog
metadata. Extract read-only preconditions next to their existing owners and
reuse them during execution.

Huterm has no general text-field component. The existing terminal
`Composition` type contains reusable pure UTF-8/UTF-16 range mapping, but its
input handler and editing workflow are PTY-bound. Extract that mapping into a
shared private helper rather than duplicating it. GPUI's input handler contract
supplies native text and IME events, but Huterm must own editing, selection,
clipboard, focus, and rendering behavior for a normal single-line field.

The global keystroke observer routes any unconsumed keystroke to the active
terminal even when another child owns focus. `WorkspaceView::busy`, close
confirmation, and reorder state currently block that route. Palette visibility
must join those blockers. The macOS composed-text gate and terminal pointer
routing need the same treatment.

## Design

### Opening and palette ownership

Add `ids::OPEN_COMMAND_PALETTE` to the protocol catalog as an argument-free
Window command. The catalog remains the source for its title, description,
menu label, keybindings, and scope. Add the two platform defaults to the
existing keymap tables.

`WorkspaceView` owns at most one `Entity<CommandPalette>`. Invoking Open Command
Palette while one exists focuses its search field without stacking another
overlay or resetting entered arguments. Refuse the initial open while close
confirmation, structural work, tab reorder, or application shutdown makes a
modal workflow unsafe.

Keep application workflow in a new `desktop/palette.rs` module. It owns:

- the command-search and argument-editing state machine;
- the captured invocation target and pre-palette key contexts;
- the catalog projection, search results, and selected row;
- a point-in-time hierarchy snapshot for identity pickers;
- the invocation draft and per-field diagnostics; and
- events for cancel, execute, and synchronous failure.

`WorkspaceView` remains responsible for opening and removing the overlay,
obtaining runtime data, dispatching the completed invocation, choosing the
focus return, and writing window status. `CommandPalette` never owns Mux,
terminal clients, configuration, or window-lifecycle policy.

Add small private modules under `huterm-gpui/src/ui/` for the single-line text
field and generic picker list. They may depend on GPUI and presentation tokens,
but not on `huterm-core`, command IDs, Mux records, or desktop state. Keep these
module interfaces narrow enough to extract into a private GPUI component crate
when a second substantial consumer such as settings justifies that boundary.

Do not adopt GPUI Kit in this delivery. Huterm pins official GPUI 0.2.2, while
current GPUI Kit uses the separately published `gpui-pre` line. Migrating the
framework would widen #24 into native window, input, rendering, and keybinding
compatibility work.

### Palette state machine

Use an explicit state rather than booleans spread across the view:

```rust
enum PaletteStage {
    Commands(CommandSearch),
    Arguments(ArgumentEditor),
}
```

`CommandSearch` owns query text, filtered catalog indices, and a selected
result. `ArgumentEditor` owns the selected `CommandSpec`, values keyed by
argument name, the active field, picker state, and validation messages. Values
remain named even though presentation follows catalog order.

Every transition clamps selection to the current result set. An empty search
shows the full catalog in catalog order. A search with no results renders an
explicit empty state. Editing the query must not recreate the palette entity or
replace its focus handle.

The state model should be testable without opening a native window. Keep search
ranking, result selection, argument progression, scalar parsing, target
replacement, and invocation construction free of GPUI entities where practical.

### Text input and component keybindings

The text field implements `EntityInputHandler` and registers it on both macOS
and Linux so AppKit and X11 deliver text, marked text, and input-method
composition through GPUI. Store content and selection as UTF-8 byte ranges
while converting at the GPUI boundary through the extracted shared mapping.
Cursor movement and deletion use Unicode grapheme boundaries. Do not reuse
terminal Option composition state.

Support character input, marked-text replacement, left/right, home/end,
backspace/delete, select all, selection extension, copy, and paste. Replace
pasted newlines with spaces. Defer cut and mouse selection until another
consumer or a concrete accessibility requirement justifies them.

Land the palette-visibility blocker in the global terminal keystroke observer
before integrating the text field. On X11, GPUI sends unconsumed text through
the input handler after action dispatch; without that blocker, the terminal
observer can consume the keystroke before the focused palette field receives
it.

Palette navigation and text-editing actions need context-specific GPUI
bindings. `bind_keymap` calls `clear_key_bindings()` on startup and every
successful reload, so it must reinstall these component bindings in the same
operation that installs command bindings. Install catalog command bindings
first and palette component bindings afterward. A plain `Palette` predicate
does not by itself outrank a predicate-free binding at equal context depth;
installation order is therefore part of the contract. Add a real GPUI matcher
test proving palette Copy, Paste, and navigation win while the field is focused
and that reload preserves the ordering.

### Input isolation and focus

Opening the palette clears terminal Option composition and focuses the palette
input. The existing terminal blur callback emits application focus-out and
finalizes local pointer state.

While `WorkspaceView` owns a palette:

- add `palette` to its key context alongside the existing `Palette` component
  context;
- include palette visibility in the global keystroke observer's
  `input_blocked` decision;
- include it in macOS `terminal_input_allowed`;
- capture palette navigation and editing actions before terminal actions;
- cover the whole window with an overlay that stops pointer and wheel
  propagation; and
- prevent tab drag, sidebar resize, link activation, selection, and application
  mouse reporting beneath the overlay.

Capture a weak terminal entity and the active tab ID when opening. On cancel,
focus that terminal if it is still the visible active tab. If it disappeared or
navigation legitimately changed, focus the current active terminal, then fall
back to the workspace focus handle when the window has no terminal. On command
execution, dismiss and restore focus before dispatch so existing command paths
can take ownership of later focus changes.

### Captured targets and hierarchy pickers

Create a client-private `PaletteTarget` containing the originating window and
the session, workspace, tab, and terminal identities available when the palette
opened. It also retains the pre-palette key-context stack used for shortcut
display. Do not add this GPUI-specific context to `CommandInvocation` or
`huterm-protocol`.

The current `WorkspaceView` stores workspace and tab IDs but not the owning
session ID. Capture the full selection ancestry through a short background Mux
read using `Mux::select_tab` or `Mux::select_workspace`, which already returns a
`SelectionTarget`. Never acquire the Mux mutex from render, input, or focus
callbacks. The palette can present command search immediately, but it must
disable final acceptance until the captured ancestry required by that command
is available. An identity picker opened before the hierarchy snapshot arrives
shows a loading row.

The same background read returns `HierarchySnapshot` for picker options.
Project it into client-private rows containing scoped IDs, kind, display name,
and ancestry. Overlay live titles from retained `TabView` records where the
desktop has them; otherwise use the tab's custom or stable fallback name. Show
ancestry in labels because display names are not unique. Picker selection
stores only the scoped ID in `CommandValue`.

Keep one hierarchy snapshot for the palette generation. Structural changes may
make an option stale after capture. Do not re-resolve its name, substitute the
new active target, or silently remove its identity at execution. Pass the ID to
the existing executor and report `StaleTarget`. A later enhancement may refresh
labels, but execution still requires the selected ID to validate.

When the user accepts the captured default for an optional identity argument,
write that ID explicitly into the invocation. This prevents later navigation
from changing the target through `fill_rename_target`. Explicit arguments in a
palette request always win over captured defaults, matching existing command
semantics. In particular, `rename_session` waits for the captured session ID;
it must not omit the argument and let execution resolve whichever session owns
the workspace later.

### Shortcut discovery

Replace `Desktop::reserved` with an atomically replaced keymap snapshot that
contains `Arc<ReservedKeys>` and `Arc<[EffectiveBinding]>`. Preserve effective
resolution order and retain each parsed `KeyBindingContextPredicate` next to
its source text. Evaluate a retained predicate against the captured context
with GPUI's public `depth_of` API; its internal `eval` helper is not available
to Huterm.

For a command row, relevant shortcuts are effective bindings whose command ID
matches and whose predicate matches the context stack captured before palette
focus. Unconditional bindings always match. When the argument editor has a
complete invocation, prefer bindings whose named arguments equal that
invocation. Before arguments are complete, show argument-bearing bindings with
their retained descriptions rather than pretending every chord invokes the
same values. Keep the first compact set on the command row and allow additional
matches to wrap only in the argument view; do not add a separate command-detail
panel in this delivery.

The palette consumes the installed keymap snapshot. It does not reparse
`config.toml`, inspect GPUI's private dispatch tree, or maintain another table
of default shortcuts. Reload must replace bindings, reserved keys, menu
shortcuts, and palette discovery metadata together. An already open palette
reads the replacement snapshot on its next render.

### Availability

Add a read-only availability query beside each existing execution owner. A
window-owned form may look like:

```rust
fn command_availability(
    &self,
    command: CommandId,
    target: &PaletteTarget,
    cx: &App,
) -> Result<(), CommandError>;
```

The exact signature varies by owner. Keep application availability in a free
function beside `run_app_command` and quake execution so it does not require
simultaneously borrowing `Desktop` and `App`. Window and runtime-context checks
stay with `WorkspaceView`, and terminal checks stay with `TerminalView`.
Extract small shared precondition helpers from current executors and call them
from both availability and execution. Do not encode busy, close, fullscreen,
quake, or terminal-exit policy in the palette.

Command-list availability checks handler presence, captured client target,
platform capability, and current state without requiring arguments the user
has not entered. After argument collection, normal catalog validation and the
executor repeat all relevant checks immediately before side effects. A command
may therefore become unavailable while the palette is open; the execution
failure remains authoritative and visible.

Availability means the executor can accept the command, not that it must
change observable state. Existing idempotent or harmless no-op commands remain
available unless their owner already defines refusal semantics.

### Execution and error reporting

The palette builds a normal `CommandInvocation` and validates it through
`huterm_protocol::validate`. Runtime identity defaults are filled from
`PaletteTarget` before validation and dispatch. Application commands use the
application executor. Window and Runtime commands run through the originating
`WorkspaceView`. Terminal commands dispatch directly to the captured
`TerminalView`; they must not go through `Desktop::invoke`, which resolves the
active terminal at execution time and could retarget an invocation after the
palette opened.

Synchronous validation, availability, or dispatch failure leaves the palette
open and shows an inline diagnostic. Once a command returns `Accepted`, close
the palette. Later failure appears in the originating `WorkspaceView` status,
not a palette entity that may no longer exist and not whichever window happens
to be active.

Audit application commands during implementation. New-window and quake paths
currently contain deferred failures that can reach stderr or a global active
window. Thread an originating-window reporter through palette dispatch so those
failures reach the invoking window without changing the behavior of existing
menu, keybinding, or OS-global callers. Do not close, retarget, or mutate an
unrelated window while reporting an error.

### Presentation

Render the palette as a viewport-clamped panel near the top center of the
window over an opaque or strongly dimmed scrim. Give it an explicit maximum
width and height. The result list scrolls within the panel and keeps the
selected row visible. The current catalog is small enough for ordinary flex
children; do not introduce virtualization until measurements justify it.

Use existing theme foreground, background, selection, and muted-opacity values
rather than adding configuration. Distinguish selection, unavailability, field
errors, and shortcut labels without relying on color alone. Preserve readable
contrast over terminal content and in fullscreen or quake presentation.

## Testing and verification

### Pure and focused tests

Add model tests for:

- empty search, case-insensitive terms, multi-term matching, ranking, and stable
  ties;
- selection after filtering and no-result behavior;
- unavailable rows retaining their reason and refusing execution;
- argument order, required-field blocking, optional omission, scalar parsing,
  bounded integers, explicit values, and invocation construction;
- captured identity defaults, explicit target replacement, duplicate display
  names, and scoped IDs;
- relevant shortcuts under unconditional and `when` predicates using the
  captured context rather than `Palette`; and
- exact argument matching for argument-bearing bindings.

Add text-field tests for Unicode grapheme movement and deletion, shared
UTF-8/UTF-16 range conversion, marked-text replacement, keyboard selection,
and newline-free paste. Use a targeted perturbation for at least one material
search or argument-flow test so its intended assertion is known to fail.

Extend command and keymap tests to prove:

- `open_command_palette` has Window scope and argument-free validation;
- both platforms' default shortcut dispatches it;
- palette component bindings outrank terminal clipboard/navigation actions in
  a real GPUI keymap; and
- startup and reload retain effective bindings and reinstall palette component
  bindings atomically with reserved keys and menus.

Exercise palette-produced rename invocations through the existing core executor.
Prove a chosen tab is renamed, a duplicate display name does not affect target
selection, an explicit target wins, and deleting the chosen target before
execution returns `StaleTarget` without renaming another tab.

### Native smoke coverage

Add dedicated `smoke:macos-palette` and `smoke:linux-palette` tasks following
the existing desktop smoke structure. Drive real native key events and both PTY
engines where terminal input and focus behavior are under test. The smoke must
cover:

- opening through the default shortcut, searching, keyboard selection, and
  executing an argument-free command;
- entering a rename, choosing a target, and observing the same core rename
  result as programmatic invocation;
- passing a complete explicit invocation without interactive collection;
- Escape from arguments and command search, with no command effect;
- a unique shell acknowledgement before opening and after cancellation or
  completion, proving focus restoration;
- typed search and argument text never appearing in the PTY;
- an unavailable command remaining visible with its reason;
- deletion of a picker target before acceptance producing the stale-target
  diagnostic without affecting another target; and
- an accepted command's deferred failure appearing in the originating window
  while another window remains open and unchanged.

On macOS, include marked-text or IME composition through the palette input and
verify opening cancels any pending terminal Option composition. Use the native
input harness rather than synthetic model calls for that evidence.

Wire both smoke binaries into `mise.toml`'s `ci:smoke:build`, add their names
and executions to `ci:smoke:run`, and add explicit platform steps in
`.github/workflows/ci.yml`. A discoverable local task without those three CI
connections is not complete coverage.

After focused iteration, run `mise run check`, `mise run test`, both platform
smokes on their native hosts, and `mise run verify`. CI remains authoritative
for macOS arm64 and Linux x86_64 compilation and smoke execution. Perform one
manual visual pass at narrow and ordinary window sizes, in windowed and
fullscreen presentation, because automated assertions do not establish panel
legibility or clipping.

## Implementation sequence

1. Add the catalog command, platform defaults, menu item, installed keymap
   snapshot, and component-binding reinstall hook. Prove catalog validation,
   default matching, shortcut discovery, and reload behavior before adding UI.
2. Build the private text field and picker list with focused Unicode,
   composition, editing, selection, and keyboard-navigation tests. Keep both
   free of command and runtime knowledge.
3. Add the pure palette state model for search, selection, argument drafts, and
   invocation construction. Prove ranking, explicit-argument skipping, optional
   defaults, and validation without a native window.
4. Host `CommandPalette` in `WorkspaceView`. Add modal rendering, focus return,
   the `Palette` context, pointer capture, and every terminal input blocker.
   Verify search text cannot enter a PTY before adding runtime pickers.
5. Capture target ancestry and hierarchy on the background executor. Add
   session, workspace, and tab projections with scoped-ID selection and
   duplicate-name labels. Route completed invocations through existing scope
   executors and prove rename plus stale-target behavior.
6. Extract and reuse command availability checks, then audit asynchronous error
   reporting so the originating window owns later failures. Add focused tests
   around each refactored precondition to prevent behavior changes for menus,
   keybindings, and programmatic callers.
7. Add the native palette smokes and wire their build, dispatch, and workflow
   steps into CI. Update README usage plus the command, default-binding, and
   key-context tables. Record in the PR that pane selection is intentionally
   deferred with Move commands to #32, despite #24's acceptance wording. Add
   any concise agent guidance discovered during implementation. Run the full
   validation set and complete the manual visual pass before handoff.

Steps 2 and 3 can proceed independently after step 1 if file ownership is kept
separate. Steps 4 through 7 are ordered because target capture, availability,
and native evidence depend on the hosted workflow.

## Alternatives considered

Keeping the entire palette inside `WorkspaceView` would minimize new modules,
but it would add text editing, search, picker, focus, and argument state to a
file that already owns window lifecycle, tabs, close confirmation, fullscreen,
quake presentation, and drag behavior. The resulting logic would be harder to
test without a native window.

Creating a new GPUI component crate immediately would make dependency rules
explicit and prepare for settings UI. It also adds a workspace package, release
metadata, and a public internal boundary with only one current consumer. Start
with the same ownership rules in private modules and extract them when reuse
makes the package boundary carry real weight.

Using GPUI Kit would provide richer existing components, but it requires moving
from Huterm's official GPUI 0.2.2 line to `gpui-pre` and reconciling root-owned
keybindings, focus, input composition, window behavior, and rendering. That is
a separate migration decision and is not needed to implement #24 correctly.

Resolving target names at execution would make picker drafts smaller, but
duplicate names and concurrent renames would make results ambiguous. The
catalog and Mux already define scoped IDs as the execution contract, so picker
names remain presentation only.

Hiding unavailable commands would shorten the list. It would also make command
discovery depend on transient state and give users no explanation for a missing
action. Keep them visible and disabled.

## Settled decisions

- The palette is a GPUI client workflow over the existing catalog and
  executors.
- Rename proves shared runtime execution in #24. Move commands stay in #32.
- Scoped IDs, not display names, are picker values.
- The invocation target is captured when the palette opens and changes only
  through an explicit picker choice or explicit supplied argument.
- Commands with declared arguments use one argument view in catalog order.
- Unavailable commands remain searchable with a reason.
- Search is deterministic and dependency-free for the current catalog.
- Private Huterm-owned GPUI components land before any component-crate split.
- GPUI Kit adoption and framework migration are separate work.
- Native smoke tests prove input isolation, focus, stale targets, and async
  failure ownership on macOS and Linux.

No unresolved questions block implementation.

[core-commands]: ../../crates/huterm-core/src/commands.rs
[gpui-commands]: ../../crates/huterm-gpui/src/commands.rs
[issue-23]: https://github.com/jimeh/huterm/issues/23
[issue-24]: https://github.com/jimeh/huterm/issues/24
[issue-32]: https://github.com/jimeh/huterm/issues/32
[keymap]: ../../crates/huterm-gpui/src/keymap.rs
[protocol-command]: ../../crates/huterm-protocol/src/command.rs
