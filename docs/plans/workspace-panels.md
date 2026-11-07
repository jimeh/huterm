# Workspace panels

Status: in progress. Plans [issue #27][issue-27] (parent [#11][issue-11]).
The maintainer picked the mockups and settled the open questions on
2026-11-07; see [Settled decisions](#settled-decisions). PR A is
implemented; see [Deviations](#deviations). PR B and PR C remain.

A window can show any workspace of its session, switch between them from
a panel at any window edge or a dropdown in the tab bar, and create,
rename, clear the name of, and close workspaces. People who never create a
second workspace see no workspace UI. Builds on [#26][issue-26]
([session switching](session-switching.md)), whose switch phases,
navigation memory, session control, and interleaving rules this plan
reuses.

## Current state

Observed on `main` at `64f082c6`:

- Core has `create_workspace` (appends, optional name), `rename_workspace`
  (`None` clears), `move_workspace`, and `close_workspace`
  (`crates/huterm-core/src/mux.rs`). `close_workspace` asks no consent,
  removes the workspace and emits `WorkspaceClosed` before terminal
  teardown (which can fail), and can leave a session with no workspace.
  `CloseRequest` has no workspace variant. Automatic names are
  `Workspace N`, numbered across the runtime by `session_numbering`.
- The catalog has `rename_workspace` (Runtime scope, optional prompted
  `workspace`) and `ArgumentKind::Workspace`; nothing creates, selects, or
  closes a workspace, and no production path makes a second one.
- A window shows one workspace: `WindowRecord::workspace`. The projection
  installs and removes views by that workspace and its session
  (`projection.rs` `installs`, `removals`), so changing the record and
  reconciling with removals allowed retargets a window. No Mux call is
  involved; core attachments know only their session.
- `SessionMemory` is keyed by session and holds one workspace; a
  same-session change saves nothing. The saved `remembered` list
  (`huterm-state` `SessionNavigation`) holds session and workspace per
  entry, capped at `MAX_SESSIONS` (256) entries and validated entry by
  entry, so duplicate sessions are accepted. Restore's
  `scoped_navigation` substitutes the session's first workspace for a
  missing one, for remembered entries too.
- An emptied window asks core for `EmptiedWindow`.
  `resolve_emptied_window` refuses with `ViewNotEmpty` while its workspace
  holds tabs, detaches when the session is shared or another workspace
  holds tabs, and otherwise closes the session. So closing a workspace's
  last tab today closes the window. `reconcile_window` marks a window
  emptied only when it removed views; `note_emptied` otherwise notices a
  vanished session, not a vanished workspace. `settle_switch` owes an
  emptied close when a switch lands on nothing.
- `ChromeLayout::build` partitions the window into title row, tab bar, and
  terminal; every PTY size, mouse mapping, and scrollbar derives from its
  `terminal` rect through `terminal_geometry`. `window_frame` and
  `WindowFrame::resolve` grant the Linux client frame and drawn buttons
  only when the resolved tab position is `Titlebar`. `resize_sidebar`, the
  fullscreen reveal (one `reveal` state, hover limited to `layout.tabs`),
  and the notch shelf (`select_notch_shelf`, which also disables
  auto-hide) assume one bar on the window edge. `TabStrip` reserves
  trailing room but has no leading slot. `SessionControlPlacement` holds
  one host, and the control writes one bounds cell that the menu anchors
  to.
- A terminal resize bumps the engine generation
  (`engine/ghostty.rs` `resize`), and a view clears a selection from an
  older generation (`desktop.rs`, `update.invalidated`). Any resize that
  changes the grid, including a window resize, clears the selection today.
- Each terminal's registry publishes a batched `TerminalStatus` (lifecycle,
  lifetime `bells`, foreground process, failures) readable without a view;
  its wakes belong to viewer slots, so nothing notifies a client about a
  terminal it does not view.
- All-window redraws follow name or attachment changes
  (`windows.rs` around 2087); a tab opening or closing visits only windows
  showing that workspace.

## Settled decisions

Picked on 2026-11-07 from the mockups (`/private/tmp/huterm-27-mockups/`,
PNGs in `shots/`; the PRs attach the picked ones):

1. Edges: left, top, right, and bottom, default left (E1). The workspace
   panel is outermost on a shared edge. A top bar takes the title row and
   `titlebar` tabs drop to a top bar beneath it (N4a). An auto-hidden
   fullscreen tab bar hides a horizontal workspace bar with it; a side
   panel never auto-hides.
2. Styles: P1, a list with a "Workspaces" header holding New Workspace
   and collapse; H1, chips for a top or bottom bar. P4 (tree) is a
   possible follow-up.
3. Collapsed: C1, the dropdown leads the tab bar; M1, its menu is built
   like the session menu; X1, the panel's collapse button and the menu's
   Show Workspace Panel toggle it.
4. Rows: R3, icon, name (dim when automatic), tab count, and one rolled-up
   status dot. CR1, New Workspace creates "Workspace N" with a shell at
   once. RN1, rename in the palette prompt. EM1, an empty workspace shows
   an empty state. L1, closing a workspace's last tab closes the workspace
   and the window moves to the workspace it showed before. SW1, a side
   panel that leaves too little room shows as the dropdown.
5. `[window] session_button_placement`: `title_bar` (default), `sidebar`,
   or `both`. F1: `sidebar` falls back to the title bar while the window
   shows no panel or bar.
6. `[workspaces] show`: `auto` (default; workspace UI only while the
   session has two or more workspaces), `always`, or `never`. Q1: the
   session menu gains a New Workspace row.
7. Settings: G1, a "Sessions" section after Windows.
8. No default key bindings. Collapse state and panel width are saved per
   window (additive fields); `position` is global. Quake windows follow
   the same rules. New Window is unchanged; opening a workspace in a new
   window is a follow-up.
9. After the plan review: a panel change behaves like a window resize,
   so the selection survives only when the grid size is unchanged. This
   is a known limit against #27's acceptance criterion 2; the reflow case
   extends [#373][issue-373].
10. After the plan review: three PRs from this thread, A, B, then C
    (below). C starts after the #81 PR merges. B builds on its own branch
    from `main`, not stacked on A unless it must be.
11. After the plan review: a horizontal workspace bar auto-hides with the
    tab bar in fullscreen only when both share an edge; otherwise it
    stays shown.

Config:

```toml
[window]
session_button = true
session_button_placement = "title_bar" # title_bar, sidebar, or both

[workspaces]
show = "auto"      # auto, always, or never
position = "left"  # left, top, right, or bottom
collapsed = false  # new windows show the dropdown instead of the panel
```

## Delivery

Three PRs from this thread, in order (settled decision 10):

- **A. Workspace switching and lifecycle.** Core close and leave-workspace
  outcomes, the workspace commands, per-workspace navigation memory, the
  switch, L1, the empty state, restore, and the session menu's New
  Workspace row. Reachable through the palette and the session menu.
- **B. Workspace panel, bar, and dropdown.** Config, the Sessions
  settings section, chrome geometry, the panel, bar, dropdown, and
  workspace menu, the sidebar session-control host, and per-window
  collapse and width. Rows show name and tab count (R1).
- **C. Workspace status dot (R3).** A core status subscription that needs
  no viewer, per-window bell acknowledgement, and the dot on rows, chips,
  and menu rows.

A lands lifecycle risk with focused review before B adds the largest
geometry change since the tab sidebar; C adds a core notification path
that deserves its own review. A is useful alone: with `auto` there is no
panel to miss, and the palette reaches everything.

## PR A: switching and lifecycle

### Core

- `CloseRequest::Workspace(WorkspaceId)` and `CloseEffect::Workspace`:
  assessment covers that workspace's terminals; commit runs
  `close_workspace`. Refused with a new `MuxError::LastWorkspace` when it
  is the session's only workspace (Close Session covers that).
- `resolve_emptied_window` gains one outcome, `CloseEffect::LeaveWorkspace
  { attachment, workspace }`, decided under the lock in this order, every
  check scoped to the attachment's session:
  1. The workspace belongs to that session and holds tabs:
     `ViewNotEmpty`, as today.
  2. Another workspace of that session holds tabs, or the workspace no
     longer belongs to that session (closed, or moved to another session)
     while the session keeps a workspace: `LeaveWorkspace`.
  3. Otherwise today's `Detach` or `Session`.

  `LeaveWorkspace` covers no terminal (`close_terminal_ids` is empty), so
  it needs no consent, and its ticket keeps the usual revision check.
  Committing it closes the workspace only if it still belongs to that
  session and holds no tab; a missing or moved-away workspace is left
  alone. It keeps the attachment. This makes L1 core's decision, at the
  same freshness check as every close, so no emptied detach can win over
  it.
- New Workspace runs `create_workspace` and `open_tab` under one lock on
  a worker and closes the workspace when the shell fails, as `new_session`
  does for sessions.

### Commands

| ID                     | Scope   | Arguments                                        | Effect                                                                                    |
| ---------------------- | ------- | ------------------------------------------------ | ----------------------------------------------------------------------------------------- |
| `new_workspace`        | Window  | `name` (optional, no prompt)                     | Creates a workspace with one shell in the window's session and switches the window to it. |
| `select_workspace`     | Window  | `workspace` (required, prompt)                   | Switches the window to that workspace of its session.                                     |
| `next_workspace`       | Window  | none                                             | The next workspace in session order, wrapping.                                            |
| `previous_workspace`   | Window  | none                                             | The previous one, wrapping.                                                               |
| `rename_workspace`     | Runtime | existing; `workspace` defaults to the window's   | Unchanged apart from the default.                                                         |
| `clear_workspace_name` | Runtime | `workspace` (optional, defaults to the window's) | `rename_workspace` with `None`; unavailable without a custom name.                        |
| `close_workspace`      | Runtime | `workspace` (optional, defaults to the window's) | Assesses and closes with consent; unavailable for a session's only workspace.             |

`select_workspace`'s picker lists only the window's session's
workspaces, by display name; a stale or foreign target is refused with
"Workspace no longer exists". Next and Previous are unavailable with one
workspace. No default bindings (settled decision 8).

### Navigation memory per workspace

`SessionMemory` becomes `NavigationMemory { session, workspace, active,
history, panes, layout_history }`, one per workspace the window showed
and left, most recent first. A session switch picks the target session's
most recent entry (today's `resolve_workspace` rule otherwise). Pruning
drops entries whose workspace closed or moved to another session. Undo
history follows the workspace, so Undo never reaches a workspace the
window no longer shows.

Saved: one `SessionNavigation` per entry, so a session may appear more
than once; the 256-entry cap stays, dropping the oldest, so an older build
still accepts the file. Restore scopes remembered entries strictly: an
entry whose workspace is missing or foreign is dropped, entries are
deduplicated by workspace (most recent wins), and only the shown
navigation keeps today's first-workspace fallback. Memory applies to the
workspace actually shown.

### Workspace switch

The #26 phases without the worker step, since nothing changes in core:

1. **Guard.** `check_switch_available` (busy, closing, quitting, a close
   confirmation, a cross-window reservation, restore, teardown); drags are
   cancelled first. A no-op for the shown workspace. The target must
   belong to the window's session.
2. **Install.** Store the left workspace's memory, set
   `record.workspace`, reconcile with removals allowed and the emptied
   edge suppressed, as `install_session` does.
3. **Settle.** `apply_reopen` applies the remembered tab, history, and
   panes once installs are idle (first tab without memory). The switch
   holds `pending_spawns` and defers checkpoints from install to settle,
   as every #26 switch does.

`install_session` and `settle_switch` gain a target that is either a
session (with an attachment) or a workspace of the shown session; one
code path. `settle_switch` owes an emptied close only when the session or
the workspace is gone, not for an empty workspace the user selected.

### L1: a workspace that empties or goes

Nothing new decides at the emptied edge; the existing emptied-close
pipeline carries L1:

1. **Owed.** `note_emptied` owes an emptied close when reconcile removed
   the last view, the window's own close removed its last tab, the session
   is gone (all as today), and newly when the window's workspace no
   longer belongs to its session, which needs no removed view (a window
   showing EM1 whose workspace another window closed).
2. **Requested** from `next_request` at today's idle boundary: after
   queued user closes and exit-closes, never during a confirmation, in a
   closing window, or in teardown. A Window or Application close absorbs
   it, as today.
3. **Assessed and committed** by core as above. The desktop dispatches
   the completion by the committed effect: only `Detach` and `Session`
   remove the window, as every emptied completion does today.
   - **Quit conversion.** The pre-assessment emptied conversion in
     `resume_close` goes: an emptied close is always assessed first.
     `close_becomes_quit` converts an emptied close only for a `Detach`
     or `Session` assessment, and only after its ticket passes the
     freshness check under the Mux lock (a new check-only call beside
     `validate_close_at`), so a sibling that gained a tab sends it back
     to assessment instead of quitting. `LeaveWorkspace` never converts:
     a Linux last window that only leaves a workspace stays open.
   - **Departing windows.** `note_close_commit` sets `close_committing`
     only for effects that remove the window, so another window's close
     never counts a window that is only leaving a workspace as gone
     (`last_window_strands`).
   - **During a Quit.** A `LeaveWorkspace` that reaches its commit while
     a Quit is underway commits: it stops nothing, and its hold makes the
     Quit wait for the follow-up (Q4).
4. **Follow-up.** After a `LeaveWorkspace` commit the window waits for the
   projection to reach the commit's sequence, then switches to its target
   on its existing attachment (not the attachment-creating session
   follower): navigation memory order, then session order, the first
   workspace with tabs, else the first workspace (EM1). The commit takes
   a `pending_spawns` hold and defers checkpoints until the follow-up
   settles, as an own-session Close Session does (Q4, Q7). A stale commit
   releases its hold and assesses again (today's retry); it changed
   nothing, so nothing is left half done. The follow-up keeps its
   recovery intent until settle: a target chosen because it held tabs
   that is empty or gone at settle owes the emptied close again, while a
   deliberately chosen empty fallback stays as EM1.

A Quake stash whose workspace closed resolves on its next summon as a
session switch does, checking the workspace and not only the session.

### Close Workspace

`CloseTarget::Workspace` through the existing pipeline, consent as for
any close, merging with no other target. A Quit cancels an own-workspace
close that has not started committing (Q2).

Closing the window's own workspace takes the `pending_spawns` hold and
defers checkpoints from the start of its commit, as an own-session Close
Session does. After the commit it follows directly, with no second
close: the session keeps a workspace (core refuses `LastWorkspace`), so
the answer is always to leave. It switches on its existing attachment to
the step 4 target, keeping the hold until that settles. A commit that
removed the workspace but reported a teardown failure shows "Close
failed" and still follows; one that left the workspace in place releases
the hold with no follow-up. Closing the populated workspace beside an
empty one lands on EM1, not on a session close.

Other windows on that workspace take the L1 path: their workspace no
longer belongs to the session, so they owe an emptied close and core
answers `LeaveWorkspace`. Until then a checkpoint or Quit may capture
them naming the closed workspace; restore's shown-navigation fallback
covers that.

### Empty state (EM1)

A window whose shown workspace holds no tab hides its tab bar and draws,
in the pane area, the workspace icon, "No tabs in `name`", a primary New
Tab button with its shortcut hint, Switch Workspace… (opens the workspace
menu anchored to it; absent with one workspace), and Close Workspace
(absent for the session's only workspace). Switch Workspace… keeps
fullscreen, Quake, and server-decorated Linux windows, which have no
title row to host the dropdown, from getting stuck. Focus moves to New
Tab; Tab and Shift-Tab cycle the buttons. The buttons are `huterm-ui`
`Button`s with accessible names.

### Session menu (Q1)

New Workspace follows New Session (workspace icon) and runs
`new_workspace`. A session with more than one workspace shows "N
workspaces · M tabs" as its detail. The native lowering keeps both.

## PR B: panel, bar, and dropdown

### Config and Settings

- `huterm-config`: `WorkspacesConfig { show, position, collapsed }` in a
  new `[workspaces]` table and `WindowConfig::session_button_placement`,
  with schema metadata, `schema:generate`, fixtures, and docs.
- Settings: `SectionId::Sessions` after Windows, groups "Session control"
  (`session_button`, `session_button_placement`), "Workspaces" (`show`,
  `position`, `collapsed`; the last two dim under `never`), and "Restore
  and names" (`restore_sessions`, `session_numbering`, moved from General
  › Sessions). Search finds them by name.
- `show = "never"` also hides PR A's session menu row and the panel
  commands; the workspace commands stay in the palette.
- Reload: `apply_reload` already re-lays out windows; the workspace chrome
  reads `self.config` in the same pass.

### Resolving what a window shows

`WorkspaceChrome::resolve` is one pure function, read by layout,
rendering, hit testing, menus, and smoke state. Inputs: `show`, the
session's workspace count, the window's collapse state, `position`, and
the terminal width a side panel would leave after the tab column, frame,
and safe area.

- `None`: `never`, or `auto` with fewer than two workspaces.
- `Dropdown`: collapsed, or a side panel that would leave the terminal
  under 384 points (SW1).
- `Panel { edge, width }` for left and right, `Bar { edge }` for top and
  bottom.

The session control resolves to a set of placements: `title_bar` gives
today's host; `sidebar` gives the panel's first row or the bar's leading
slot, falling back to today's host under `None` or `Dropdown` (F1);
`both` gives both, or one under the fallback. Each placement paints and
records its own bounds, the click names its host, and the session menu
anchors to the host that opened it. `open_session_menu` from the palette
uses the first visible host.

### Geometry

`ChromeLayout::build` takes the resolved chrome and carves it from the
content first, then partitions the rest exactly as today. It gains a
`workspace: Option<Bounds>` and that band's strip inset.
`WorkspaceChrome::None` must reproduce today's layout exactly.

| Chrome      | Band                                                                                                                      | What moves                                                                                                                                                                            |
| ----------- | ------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Left panel  | Full content height, width 140 to 400 and at most half the content; rows start below the title row and safe area          | Traffic lights sit in the panel strip; the title row and tab bar span the rest                                                                                                        |
| Right panel | Mirror                                                                                                                    | The menu button and a title-bar session control sit in the panel strip, as above a right tab column                                                                                   |
| Top bar     | The title row when the window has one; otherwise 32 points below the safe area; in non-native fullscreen, the notch shelf | `titlebar` tabs lay out as a top bar beneath it; the title text is dropped; the menu button and session control end the bar                                                           |
| Bottom bar  | 32 points at the content bottom                                                                                           | The tab bar sits above it                                                                                                                                                             |
| Dropdown    | None                                                                                                                      | Horizontal strips take a leading width (trigger and divider) through `strip_bounds` and `TabStrip`; a column's rows start below a trigger row; a hidden bar's title row leads with it |

- **Title row ownership.** Whether the window has a Huterm title row
  stays a property of the configured position and platform:
  `window_frame`, the Linux frame inset, drawn buttons, and drag and
  double-click gestures read the host-resolved position before the
  workspace remap, and decoration requests keep reading the raw
  configured position, as `apply_reload` does today. Only the tab bar's own layout
  sees `top` under a top workspace bar. So `auto` showing or hiding the
  bar never changes decorations; the bar draws the Linux buttons at the
  ends the desktop layout names, and its empty space moves the window.
- **Fullscreen auto-hide.** A horizontal workspace bar on the tab bar's
  edge hides and reveals with it as one band: one presentation, one
  reveal progress, one trigger edge, and a hover-hold region covering both
  bars. A bar on another edge, and a side panel, stay shown.
- **Notch shelf.** A top workspace bar takes the shelf; the tab bar then
  never selects it and keeps today's no-auto-hide rule for shelf windows.
- `resize_sidebar` measures from the tab column's own edge, not the
  window edge. The panel's inner edge has its own handle with the same
  clamp; its width is per window.
- PTY size, mouse mapping, scrollbars, and pane drops follow `terminal`;
  `title_row_layout` excludes the dropdown and panel-strip controls.
  `bar_reservation` and `initial_window_size` count the band.

### Invalidation

`Touched` gains a session-level workspace-chrome flag, set by workspace
creation, closure, move, and rename, and by a tab opening or closing in
any workspace; `windows_to_visit` then visits every window on that
session, so counts, order, names, and badges stay current, including in an
open workspace menu. A window's own workspace switch notifies the other
windows on its session (window badges) without a reconcile.

### Panel and bar

- P1: a header ("WORKSPACES", New Workspace, and collapse buttons) and
  28-point rows; a `ScrollArea` with the tab column's overlay scrollbar;
  showing a workspace scrolls its row into view. Background one step from
  the tab bar's, derived from theme tokens (no new theme key).
- H1: 24-point chips scrolling with the tab bar's arrows and indicator; +
  and collapse pinned at the end.
- Rows and chips: click switches; double-click runs the rename prompt; a
  right press opens a `MenuSpec` context menu (Rename…, Clear Name, New
  Workspace, Close Workspace…) acting on that row; `macos_native_menus`
  is honoured.
- Keyboard: `focus_workspace_panel` focuses the shown row; arrows, Home,
  End, Enter, F2 (rename), Shift-F10 or the menu key, and Escape (back to
  the terminal), with `huterm-ui` roving focus. Each row is a button named
  "Workspace: `name`, `n` tabs", with selected state on the shown one.

### Dropdown and workspace menu

- The trigger is `huterm-ui` `MenuTrigger` with a new workspace icon, the
  display name (dim when automatic), and a chevron; accessible name
  "Workspace: `name`", expanded state while open.
- M1: a `MenuSpec` like the session menu: a row per workspace (check,
  name, "N tabs", window badge when another window shows it), then New
  Workspace, Rename Workspace…, Clear Name (custom names only), Close
  Workspace…, Show Workspace Panel. Opens under the trigger, upward from a
  bottom bar; `open_workspace_menu` anchors to the trigger, the EM1
  button, or the menu button's fallback. Live updates and vanished rows as
  in the session menu.
- X1: `toggle_workspace_panel` (Window scope) flips the window's collapse
  state, saved with the window.

### Persistence

`WindowLayout` gains `workspace_panel_collapsed: bool` and
`workspace_panel_width: f32` (serde default, skipped when default,
validated finite and non-negative). New windows take
`[workspaces] collapsed` and 180 points.

## PR C: status dot

- Core: a status subscription that needs no viewer, fed from
  `StatusPublisher::flush` (already batched by revision). One coalescing
  wake per subscriber; a read returns, per workspace of a session, each
  terminal's identity, lifecycle, failure, foreground-busy flag, and
  lifetime bell count, read from the registries without waiting.
- Desktop: each window keeps a bell baseline per terminal: the count when
  it last showed that terminal's workspace, set on first sight and on
  leaving, pruned with the terminal. The window applies its baseline,
  then rolls each workspace up to the most urgent of busy, bell, exited,
  and failed (the `TabStatus` order). Reads run on a worker; a result
  older than the latest applied revision is dropped. Two windows can
  differ in bell state.
- The dot draws on rows, chips, and workspace menu rows, with a tooltip
  naming the tabs.

## Quit, close, and switch interleavings

Rules from [session switching](session-switching.md#rules) apply
unchanged. Additions: a workspace switch and an L1 follow-up count as
switches (Q4, Q7); New Workspace counts as a shell-producing switch; an
own-workspace Close Workspace behaves as an own-session Close Session for
Q2 and Q3 and holds Quit and checkpoints from its commit through its
direct follow-up; a `LeaveWorkspace` commit holds them until its
follow-up settles; an emptied close converts to Quit only after a fresh
`Detach` or `Session` assessment.

| #    | State                                                                                                        | Event                                           | Intended behavior                                                                                             | Test                                                                                                                        |
| ---- | ------------------------------------------------------------------------------------------------------------ | ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| W1   | Workspace switch installing                                                                                  | Quit in the same window                         | Quit queues and runs after settle; both workspaces' navigation saved                                          | `quit_waits_for_a_workspace_switch_and_saves_both_workspaces`                                                               |
| W2   | Workspace switch installing                                                                                  | Quit in another window                          | Quit waits (Q4) and saves the restored tab and pane                                                           | `quit_from_another_window_waits_for_a_workspace_switch`                                                                     |
| W3   | Workspace switch or L1 follow-up in flight                                                                   | Checkpoint due                                  | Deferred until settle (Q7)                                                                                    | `a_checkpoint_waits_for_a_workspace_switch`                                                                                 |
| W4   | Workspace switch in flight                                                                                   | Close Window                                    | Queued; runs after settle                                                                                     | `a_window_close_queued_behind_a_workspace_switch_runs_once_it_settles`                                                      |
| W5   | Quit requested or pending                                                                                    | New, select, next, previous, own close          | Refused, "application is quitting" (Q3)                                                                       | `workspace_changes_are_refused_while_a_quit_runs`                                                                           |
| W6   | New Workspace preparing                                                                                      | Shell fails                                     | Workspace closed, one notice, window unchanged                                                                | `a_failed_new_workspace_closes_only_what_it_created`                                                                        |
| W7   | New Workspace worker held                                                                                    | Quit                                            | Quit waits; the new workspace is saved                                                                        | `quit_waits_for_a_new_workspace`                                                                                            |
| W8   | Select issued                                                                                                | Target closed first                             | Refused with "Workspace no longer exists"; window unchanged                                                   | `selecting_a_closed_workspace_leaves_the_window_unchanged`                                                                  |
| W9   | Last tab of the shown workspace closes; another holds tabs                                                   | Tab close or exit-close commits                 | `LeaveWorkspace`: the empty workspace closes, the window moves to its remembered workspace; no Linux Quit     | `closing_a_workspaces_last_tab_moves_to_the_previous_workspace`, `an_exit_close_of_a_workspaces_last_tab_moves_on`          |
| W10  | Last tab closes; no other workspace holds tabs, workspace still exists                                       | Same                                            | Today's emptied close (detach, session close, Linux Quit rule)                                                | existing emptied-close tests, plus `an_emptied_workspace_with_only_empty_siblings_closes_the_window`                        |
| W11  | Emptied close assessed (`Assessed` held) as `Detach`, including a Linux last window with an unviewed session | A sibling gains a tab before commit             | The freshness check fails before any Quit conversion; reassessment answers `LeaveWorkspace`; the window stays | `a_sibling_gaining_a_tab_turns_a_held_emptied_detach_into_leave_workspace`, `a_stale_emptied_detach_on_linux_does_not_quit` |
| W12  | Emptied close assessed as `LeaveWorkspace`                                                                   | The only populated sibling empties first        | Stale; reassessment answers `Detach` or `Session`; the last-window rule applies                               | `a_sibling_emptying_turns_a_held_leave_workspace_into_a_window_close`                                                       |
| W13  | Linux last window, `LeaveWorkspace` assessed                                                                 | Commit                                          | No Quit conversion; the window stays and follows                                                              | `leaving_a_workspace_never_becomes_quit_on_linux`                                                                           |
| W14  | L1 follow-up chose W2 because it held tabs                                                                   | W2 empties or closes before settle              | The follow-up keeps its recovery intent and owes the emptied close again at settle; the next pass resolves it | `an_l1_follow_up_whose_target_went_resolves_again`                                                                          |
| W15  | `LeaveWorkspace` commit held, before its follow-up                                                           | Quit or checkpoint                              | Quit waits and checkpoints defer until the follow-up settles                                                  | `quit_waits_for_a_leave_workspace_follow_up`                                                                                |
| W16  | Emptied close owed (L1)                                                                                      | Quit confirming                                 | Absorbed by the Quit; a cancelled Quit leaves it owed and it runs next                                        | `a_cancelled_quit_lets_an_owed_leave_workspace_run`                                                                         |
| W17  | Two windows on W; A closes W's last tab                                                                      | Commit                                          | Both leave; W closed once; each lands on its own remembered workspace                                         | `two_windows_on_an_emptied_workspace_both_move_on`                                                                          |
| W18  | A on W, B on W2                                                                                              | A leaves W                                      | B unaffected                                                                                                  | `leaving_a_workspace_leaves_other_windows_alone`                                                                            |
| W19  | Own Close Workspace assessing or confirming                                                                  | Quit                                            | Cancelled or dismissed (Q2); the workspace survives and is saved                                              | `a_quit_cancels_an_own_workspace_close`                                                                                     |
| W20  | Own Close Workspace committing (commit held)                                                                 | Quit in another window, or a checkpoint         | Quit waits and the checkpoint defers through the commit and the direct follow-up (Q4, Q7)                     | `quit_waits_for_a_committing_workspace_close_and_its_follow_up`, `a_checkpoint_waits_for_an_own_workspace_close`            |
| W21  | Close Workspace of W from A; B shows W, or B shows W empty (EM1)                                             | Commit                                          | A and B both leave through `LeaveWorkspace`                                                                   | `closing_a_shared_workspace_moves_both_windows`, `closing_a_workspace_shown_empty_moves_its_window`                         |
| W22  | Close Workspace commit removes W, then teardown fails                                                        | Completion                                      | "Close failed" notice; the window still leaves W                                                              | `a_failed_workspace_teardown_still_leaves_the_workspace`                                                                    |
| W22a | Own Close Workspace commit goes stale and leaves the workspace in place                                      | Completion                                      | Hold released, no follow-up; the close assesses again                                                         | `a_stale_own_workspace_close_releases_its_hold`                                                                             |
| W23  | Close Workspace of the populated one of two, the other empty                                                 | Commit                                          | The window shows the empty sibling (EM1); the session survives                                                | `closing_the_populated_workspace_shows_the_empty_one`                                                                       |
| W24  | Close Workspace on the session's only workspace                                                              | Command or menu                                 | Unavailable; core refuses `LastWorkspace` if reached                                                          | `the_last_workspace_cannot_be_closed_as_a_workspace`                                                                        |
| W25  | W moves to another session (another client), with tabs or empty                                              | Batch                                           | The window leaves W; core leaves W alone in its new session                                                   | `a_populated_workspace_moved_away_is_left`, `an_empty_workspace_moved_away_is_left_and_kept`                                |
| W26  | Windows on different workspaces of one session                                                               | Close Session from one                          | Existing Q6 follow-up for that window; others take the session-gone path; memory pruned                       | `closing_a_session_shown_on_two_workspaces`                                                                                 |
| W27  | Windows on different workspaces of one session                                                               | Quit, then restore                              | Each returns to its workspace with its own navigation                                                         | `two_windows_on_two_workspaces_restore_their_workspaces`                                                                    |
| W28  | Saved memory names closed or moved workspaces                                                                | Restore                                         | Those entries drop; duplicates collapse; the shown navigation keeps its fallback                              | `restore_drops_stale_workspace_memory`                                                                                      |
| W29  | Quake stash on W                                                                                             | W closes, checkpoint, then summon               | Shows its remembered or first workspace with tabs                                                             | `a_quake_stash_on_a_closed_workspace_shows_another`                                                                         |
| W30  | Window shows an empty workspace it selected (EM1)                                                            | Settle, Quit, checkpoint                        | No emptied close; Quit and checkpoint save it                                                                 | `selecting_an_empty_workspace_shows_the_empty_state`                                                                        |
| W31  | Close confirmation, reservation, or arrangement showing                                                      | Workspace switch                                | Refused by the guard                                                                                          | `workspace_switches_respect_the_structural_guard`                                                                           |
| W32  | PR B: panel toggle, position change, or config reload                                                        | While a terminal has focus                      | Focus, process, and mouse mapping preserved; only the size changes                                            | `panel_changes_keep_focus_mouse_mapping_and_processes`                                                                      |
| W33  | PR B: session menu or workspace menu open                                                                    | Collapse, reload, or the host disappears        | The menu closes or re-anchors to a remaining host; focus returns to the terminal                              | `a_menu_whose_host_goes_closes_cleanly`                                                                                     |
| W34  | A commits `LeaveWorkspace` (commit held); Linux; an unviewed session                                         | Close Window in B                               | A still counts as open, so B closes without becoming Quit                                                     | `a_window_leaving_a_workspace_still_counts_as_open`                                                                         |
| W35  | Emptied close assessed in A                                                                                  | Quit confirming in B, then A reaches its commit | `LeaveWorkspace` commits and follows, and the Quit waits for its hold; `Detach` and `Session` behave as today | `a_leave_workspace_commits_while_a_quit_confirms`                                                                           |

## Observable success

- With one workspace and `show = "auto"`, every window looks and behaves
  as on `main`.
- New Workspace from the session menu or palette shows a fresh shell in a
  new workspace; the panel appears in every window on that session.
- Two windows on one session can show different workspaces and switch
  independently; each returns to its own tab and pane in each workspace.
- Closing a workspace's last tab lands on the previous workspace;
  closing the last workspace's last tab closes the window as today.
- The panel, bar, and dropdown match the picked mockups at every edge
  and tab position, in fullscreen, Quake, narrow windows, and on Linux.

## Failure paths and risks

| Risk                                                      | Handling                                                                                                    |
| --------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| An emptied detach wins over L1                            | Core decides `LeaveWorkspace` under the lock, at the commit's freshness check; W11, W12                     |
| A selected empty workspace triggers an emptied close      | Only removal, a gone workspace, or a gone session owes one; W30                                             |
| A Linux last window quits when it only leaves a workspace | Quit conversion only for `Detach` or `Session` effects; W13                                                 |
| Decorations flip as `auto` shows the bar                  | Title row ownership reads the configured position, not the remapped tab layout                              |
| Geometry regressions in today's tab positions             | `WorkspaceChrome::None` must equal today's `ChromeLayout`; a test compares both across positions and frames |
| A side panel squeezes the terminal                        | SW1 on the terminal width after the tab column; tiny-window tests                                           |
| Status wakes too often (PR C)                             | Coalesced: one pending wake, one worker read at a time                                                      |
| Downgrade reads several entries per session               | Cap stays 256; older validation accepts duplicates                                                          |
| Overlap with the #81/#407 attention work                  | PR C rebases onto whichever lands first and reuses its attention source if one exists                       |
| Overlap with #44's ID migration                           | Mechanical rebase                                                                                           |

## Non-goals

- Tree style (P4), opening a workspace in a new window, reordering
  workspaces and moving tabs onto them (#32), per-window panel position,
  default key bindings, and server-mode workspace views (#45).
- Preserving a selection across a terminal resize (settled decision 9,
  tracked on [#373][issue-373]).

## Verification

| Concern                                                                                                                                                                                         | Evidence                                                                                                                                                                                                                                                                                 |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Core: workspace close consent, last-workspace refusal, stale tickets; `LeaveWorkspace` resolution order and commit                                                                              | `huterm-core` lifecycle tests, one per resolution branch                                                                                                                                                                                                                                 |
| Atomic create-with-shell rollback                                                                                                                                                               | `huterm-core` mux test                                                                                                                                                                                                                                                                   |
| Commands: availability, defaults, picker filtered to the session                                                                                                                                | Catalog and palette tests                                                                                                                                                                                                                                                                |
| Switching keeps shells alive and restores per-workspace navigation and undo                                                                                                                     | Tier 2 window tests with shell acknowledgements                                                                                                                                                                                                                                          |
| One view's workspace change leaves another view's workspace, tab, pane, and selection alone (acceptance 3)                                                                                      | Tier 2 two-window test asserting each explicitly, plus a `check-viewers.ts` step through `invoke-in select_workspace` on macOS and Linux                                                                                                                                                 |
| Interleavings W1 to W35                                                                                                                                                                         | One test per row, ordered by worker hooks and sequence waits, never sleeps                                                                                                                                                                                                               |
| Navigation memory save, strict restore, cap, older files                                                                                                                                        | `huterm-state` schema tests; tier 2 restore tests                                                                                                                                                                                                                                        |
| Geometry: 4 edges × 5 tab positions, collapsed, fullscreen and shared-edge auto-hide, Quake, notch shelf, Linux frame with leading and trailing buttons, small windows, overflow (acceptance 1) | Pure `ChromeLayout` and `WorkspaceChrome::resolve` tests beside `all_placements_share_nonoverlapping_terminal_and_tab_bounds`; `None` equals today's layout                                                                                                                              |
| Mouse mapping and sidebar resize with a panel beside the tab column                                                                                                                             | Extend `application_mouse_coordinates_follow_all_tab_placements` and the sidebar resize tests                                                                                                                                                                                            |
| Panel changes and reload keep focus, mouse coordinates, and processes (acceptance 2)                                                                                                            | W32 tier 2 test; native steps in `smoke:macos-palette` and `smoke:linux-palette`: real key input reaches the same shell (same pid) after a toggle, a PTY mouse report lands on the expected cell after a toggle, panel rect, row click switches, dropdown opens its menu, keyboard focus |
| Session control placements, per-host anchors                                                                                                                                                    | Pure placement tests; window tests opening the menu from each host                                                                                                                                                                                                                       |
| Invalidation: a hidden workspace's count changes; another window's badge updates with its menu open                                                                                             | Window tests                                                                                                                                                                                                                                                                             |
| Menus: keyboard, live updates, native lowering                                                                                                                                                  | Window tests on the workspace menu; `MenuSpec` lowering tests                                                                                                                                                                                                                            |
| Status (PR C): roll-up order, coalescing, two windows with different baselines, a publication during a read                                                                                     | Core and window tests                                                                                                                                                                                                                                                                    |
| Settings section, schema, coverage                                                                                                                                                              | `settings.rs` section tests, schema check, coverage test                                                                                                                                                                                                                                 |
| Visual fidelity to the picked mockups                                                                                                                                                           | Screenshots beside the mockups in each PR; `ux-review` (Codex computer use, scratch bundle) briefed with the PNGs                                                                                                                                                                        |

## Open questions

None. The three questions raised after review were settled on
2026-11-07 (settled decisions 9 to 11).

## Deviations

Recorded while implementing PR A:

- The Quit conversion's freshness check runs on the emptied close's commit
  worker rather than as a separate round trip: `finish_close` decides that
  a `Detach` or `Session` assessment would become Quit, and the worker then
  only calls the new check-only `Mux::validate_close_scope` under the lock
  and commits nothing. A fresh ticket becomes Quit; a stale one assesses
  again. The UI thread never takes the Mux lock for it.
- A Quit confirmed while a `LeaveWorkspace` follow-up holds
  `pending_spawns` (W35) dismisses its dialog and waits as a Quit requested
  then would (`quit_waits_for_holds`); once the hold settles it assesses
  again and asks again only when jobs still run. The leave's commit already
  moved the revision, so the old consent could not have committed anyway.
- The empty state's buttons take the existing `Panel` key context, so Tab,
  Shift-Tab, Enter, and Space reuse the `panel_*` commands instead of new
  catalog commands; the commands reach the empty state when one of its
  buttons holds focus.
- The workspace icon is Lucide's `layout-grid`, added to the pinned UI
  icons, on the empty state and the session menu's New Workspace row. PR B
  can reuse it for the dropdown and panel.
- The follow-up's fallback, "else the first workspace", is the session's
  first workspace other than the one the window left.
- Restore also drops a remembered entry that names the workspace the window
  shows, since that workspace's own navigation applies there.
- The picker filter applies to `select_workspace` only. Other window
  commands that take a workspace, such as `move_pane_to_new_tab`, still
  list every session's workspaces, as before.
- `rename_workspace` already defaulted to the window's workspace in the
  desktop, so it is unchanged.
- The new commands join the Settings keybindings page's existing Sessions
  group, which every command must belong to; no Settings section changed.
- While Quit asks in a window, that window's dialog refuses the workspace
  commands first ("close confirmation pending"), as it does for every
  command; W5's "application is quitting" applies while Quit waits and in
  other windows.
- Tier 2 tests exercise the Linux last-window conversions on macOS through
  a test-only platform override on the view (`test_platform`).
- W29's test takes the stash and opens a regular window from it, since a
  tier 2 test cannot place a Quake window; the summon path runs the same
  `take_stash`.

[issue-11]: https://github.com/jimeh/huterm/issues/11
[issue-26]: https://github.com/jimeh/huterm/issues/26
[issue-27]: https://github.com/jimeh/huterm/issues/27
[issue-373]: https://github.com/jimeh/huterm/issues/373
