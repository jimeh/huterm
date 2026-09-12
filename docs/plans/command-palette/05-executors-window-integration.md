# Chunk 5: executors and window integration

Part of [the command palette plan](../command-palette.md). Implements the
executor-side and window-side machinery the palette UI (chunk 6) will sit
on. Requires chunks [1](01-protocol-catalog.md), [2](02-keymap-dispatch.md),
[3](03-config-schema.md), and [4](04-search-ranking.md) merged.

Owner: Codex implementer. Dependencies: chunks 1 to 4. Must land before
chunk 6 starts, because both edit `windows.rs`.

## Objective

Everything the plan asks of executors and `WorkspaceView` that is not the
palette's own presentation: `select_tab` by identity, reset-name commands,
tab activation history and `select_recent_tab`, `copy` availability,
interactive prompting from `run_command`, status-line error routing, the
availability refresh moved out of `render`, snapshot custom names, quake
profile listing, history recording, and ellipsis menu items.

Files: `crates/huterm-core/src/commands.rs`,
`crates/huterm-core/src/mux/lifecycle.rs` (only if the snapshot lacks what
is needed), `crates/huterm-gpui/src/desktop/windows.rs`,
`crates/huterm-gpui/src/desktop.rs` (`install_menus`, `bind_keymap`),
`crates/huterm-gpui/src/desktop/quake_windows.rs` (one read-only accessor),
`crates/huterm-gpui/src/desktop/palette/mod.rs` (only the small hooks listed
below), and the README command table rows for the new commands' behaviour.

## Invariants (AGENTS.md)

- Never acquire the Mux mutex from render, input, or focus callbacks.
  Structural commands serialize through Mux on background workers.
- Runtime rename commands fill omitted targets on the UI thread and resolve
  the session under the Mux lock on the worker. Keep that path for resets.
- Foreground checks belong to the PTY owner. Do not add ps scans.
- Terminal-scope commands dispatch to the captured `TerminalView`, never
  through `Desktop::invoke`.
- Deferred failures report to the originating window; keep the existing
  reporter plumbing.

## Changes

### Core: reset commands

In `huterm-core/src/commands.rs`, `execute` handles `reset_tab_name`,
`reset_workspace_name`, `reset_session_name` by calling the existing
`rename_*` Mux methods with `None`. Missing identity is `MissingArgument`
as for renames. Add a test alongside the rename test: rename then reset
yields `custom_name() == None`; reset on an already-clear name succeeds
(idempotent); a stale target returns `StaleTarget`.

### Snapshot custom names

`HierarchySnapshot` carries `Session`, `Workspace`, and tab records that
already expose `custom_name()`. Confirm the palette-facing projection can
read it; if the tab record inside `Workspace.tabs` lacks the accessor, add
one in `mux.rs` next to `display_name`. No new snapshot fields.

### Window: `select_tab` by identity

In `run_command`, `ids::SELECT_TAB`:

```rust
ids::SELECT_TAB => match (invocation.tab("tab"), invocation.integer("index")) {
    (Some(tab), _) => self.select_by_id(tab, window, cx),
    (None, Some(_)) => self.select_index(select_tab_slot(invocation)?, window, cx),
    (None, None) => unreachable!("validate guarantees one target"), // see below
}
```

Do not use `unreachable!`; `run_command` runs `validate` first (see
"Interactive prompting"), so the last arm returns
`Err(CommandError::MissingArgument { name: "tab" })` defensively.
`select_by_id` checks `check_available(true)`, finds the tab in
`self.tabs`, returns `StaleTarget` when absent, and calls
`select_from_command`. Availability for `select_tab` stays
`check_navigation_available`.

### Window: activation history and `select_recent_tab`

Add `history: Vec<TabId>` to `WorkspaceView` (most recent first, no cap
needed; it is bounded by the tab count). Update it in the one place that
sets `self.active = Some(id)` (currently around `select`), moving `id` to
the front. Prune on tab removal wherever `self.tabs` loses an entry (there
is one removal path; find it by searching for `tabs.remove` / `retain`).

