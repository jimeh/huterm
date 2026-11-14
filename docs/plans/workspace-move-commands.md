# Workspace move commands

Status: implemented. Closes [issue #32][issue-32]. Follows
[workspace drags](workspace-drags.md) (D, #458), whose drop paths these
commands reuse.

## Request

Finish #32 (coordinator brief, 2026-11-13):

- Keyboard and palette equivalents for every move #458 added by drag: a
  whole tab to another workspace, a workspace to another existing session,
  and workspace reordering.
- #458's known limitations: a pane moved between workspaces of one window
  loses its view; an incoming drag does not edge-scroll the destination's
  workspace list; a narrow window restored with many workspaces opens its
  sheet without revealing the shown row.

Settled before this plan: workspaces do not drop on session rows (the
command covers it), and #458's defaults stay.

## Commands

No default binding outside `Sidebar`, as their siblings have none.

| Command | Scope | Arguments | Behavior |
| --- | --- | --- | --- |
| `move_tab_to_workspace` | Window | `tab` optional, not prompted (active tab); `workspace` required, prompted | Moves the whole tab, layout and terminals intact, to the end of that workspace in any session, as a row drop does. |
| `move_workspace_to_session` | Window | `workspace` optional, not prompted (window's); `session` required, prompted | Moves the workspace to the end of that session, as a drop on another window does, without showing it there. |
| `move_workspace_up` / `move_workspace_down` | Window | `workspace` optional, not prompted (window's) | Moves it one place in its session; unavailable at that end. |
| `sidebar_move_up` / `sidebar_move_down` | Window | none | Bound only while `Sidebar`: `alt-up` / `alt-down` on both platforms. Moves the focused workspace row, or the focused session list row, one place; focus stays on the row, which scrolls into view. Nothing at the list's end. |

Pickers leave out where the item already is: the tab's own workspace, or the
workspace's own session, from the explicit argument when one is given. With
one candidate left, Enter runs the move without a picker, as for any
single-choice slot.

Availability:

- `move_tab_to_workspace`: the structural guard and runtime check; the tab
  is one of the window's shown tabs; another workspace exists, in any
  session. A gone destination is `StaleTarget`; the tab's own workspace is
  unavailable.
- `move_workspace_to_session`: the guard and runtime check; the workspace
  is in the window's session; another session exists. A session's only
  workspace may move, as by drag.
- `move_workspace_up` / `down`: the guard; not at that end of the session.
- A tab drag in progress refuses each command (`check_runtime_available`);
  a row or pane drag is cancelled first.

## Defaults awaiting confirmation

Chosen here, each cheap to change:

- No command activates its destination. The window keeps its workspace
  unless the move took or emptied it; then it follows or closes through its
  emptied close under the existing rules: a pinned workspace stays empty, a
  sibling shows, otherwise the window's session detaches or closes, as after
  a drop. A workspace moved to a session no window shows is reached through
  the session list. The drag's X1 (the destination window shows it) has no
  command counterpart, since a command names a session, not a window.
- `alt-up` / `alt-down` reorder rows in the `Sidebar` context; no alt-arrow
  pattern existed before. Session list rows take them too, since that list
  shares the `Sidebar` keys; reordering sessions reuses `Mux::move_session`.

## Placement

Each new item goes where its siblings already live, with no new pattern:

- Tab context menu: **Move Tab to Workspace…** between Move Tab to New
  Window and Move Tab into Pane…; it opens the palette prompt with the tab
  set, as Move Tab into Pane… does. Disabled with a reason while no other
  workspace exists.
- Workspace row menu: **Move Workspace to Session…** (`Layers`, the session
  icon) while another session exists, then, while the session has another
  workspace, **Move Workspace Up** and **Move Workspace Down** (arrow icons,
  disabled at the ends) before Move Workspace to New Window.
- Session row menu: **Move Session Up** and **Move Session Down** (arrow
  icons, disabled at the ends) after Open in New Window, while there is
  another session, so the session list's keyboard reorder has a visible
  equivalent.
- Palette: the four operation commands by catalog title. The two
  `sidebar_*` commands stay contextual, as their siblings are.
- Settings keybindings: each joins its siblings' group.

## Shape

- `huterm-protocol`: the six ids and catalog specs. `schemas/` regenerated.
- `keymap.rs`: the two `Sidebar` defaults.
- `windows/workspace_moves.rs`: availability and the three window commands,
  reusing:
  - `commit_tab_move` with `Operation::MoveTab { before: None }`: its busy
    flag, failure notice, `reconcile_own`, `settle_moved`, and focus
    restore;
  - `WorkspaceView::move_workspace`, now a method that holds the window busy,
    resumes a queued close, reports failure, and reveals a reordered row.
    Row drops use it too.
  - Both hold Quit and checkpoints until the move settles (`hold_move`): a
    pending spawn, which Quit hosted by any window waits for, and a transfer
    reservation, which checkpoints wait for and which makes a capture already
    under way start again. A tab reorder within its workspace takes neither.
- A test-only `Stage::Move` hook on both workers.
- `workspace_menu.rs`, `tab_menu.rs`, `workspace_panel.rs`,
  `session_control.rs`: rows, picks, and the sidebar keys.
- `palette/mod.rs`: the two picker exclusions (`exclusion`), in both the
  slot editor's domain and `domain_for`.

### Follow-ups

- **Pane views across workspaces.** `adopt_moved_panes` runs after
  `adopt_moved_tabs`, before the per-workspace passes. For each pane the
  projection now holds in a tab of another held workspace, it puts the
  source tab's panes in projected order, takes the view
  (`take_pane_view`, dropping a tab left empty), hides it when it leaves
  the shown workspace, and puts it in the new tab's view, built when the
  window has none (`put_pane_view`). It skips a busy window and views the
  projection has not caught up with, as tab adoption does.
  `adopt_pane_view` is now these two halves. The kept view keeps its
  viewer, scroll position, and terminal state; a selection still clears
  when the pane's new size reflows its terminal, as any resize or output
  clears one (the engine bumps its generation), so a pane moved into a new
  tab of another size loses it, as it would within one workspace.
- **Incoming edge scroll.** `Incoming` gains the `ListScroll` a local
  gesture has. While the pointer is over the destination's drawn workspace
  list, frames scroll it at the edges and retarget the incoming preview.
- **Sheet reveal.** A failing window test confirmed the bug. GPUI fills a
  list's child bounds before applying a scroll request, but on a handle's
  first prepaint its overflow is still the default, so the request is
  dropped rather than deferred. `reveal_workspace_row` now records a
  pending reveal when a list has not laid out, and render repeats it once
  the drawn list has bounds, asking for at most one extra frame.

## Failure paths

| Case | Handling | Test |
| --- | --- | --- |
| Destination workspace or session gone before the command | `StaleTarget` | `tab_moves_refuse_their_own_workspace_a_gone_one_and_a_busy_window`, `workspace_moves_to_sessions_refuse_their_own_session_and_a_gone_one` |
| Destination gone while the worker waits | Mux refuses; notice; nothing moves; busy clears | `a_destination_that_goes_while_a_tab_move_waits_refuses_it` |
| Busy window | Unavailable | both refusal tests |
| Quit or a checkpoint while a move's worker waits | Both wait for the move's hold, whichever window hosts Quit; the capture holds the workspace once, in its new session | `quit_waits_for_a_workspace_move_and_saves_the_workspace_once`, `a_destination_that_goes_while_a_tab_move_waits_refuses_it` |
| A tab move empties the shown workspace | Follows a sibling; a pinned one stays empty | `a_tab_move_that_empties_the_shown_workspace_follows_unless_pinned` |
| A workspace move takes the shown workspace | Every window showing it follows; destination windows hold it hidden | `a_workspace_moved_to_another_session_leaves_every_window_showing_it` |
| A session's only workspace moves | Its emptied window closes, as by drag | `a_sessions_only_workspace_moves_and_its_emptied_window_closes` |
| Reorder at an end | Unavailable; the sidebar key does nothing | `workspaces_move_up_and_down_and_stop_at_the_ends`, `the_sidebar_keys_move_the_focused_row_and_keep_its_focus` |

## Review revisions

A Codex and a Claude plan review ran before implementation finished; each
finding was checked against the code.

- Adopted: allow a lone workspace to move to another session, matching the
  drag; state source recovery separately from destination activation;
  reveal the moved row after a keyboard reorder; hold the window busy during
  a workspace move so Quit waits; a test hook for deterministic
  interleavings; order panes before cross-workspace adoption; the sheet
  diagnosis, corrected from a reproduction; four palette commands, not six;
  schema, configuration, and agent guide updates; repoint
  `undo_focuses_a_pane_whose_view_has_to_install_again` at another session
  so it still covers the install path, with a new test for the kept view.
- Not adopted: reserving destination windows and handing queued closes to
  them, as `dispatch_transfer` does. A command shows nothing in a
  destination window, the Mux move is atomic under its lock, and
  `move_pane_to_new_tab` already moves across sessions this way;
  destination windows reconcile as for any other client's change. A pane
  close queued in the source during a move to another session ends quietly,
  as R1 settled for hidden workspaces. Reusing `dispatch_transfer` would
  also activate the destination window, which a command must not do.
- Not adopted: new native smokes. The window tests drive the keymap
  through real key presses and the menus through their picks; the existing
  overlays smoke walks the tab menu past the new item, which keyboard
  navigation skips while disabled.

## UX review

A Codex computer-use review of a scratch bundle found the moves, reorders,
and their propagation to a second window working, and four issues:

- Moving a split pane to a new tab of another workspace cleared its
  selection. The view is kept (the window tests compare entities); the
  pane's new size reflows the terminal, which clears any selection, as a
  window resize does. Recorded as a limitation, not changed here.
- Picker rows exposed only their label, so two "Workspace 1" destinations in
  different sessions read the same. Fixed: an option's second line is now
  its accessible description, for every picker.
- Session reordering had no visible equivalent of its keys. Fixed with the
  session row menu items above.
- A disabled menu item's reason does not reach native menus, which also
  drop item icons. This predates the change and affects every item; follow
  up.

Showing the `Sidebar` keys beside the row menus' items is a follow-up too:
no row menu shows its local keys today, F2 for Rename included.

## Code review

The PR's dual review (Codex and Claude) found that a move held only its own
window's `busy` flag, which Quit honors only in its host window and
checkpoints never read; `hold_move` now covers both. It also tightened the
pending reveal to the drawn list and added a test for the row menu's
session prompt. A reorder briefly blocks the window's input while it
commits, as a tab move already does; that stays.

The follow-up round found that a workspace row drop could still commit
while the window was busy with another operation, ending that operation's
`busy` and sharing its hold slot; a busy window now takes no workspace row
drop. Two findings stay as accepted residual risk:

- The holds end when the move's completion reconciles. When the move took
  the window's shown workspace, the window then follows through its emptied
  close, whose commit takes the close pipeline's own hold; a checkpoint in
  the gap before that commit saves the window still naming the departed
  workspace. Restore falls back to the session's first workspace, and the
  next checkpoint, which the follow-up triggers, corrects it. A drop on
  another window and any other client's move leave the same gap. Holding
  until the follow-up would tie the move's hold to a close pipeline that
  can end without committing, which risks a Quit that never resumes.
- Pane moves between workspaces (`commit_pane_move`, which #458's row drops
  and Move Pane to New Tab share) still hold only `busy`, as before this
  change. A follow-up can give them `hold_move`.

## Non-goals

- Workspace drops on session rows (decided), session rows across windows.
- A default binding outside `Sidebar`.
- Moving a hidden workspace's tab by command: `tab` names the shown
  workspace's tabs, as for every tab command.
- The slot-preview overlap #458 listed; it predates this and is unchanged.

## Verification

- Unit: catalog and keybinding groups; row menu rows at the ends, with one
  workspace, without another session; the tab menu's disabled row; picker
  exclusions; `step_before` at each position.
- Window tests in `window_tests/workspace_moves.rs` (the table above, plus
  kept tab and pane views, a cross-session tab move, palette and menu
  reorders, and session list keys), `layout_undo.rs` (undo and redo of a
  kept pane view), `workspace_drags.rs` (incoming edge scroll), and
  `workspace_panel.rs` (the first sheet reveal).
- `mise run check`, `mise run test`, and CI's input and overlays smokes.
- UX review through Codex computer use on a scratch bundle: discovery of
  the menu items and palette prompts, and keyboard reordering.

[issue-32]: https://github.com/jimeh/huterm/issues/32
