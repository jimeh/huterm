# Workspace drags

Status: implemented. Refs [issue #27][issue-27]. Third of three stacked
changes: [retained workspace views](retained-workspace-views.md) (R1),
[unified drags](unified-drags.md) (R2), then this one (D). Mockups:
[workspace drags][mockup], built on [sidebar mockup v3][v3] and the #401
picks.

## Request

The maintainer asked on 2026-11-09:

- Tabs and panes drop on sidebar workspace rows, from this window or
  another, and move into that workspace.
- A row spring-loads its workspace as an inactive tab spring-loads its tab.
- A dragged workspace or session row shows a cue under the pointer, as a
  dragged tab does.
- A workspace drags out into its own window, or onto another window to
  move into that window's session.

## Decisions

Picked from the mockups (H1, RC1, W1, M1, X1) and the coordinator's
defaults; each awaits the maintainer's confirmation.

| Topic | Decision |
| --- | --- |
| Row drop look | H1: the row takes the drop region the pane edges use, accent at 22% with a 2 pt accent border, also while it spring-loads. |
| Row drag cue | RC1: the shared title chip with the row's name, 12 pt from the pointer; the row stays dimmed and the line marks its slot. |
| Tear-off cue | W1: S1 on the source row (contents at 45%, dashed outline, centred "↗ New window" pill). macOS with the preview on shows M1, titled with the workspace's name. |
| Drop on another window | X1: over its workspace list a line marks the slot; elsewhere the content beside its sidebar takes the drop region and the workspace goes last. The destination then shows it. |
| Follow | A tab or pane dropped on a row does not change the shown workspace; spring-loading follows. |
| Own rows | The dragged item's own workspace's row takes no drop and does not spring. The shown row of another workspace takes a drop but does not spring. |
| Lone workspace | The session's only workspace does not tear off (no cue, the dimmed chip on macOS). It can still move onto another window, whose emptied source then closes, as a lone tab does. |
| Corner switcher | Takes no drop and does not spring open. |
| Session rows | Take no workspace drop; session row drags get the chip only, never cross windows. |
| Command | `move_workspace_to_new_window` (optional `workspace`) in the row menu and the palette shares the tear-off path, unavailable for a lone workspace. |
| Emptied origin | A move that empties a hidden workspace closes it unless pinned or its session's last, through `Mux::close_emptied_workspace`, as an exit-close does (R1). |

## Shape

- `windows/drag.rs`: `DropTarget::Workspace { workspace, spring }` and
  `DropTarget::Window`; `SpringKey::Workspace`; `Hover` and `SpringBack`
  hold `Shown` (workspace and tab) pairs; `operation` maps a tab or pane on
  a row to `MoveTab` or a `NewTab` move at its end, a tab slot or title row
  in another workspace to `MoveTab`, and a workspace slot or window to
  `MoveWorkspace` with its destination session.
- `drag/gesture.rs`: rows resolve for tabs and panes; a gesture sprung
  away from its workspace stays valid while its views are held and resolves
  with another window's rules; workspace rows route through the
  cross-window sampler; the spring path switches without cancelling.
- `cross_window.rs`: `Payload` carries a workspace drag (its panes,
  optional focus, name); `tear_off_workspace` and `put_back`.
- `huterm-core`: `Mux::move_workspace_to_new_session` creates a session
  around the moved workspace in one step, refusing without creating one.
- Drawing: the row chip (`drag/overlay.rs`), H1 and W1 on rows
  (`sidebar_rows.rs`), the X1 region and row line for an incoming
  workspace.

## Review revisions

A Codex plan review before implementation raised eight points; each was
checked against the code:

- Tear-off moves first, unlike a tab tear-off, which can open its window on
  an empty workspace first; a window needs a workspace to show, and a
  placeholder would join the new session. Recovery is non-destructive
  instead: `put_back` returns the workspace to its slot, else last, and
  closes the new session only once empty; an attach failure rolls back under
  the same lock. The source holds a transfer reservation from the start, so
  checkpoints wait. A Quit capture loses nothing in any order, since
  `capture_runtime` keeps every session that holds a tab or a pinned
  workspace.
- A cross-window drop on a hidden row keeps the destination reserved until
  its panes install hidden. A close the user queued for a pane that then
  lands in a hidden workspace ends quietly, as R1-15 settled for any close
  of a pane moved into a hidden workspace.
- Spring-loading uses a dedicated guarded path that records the expected
  workspace before switching and never takes focus in a destination.
- The cross-window payload of a gesture sprung away from its workspace
  reads the held views and the projection.
- The shown row takes drops; only spring-loading skips it.
- An empty pinned workspace drags and tears off (no focus pane).
- A tab or pane over the workspace list scrolls it at the edges, as a row
  does. Native smokes are not extended; see the PR's gaps.

## Interleavings

Tests are in `window_tests/workspace_drags.rs` unless noted.

| Row | Interleaving | Expected | Test |
| --- | --- | --- | --- |
| D-1 | Quit while a workspace tear-off's worker runs | Quit waits; the capture holds the workspace once | `a_workspace_tear_off_interrupted_by_quit_saves_the_workspace_once` |
| D-2 | Checkpoint during a tear-off, while its window installs | Waits on the reservation until the tabs show | `a_workspace_tear_off_reserves_its_window_until_its_tabs_show` |
| D-3 | Window close during a spring-loaded drag | The drag ends; nothing moves | `closing_the_window_ends_a_spring_loaded_drag` |
| D-4 | Destination busy or closing at release | Refused; nothing moves | `a_closing_window_takes_no_workspace_drop` |
| D-5 | Session switch during a spring-loaded drag | The drag ends; nothing moves | `a_session_switch_ends_a_spring_loaded_drag` |
| D-6 | The sprung workspace closes elsewhere | The drag ends; the window follows; nothing moves | `closing_the_sprung_workspace_ends_the_drag` |
| D-7 | The target row's workspace closes before release | No drop, no notice | `a_row_that_goes_before_release_takes_no_drop` |
| D-8 | The dragged tab goes (exit-close) after a row spring | The drag ends and springs back | `an_exit_of_the_dragged_tab_ends_a_spring_loaded_drag` |
| D-9 | The dragged workspace goes before an outside release | No tear-off, no notice; the cross-window record ends | `a_workspace_that_goes_before_release_does_not_tear_off` |
| D-10 | A sprung drop empties its hidden origin | It closes unless pinned | `a_spring_loaded_drop_closes_the_emptied_origin_unless_pinned` |
| D-11 | A row drop empties the shown workspace | The window follows through its emptied close | `a_row_drop_that_empties_the_shown_workspace_follows` |
| D-12 | A workspace dropped on another window while two source windows show it | Both follow; the destination shows it | `a_workspace_dropped_on_another_window_moves_and_shows_there` |
| D-13 | Spring deadline while an arrangement runs | No switch; the drag goes on | `a_busy_window_does_not_spring_a_row` |
| D-14 | Opening the tear-off window fails | Back in its slot; one session fewer; notice | `a_failed_workspace_tear_off_puts_the_workspace_back` |
| D-15 | Escape, a release without a target, or blur after a row spring | Shows the origin workspace and tab | `a_cancelled_drag_springs_back_across_workspaces` |
| D-16 | A cross-window drop on a hidden row while installs wait | The destination stays reserved until the view installs hidden | `a_tab_dropped_on_another_windows_row_moves_there_without_switching` |
| D-17 | A tab sprung away, then dragged to another window | Its payload still carries the tab | `a_tab_sprung_into_another_workspace_still_leaves_for_another_window` |

## Verification

- Unit: the operation table rows (`drag.rs`), `Hover` spring-back across
  workspaces, the row menu, and core `move_workspace_to_new_session`
  (moves with live terminals, events, revoked viewers, refusals and the
  item limit that create nothing).
- Window tests: the rows above, plus local row drops for tabs and panes,
  springing into a bar, the row chip, a workspace over another window's
  list, tear-off by drag and by command, a pinned empty tear-off, and edge
  scrolling under a tab.
- `mise run check`, `mise run test`, `smoke:macos-input`, and
  `smoke:linux-input` (Docker). A UX review through computer use on a
  scratch bundle compares the running app with the mockup.

## Non-goals

- Drops on the corner workspace switcher, workspace drops on session rows,
  session rows across windows.
- A progress cue for spring-loading; tabs have none either.
- Adopting a pane's view across workspaces of one window; a moved pane
  reinstalls there, as across windows.

[issue-27]: https://github.com/jimeh/huterm/issues/27
[mockup]: https://plans.jimeh.dev/axkjsbrns72kivld37iujyoezq/huterm-workspace-drag-mockup.html
[v3]: https://plans.jimeh.dev/wje7hjohtru35mwxtxxuetqhdi/huterm-sidebar-mockup-v3.html