`run_command` handles `ids::SELECT_RECENT_TAB`: the first history entry
that is not the active tab and still exists; when none, return
`Ok(CommandOutcome::Completed)` without change. Availability:
`check_navigation_available` plus `Unavailable("window has one tab")` when
`tabs.len() < 2`.

Expose `pub(super) fn tab_history(&self) -> &[TabId]` for chunk 6.

### Window: `copy` availability

`TerminalView::command_availability` returns
`Unavailable("no selection")` for `ids::COPY` when the view has no
selection. The selection field is the one the `selection` key context
reads (`key_context` around line 1544 of `desktop.rs`). Existing smoke
`copy` steps, if any, must still pass; check `scripts/check-*.ts` for
`"copy"` before changing behaviour and adjust only if a smoke copies with
no selection.

### Window: reset-name availability

In `command_availability`'s Runtime arm, the reset commands are unavailable
with `"tab has no custom name"` (and the workspace/session equivalents)
when the captured target's record in the palette's hierarchy snapshot has
no custom name. The snapshot lives on the palette entity; add
`pub(super) fn custom_name_state(&self, value: &CommandValue) -> Option<bool>`
to `CommandPalette` returning `None` while the snapshot is missing.
Availability returns `Ok(())` while `None` (unknown), so the row is not
falsely dimmed before the snapshot lands; execution re-validates in core.
The refresh point below re-evaluates once the snapshot arrives.

### Window: interactive prompting

Add a private `fn invoke_interactive(&mut self, invocation, window, cx)`
used by the `InvokeWindow` action handler, `InvokePalette` forwarding, and
menu dispatch. It:

1. Runs `validate_supplied` (already done by `CommandAction`, cheap to
   repeat).
2. Looks up the spec and calls `spec.missing_prompted(invocation)`.
3. If non-empty and the command is not `Palette` scoped, calls
   `open_palette_request(Some(invocation), window, cx)` and returns its
   result. If the palette is already open, the existing "focus it" path
   applies and the request is ignored; that is acceptable for now and
   chunk 6 refines it.
4. Otherwise runs `validate` and then the existing `run_command`.

The programmatic entry (`Desktop::invoke` and `run_app_command`) keeps
calling `validate` first; it is the non-interactive path and returns
`MissingArgument`.

`open_palette_request` already accepts a request. Add a `requested: bool`
to `PaletteTarget` or the palette constructor so chunk 6 can implement
"back closes" for requested stages; for now just store it.

### Window: `InvokePalette` handling

`WorkspaceView` registers `on_action` for `InvokePalette`: if
`self.palette` is `Some`, forward with
`palette.update(cx, |palette, cx| palette.run(&action.0, window, cx))`;
otherwise ignore (the binding is conditional on `Palette`, so this only
happens if a user forces it). Add `pub(super) fn run(&mut self,
invocation, window, cx) -> Result<CommandOutcome, CommandError>` on
`CommandPalette` that matches ids to the stub methods chunk 2 left; chunk 6
replaces the bodies.

### Window: status-line error routing

In `handle_palette_event`, `PaletteEvent::Execute`: after the palette is
removed and focus restored, a synchronous `Err` from dispatch writes
`self.status = Some(error.to_string())` and notifies; the branch that
re-attached the palette and called `set_error` is deleted. Pre-dispatch
validation and availability failures keep returning the error to the
palette (that branch stays).

### Window: availability refresh out of `render`

Remove the block in `render` that recomputes `palette_availability` and
calls `set_availability`/`set_keymap`. Instead:

- compute availability and pass the keymap when the palette opens (already
  done) and when the hierarchy snapshot arrives (already done);
- add `fn refresh_palette(&mut self, cx)` called from the window refresh
  pump (the per-window tab-event drain) whenever `tabs`, `active`, `busy`,
  `close.confirmation`, `reorder`, or quake presentation changed since the
  last refresh, and from the config-reload path so `set_keymap` still runs.
  Track "changed" with a small hash or a dirty flag set by the mutation
  sites; a dirty flag set in `select`, `new_tab` completion, tab removal,
  `busy` transitions, confirmation open/close, and reload is sufficient.

