# Retained workspace views

Status: implemented. Refs [issue #27][issue-27]. The maintainer chose
retained views on 2026-11-09 (R1), after [workspace panels](workspace-panels.md)
shipped switching as an install. A later change (R2) unifies the drag code,
and D adds drops on sidebar workspace rows, spring-loaded rows, and workspace
tear-off; neither is part of R1.

## Decision

A desktop window keeps terminal views for every workspace of its attached
session, not only the one it shows. Showing another workspace of the session
is then a visibility change, as showing another tab is: no install, no
settle, no `pending_spawns` hold, no checkpoint deferral. A session switch
still replaces every view.

Hidden views were already cheap: a view that is not visible takes no
snapshots and requests no frames (`SnapshotPacer::begin`). Their activity
drains still run.

## Data structure

- `WorkspaceView::tabs` keeps the shown workspace's views, aligned with the
  window model's entries, so the tab bar, closes, drags, and accessibility
  keep their meaning.
- `WorkspaceView::hidden` holds every other workspace's views by workspace.
  They have no model entries. A hidden workspace's active tab, history,
  focused panes, and layout history stay in its `NavigationMemory`, which is
  already saved, restored, and pruned.
- `with_hidden` puts one hidden workspace's views in `tabs` and names the
  workspace in `scope`, so the view-level operations that reconcile and
  installs use apply to it. Model writes (`push_tab_view`,
  `drop_tab_views`, `apply_tab_view_order`) happen only for the shown
  workspace.
- `PendingInstalls` records each in-flight tab's workspace, so a restore or
  a session switch settles once the shown workspace's installs are idle and
  hidden ones finish in the background, and a failed install waits for a
  batch that touches its own workspace.

## Reconcile

`reconcile_window` stays generic. A window reconciles its shown workspace,
then each hidden one through `with_hidden`: pane order, pane adoption,
removals, installs, and replacements of revoked viewers. Workspaces that
leave the attached session drop their views; ones that join get views.
`windows_to_visit` also visits windows whose session holds a touched
workspace.

Before those passes, `adopt_moved_tabs` moves the view of a tab that moved
between two workspaces the window holds into its new workspace, keeping its
viewer, scroll, and selection, as pane adoption does within a tab. A drop of
a tab onto a hidden workspace row (D) then needs no install, and
spring-loading a row is as cheap as spring-loading a tab.

Install completions route each tab to the workspace the projection places
it in: the shown one, a hidden one, or nowhere.

## Switch

`show_workspace` runs in one update: it marks the left workspace's bells
seen, files its navigation memory (`switch_to`), swaps the views
(`swap_shown`), gives the model the target's entries with its remembered
active tab, history, and pane focus, and selects the active tab or shows the
empty state. `switch_workspace` and `follow_left_workspace` (after
LeaveWorkspace or an own-workspace close) use it; the follow-up releases its
close hold after the window names its new workspace. New Workspace keeps its
worker and phases, and its install keeps the left workspace's views hidden.

The one asynchronous remainder is a target whose tab views are still
installing, such as right after the window attached: the switch shows what
it holds, and `Arriving` applies the remembered focus as the rest land.

## Hidden workspaces behave like hidden tabs

Settled with the maintainer on 2026-11-09:

- Program notifications are delivered. Show Tab switches to the workspace
  first. This closes #449's gap for every session a window is attached to;
  a session no window shows still has no viewer.
- Bells mark the tab, ring the audible bell, and ask for attention while
  the window is inactive, without a visual flash. The row's bell follows the
  per-window baselines as before; the shown workspace's row never shows one.
- Failure notices show.
- `close_on_exit` closes the exited tab through the window's exit-close.
  A user's close never reaches a hidden pane.
- A hidden workspace that such an exit-close empties follows the shown
  workspace's rule: it closes unless pinned, decided under the Mux lock by
  `Mux::close_emptied_workspace`, which closes only an empty, unpinned
  member of the attachment's session that has a sibling. The coordinator
  chose this default; the maintainer has not confirmed it.

Hidden views publish no accessibility tree, never take focus, and keep their
geometry until shown: a window resize does not resize hidden workspaces'
terminals, which resize when shown, as before. Configuration reloads reach
them.

## Interleavings

| Row | Interleaving | Expected | Test |
| --- | --- | --- | --- |
| R1-1 | Switch away and back | Same views, no subscription, completes in the update; Quit after it waits for nothing | `a_workspace_switch_completes_at_once_and_quit_saves_both_workspaces`, `switching_workspaces_keeps_each_workspaces_shells_navigation_and_undo` |
| R1-2 | Switch while the target's tabs install | Shows what it holds; remembered active tab applies on arrival; no empty state | `a_switch_to_a_workspace_still_installing_applies_its_memory_as_tabs_arrive` |
| R1-3 | Switch while busy, confirming, arranging, or during New Workspace | Refused | `workspace_switches_respect_the_structural_guard` |
| R1-4 | Tab closed elsewhere in a hidden workspace | Hidden view drops; no emptied close | `hidden_views_follow_closes_splits_and_moves_made_elsewhere` |
| R1-5 | Hidden workspace closed elsewhere | Its views drop | same |
| R1-6 | Pane split elsewhere in a hidden tab | Pane view installs hidden; focus stays | same |
| R1-7 | Tab moved between held workspaces | View kept; joins the shown workspace inactive; leaving it hands over the active tab | same |
| R1-8 | Hidden workspace moved to another session | Its views drop; selecting it is refused | `a_hidden_workspace_moved_to_another_session_drops_its_views` |
| R1-9 | Cross-window moves and tear-off | Unchanged; adoption keeps a retained view | existing `cross_window` tests |
| R1-10 | Session switch | Every view replaced; target's workspaces install | `a_session_switch_replaces_every_workspaces_views` |
| R1-11 | Leave or own-workspace close, then Quit or checkpoint | Hold ends after the follow-up names the new workspace | `quit_waits_for_a_leave_workspace_follow_up`, `a_checkpoint_waits_for_an_own_workspace_close`, `a_leave_workspace_commits_while_a_quit_confirms` |
| R1-12 | Restore | Every workspace installs; a switch applies its restored memory with no install | `a_restored_window_holds_every_workspace_and_switches_to_its_memory` |
| R1-13 | Checkpoint right after a switch or follow-up | Saves the new workspace | `a_checkpoint_right_after_a_workspace_switch_saves_the_new_workspace` |
| R1-14 | Follow-up target emptied while the leave commits | Follows past it | `an_l1_follow_up_passes_over_a_remembered_workspace_that_emptied` |
| R1-15 | User close of a pane moved into a hidden workspace | Ends quietly | `a_pane_close_whose_pane_leaves_the_window_ends_quietly_and_keeps_the_queue` |
| R1-16 | Bell in a hidden workspace | Tab and row marked, no flash, seen on show | `a_hidden_workspaces_bell_marks_its_tab_and_row_without_a_flash` |
| R1-17 | Notification in a hidden workspace | Notice with Show Tab, which switches | `a_hidden_workspaces_notification_offers_show_tab_which_switches_to_it` |
| R1-18 | Exit in a hidden workspace | Tab closes; emptied unpinned workspace closes; pinned stays; window stays | `an_exit_in_a_hidden_workspace_closes_its_tab_and_an_emptied_workspace` |

## Measurement

See the pull request for the numbers: a release `restore_smoke` build of
the base and of this change, relaunched five times on a saved session of 10
workspaces with 5 tabs each, measuring time to restore, time until every
view installed, resident memory after settling and after 10 seconds idle,
and switch latency over two laps of every workspace.

[issue-27]: https://github.com/jimeh/huterm/issues/27