### Window: history recording

Add `recent: RecentCommands` (chunk 4 type) to `WorkspaceView` and
`frequency: CommandFrequency` to the `Desktop` global. Record in
`handle_palette_event` when dispatch returns `Ok(_)`, before the palette is
dropped. Expose both to chunk 6 through
`pub(super) fn history_view(&self, cx: &App) -> HistoryView<'_>`; the
lifetime needs both borrows, so return the pair of references instead if
that is simpler.

### Quake profile listing

Add to `quake_windows`:

```rust
/// Configured profiles with their live window state, sorted by name.
pub(super) fn profile_rows(cx: &App) -> Vec<ProfileRow>;
pub(super) struct ProfileRow {
    pub(super) name: String,
    pub(super) geometry: String,   // "top · 100% × 50%" or "fullscreen"
    pub(super) state: ProfileState,
}
pub(super) enum ProfileState { NotSummoned, Hidden { tabs: usize }, Visible }
```

Built from `Desktop.config.quake.profiles` and `Registry.windows`. Read
only; no Mux. Tab counts come from the associated `WorkspaceView`'s
`tabs.len()` via the window handle. Chunk 6 consumes this.

### Menus

In `install_menus`, add `item(ids::SELECT_TAB)` under Window as
"Select Tab…" and `item(ids::RENAME_TAB)` under Window as "Rename Tab…",
plus `item(ids::SELECT_RECENT_TAB)`. Menu labels come from the catalog
title; add an ellipsis when `spec.missing_prompted(&invocation)` is
non-empty in the `menu_item` helper (`commands.rs`), so any prompting
command gets it automatically.

### README

In the command table: `select_tab` row becomes "`index` (1 to 9; 9 selects
the last tab) or `tab`; exactly one. Without either, prompts for a tab."
Add rows for `select_recent_tab` and the three reset commands. Add the
`copy` "requires a selection" note.

## Tests

- Core: `reset_clears_custom_names_and_is_idempotent` as described.
- Window unit tests (there are existing `WorkspaceView` tests in
  `windows.rs` that run without a native window; follow their pattern):
  - `activation_history_tracks_selection_and_prunes_closed_tabs`;
  - `select_recent_tab_toggles_between_two_tabs_and_is_a_no_op_alone`;
  - `interactive_invocation_without_required_arguments_opens_the_palette`
    (bare `select_tab` results in `self.palette.is_some()` with a requested
    stage; `toggle_quake` bare does not open the palette);
  - `programmatic_invocation_without_required_arguments_fails`
    (`Desktop::invoke` path returns `MissingArgument`);
  - `synchronous_dispatch_failure_lands_in_status_not_palette`;
  - `copy_is_unavailable_without_selection`.
- `profile_rows` test: two profiles, one summoned and hidden with two
  tabs, correct names, geometry strings, and states.
- `menu_item` adds an ellipsis for `select_tab` and `rename_tab` and not
  for `new_tab`.

## Verification

```sh
cargo test --locked -p huterm-core commands
cargo test --locked -p huterm-gpui
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --check
mise run check
mise run smoke:macos-palette   # on macOS; Linux hosts run smoke:linux-palette
```

The existing palette smoke must still pass. It exercises the first
iteration's argument view, which chunk 6 replaces; if a step fails solely
because a stub from chunk 2 changed dispatch, fix the stub, not the smoke.

## Self-review checklist

- No Mux lock in `render`, `key_context`, or any input handler.
- `render` no longer mutates the palette entity.
- The palette never re-attaches itself after a dispatch failure.
- Programmatic invocations still get `MissingArgument`; only interactive
  ones prompt.
- `unreachable!` and `expect` were not introduced outside tests.

Report: files changed, test names and results, smoke result, and deviations
with reasons. Call out any place where the plan's assumptions about
`windows.rs` structure were wrong and how you resolved it.
