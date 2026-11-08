# Workspace panels

Status: in progress. Plans [issue #27][issue-27] (parent [#11][issue-11]).
The maintainer picked the mockups and settled the open questions on
2026-11-07, then revised PR B's design on 2026-11-08; see
[Settled decisions](#settled-decisions). PR A is implemented (see
[Deviations](#deviations)); PR B is being reworked to the revised design;
PR C remains.

A window can show any workspace of its session, switch between them from
a left or right sidebar or a switcher beside the sidebar button, reorder
them by dragging, and create, rename, clear the name of, and close
workspaces. The sidebar also holds the session switcher. People who never
create a second workspace see only the sidebar button. Builds on
[#26][issue-26]
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

Revised on 2026-11-08 from the sidebar mockup
(`https://plans.jimeh.dev/s3ektagrz4n65s7qmyy5n6soyq/huterm-sidebar-mockup.html`).
These replace decisions 1, 3, 5, 6, and 11, the SW1 part of 4, and the
top and bottom bar parts of 2; the rest stand.

 1. The workspace panel becomes a general sidebar, docked on the left
    (default) or right only. It holds the workspace list and the session
    switcher.
 2. A sidebar button is always visible in the corner on the sidebar's
    side: after the traffic lights on the left, just inside the menu
    button on the right (R1). It stays in the same spot while the
    sidebar shows, inside the sidebar's strip (V1).
 3. While the sidebar is hidden and the session has two or more
    workspaces, a workspace switcher sits beside the sidebar button: icon,
    name, and chevron (K1), icon only when there is no room.
 4. The switcher opens a GPUI popover, as tall as its content, holding
    the sidebar's content, even with native menus (P1).
 5. Session switcher: a row at the bottom of the sidebar (B1) or at the
    top (B3), chosen by `[sidebar] session_switcher`, default bottom. It
    uses the layers icon, as the #26 control does. The #26 title-bar
    session control becomes opt-in, off by default (H1).
 6. Workspaces reorder by drag and drop, in the sidebar and in the
    popover. Dropping a tab on a row and dragging a workspace to another
    window are follow-ups.
 7. New windows start with the sidebar hidden (`[sidebar] visible`);
    each window's state and width are saved with its layout. The sidebar
    never opens by itself. A window too narrow to dock it (terminal under
    384 points) opens it as a sheet over the terminal instead.
 8. Vertical tabs on the sidebar's side stay a separate column, with the
    sidebar outermost (N1). Nesting tabs under their workspace (N2) is a
    possible follow-up.
 9. The switcher popover and the session popover are GPUI popovers; a
    row's right-click menu stays a `MenuSpec` and may be native.
10. Settings: separate "Sessions" and "Workspaces" sections after
    Windows, replacing G1's single section.

Config:

```toml
[window]
session_button = false        # the #26 title-bar session control

[sidebar]
position = "left"             # left or right
visible = false               # new windows start with the sidebar shown
session_switcher = "bottom"   # bottom or top
```

## Delivery

Three PRs from this thread, in order (settled decision 10):

- **A. Workspace switching and lifecycle.** Core close and leave-workspace
  outcomes, the workspace commands, per-workspace navigation memory, the
  switch, L1, the empty state, restore, and the session menu's New
  Workspace row. Reachable through the palette and the session menu.
- **B. Sidebar and workspace switcher.** Config, the Sessions and
  Workspaces settings sections, chrome geometry, the sidebar, its button,
  the switcher and its popover, the session switcher, drag reordering,
  and per-window visibility and width. Rows show name and tab count (R1).
- **C. Workspace status dot (R3).** A core status subscription that needs
  no viewer, per-window bell acknowledgement, and the dot on the
  sidebar's and the popover's rows.

A lands lifecycle risk with focused review before B adds the largest
geometry change since the tab sidebar; C adds a core notification path
that deserves its own review. A is useful alone: the palette and the
session menu reach everything.

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
  2. That session holds any other workspace, with or without tabs,
     including when the workspace no longer belongs to that session
     (closed, or moved to another session) while the session keeps one:
     `LeaveWorkspace`. An emptied window never ends a session that still
     holds another workspace (revised 2026-11-07; see
     [Deviations](#deviations)).
  3. Otherwise, when it was the session's only workspace, today's
     `Detach` or `Session`.

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
     `validate_close_at`), so a sibling workspace that appeared sends it
     back to assessment instead of quitting. `LeaveWorkspace` never
     converts: a Linux last window that only leaves a workspace stays
     open.
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
menu anchored to it; absent with one workspace; in PR A it opens the
`select_workspace` palette prompt until PR B adds the workspace menu, see
[Deviations](#deviations)), and Close Workspace (absent for the session's
only workspace). Switch Workspace… keeps
fullscreen, Quake, and server-decorated Linux windows, which have no
title row to host the dropdown, from getting stuck. Focus moves to New
Tab; Tab and Shift-Tab cycle the buttons. The buttons are `huterm-ui`
`Button`s with accessible names.

### Session menu (Q1)

New Workspace follows New Session (workspace icon) and runs
`new_workspace`. A session with more than one workspace shows "N
workspaces · M tabs" as its detail. The native lowering keeps both.

## PR B: sidebar and workspace switcher

Revised on 2026-11-08 (settled decisions 12 to 21). The first design's
panel, bar, and dropdown were implemented and reviewed, then replaced
before merging; [PR B deviations](#pr-b-deviations) records what carried
over.

### Config and Settings

- `huterm-config`: a new `[sidebar]` table, `SidebarConfig { position,
  visible, session_switcher }`, with schema metadata, `schema:generate`,
  fixtures, and docs. `[workspaces]` and
  `[window] session_button_placement` go; neither was released.
  `[window] session_button` stays and now defaults to `false`.
- Settings: `SectionId::Sessions` and then `SectionId::Workspaces`, after
  Windows. Sessions holds "Session switcher" (`session_switcher`,
  `session_button`) and "Restore and names" (`restore_sessions` and
  `session_numbering`, moved from General › Sessions). Workspaces holds
  "Sidebar" (`position`, `visible`). Search finds them by name.
- Reload: `apply_reload` re-lays out windows; a position change moves a
  docked sidebar and its button. A `visible` change applies to new
  windows only.

### What a window shows

`WorkspaceChrome::resolve` is one pure function read by layout, rendering,
hit testing, popovers, and smoke state. Inputs: the window's visible
state, `position`, the session's workspace count, and the terminal width
a docked sidebar would leave after the tab column, frame, and safe area.

- `Docked { edge, width }` while visible and the terminal keeps at least
  384 points.
- `Undocked { edge }` otherwise. While visible but too narrow to dock,
  the sidebar button opens the sidebar as a sheet over the terminal
  instead (settled decision 18).
- The workspace switcher shows while undocked and the session has two or
  more workspaces.

### Corner controls

- The sidebar button sits in the corner on the sidebar's side, in
  whichever element holds that corner: the docked sidebar's strip, a tab
  column's strip, the title-bar tab bar, or the title row. On the left it
  follows the traffic lights (macOS) or leads (Linux); on the right it
  sits just inside the menu button (R1), before any Linux window buttons.
  Docking moves it into the sidebar's strip at the same spot (V1).
- The switcher sits beside the button on its inner side. In a tab
  column's strip, it is a full-width row at the top of the column
  instead. It is a `MenuTrigger` with the workspace icon, the display
  name (dim when automatic), and a chevron (K1), named "Workspace:
  `name`", with expanded state while its popover is open. It shrinks to
  the icon (K3) when the strip has no room for the name.
- With no title row (Linux server decorations, fullscreen, Quake), the
  controls lead or end the tab bar; see [Open questions](#open-questions).
  In fullscreen they hide and reveal with the auto-hidden bar that holds
  them; a docked sidebar stays shown.
- The title-bar session control from #26 shows only with
  `session_button = true` (H1).

### Sidebar content

- Header: "WORKSPACES" and New Workspace.
- Rows (R1 now, the dot in PR C): icon, name (dim when automatic), and
  tab count; 28 points; a `ScrollArea` with the tab column's overlay
  scrollbar; showing a workspace scrolls its row into view. Click shows;
  double-click runs the rename prompt (RN1); a right press opens a
  `MenuSpec` context menu (Rename…, Clear Name, New Workspace, Close
  Workspace…) that honours `macos_native_menus`.
- Drag to reorder (settled decision 17): rows reorder within the list in
  the sidebar and in the switcher popover, with the tab drag's threshold,
  insertion line, edge auto-scroll, and Escape to cancel. A drop calls
  `Mux::move_workspace` within the session with the row it lands before
  as the anchor, so concurrent changes cannot misplace it. A drag whose
  workspace goes, or whose session changes, cancels.
- Session switcher (B1, B3 as a setting): a row with the layers icon, the
  session name, an "N workspaces" caption at the bottom (compact at the
  top), and an up-down chevron. It opens the session menu as a Huterm
  popover even with native menus (settled decision 20): upward from the
  bottom, downward from the top, beside the switcher popover when opened
  from it.
- Keyboard: `focus_sidebar` focuses the shown row; a `Sidebar` key
  context's `sidebar_*` commands walk rows with the arrows, Home, and End,
  show with Return, rename with F2, open the row menu with Shift-F10 or
  the menu key, and return to the terminal with Escape, with `huterm-ui`
  roving focus. Each row is a button named "Workspace: `name`, `n` tabs",
  selected while shown. Tab reaches the session row.
- Background one step from the tab bar's, derived from theme tokens. The
  inner edge resizes the sidebar from 140 to 400 points and at most half
  the content.

### Workspace switcher popover

- P1: a GPUI popover, never a native menu, anchored under the switcher
  and as tall as its content up to a cap, then scrolling. It holds the
  sidebar's content: header, rows (click, drag, double-click, right
  press), and the session row at the configured end. A row click shows
  that workspace and closes the popover; Escape or an outside press
  closes it and returns focus to the terminal.
- The sheet (narrow windows) holds the same content at full height below
  the corner strip, and closes the same ways.
- `open_workspace_switcher` opens it at the switcher, else at the EM1
  button, else at the sidebar button; the empty state's Switch Workspace…
  opens it at itself.
- Live updates: counts, names, order, and badges follow `Touched::chrome`;
  a vanished row disappears; the popover closes when its anchor goes.

### Sidebar commands

`toggle_sidebar` (Window scope; flips the window's visible state, or opens
the sheet while undocked by width), `focus_sidebar`, and
`open_workspace_switcher` replace `toggle_workspace_panel`,
`focus_workspace_panel`, and `open_workspace_menu`. No default bindings.

### Geometry

`ChromeLayout::build` takes the resolved chrome, carves the docked
sidebar's band from the content first, then partitions the rest exactly
as on `main`. A hidden sidebar must reproduce `main`'s layout except for
the corner controls' slot.

| Chrome       | Band                                                                 | What moves                                                                                                                               |
| ------------ | -------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| Docked left  | Full content height; rows start below the corner strip and safe area | The traffic lights and the sidebar button sit in the sidebar's strip; the title row and tab bar span the rest                            |
| Docked right | Mirror                                                               | The sidebar button, menu button, title-bar session control, and Linux buttons sit in the sidebar's strip                                 |
| Undocked     | None                                                                 | The corner's holder reserves a leading (left) or trailing (right) slot for the button and switcher through `strip_bounds` and `TabStrip` |
| Sheet        | Overlay below the corner strip; not part of the layout               | Nothing; the terminal keeps its size                                                                                                     |

- Title row ownership stays a property of the configured tab position and
  platform: `window_frame`, the Linux frame inset, drawn buttons, and
  drag and double-click gestures read the host-resolved position, so
  showing or hiding the sidebar never changes decorations.
- `resize_sidebar` (the tab column) measures from the column's own edge;
  the sidebar's inner edge has its own handle; both widths are per
  window.
- PTY size, mouse mapping, scrollbars, and pane drops follow `terminal`;
  `title_row_layout` excludes the corner controls. `initial_window_size`
  counts a docked band.

### Invalidation

`Touched::chrome` holds the sessions whose workspace list, order, names,
or tab counts changed; `windows_to_visit` visits every window on them, so
counts, order, names, and badges stay current, including in an open
switcher popover. A window's own workspace switch notifies the other
windows on its session.

### Persistence

`WindowLayout` gains `workspace_panel_visible: Option<bool>` and
`workspace_panel_width: f32` (serde default, skipped when default, width
validated finite and non-negative); `sidebar_width` already names the tab
column's width. A window saved without a visible state, and a new window,
take `[sidebar] visible`; width defaults to 180 points.

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
- The dot draws on the sidebar's and the popover's rows, with a tooltip
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

| #    | State                                                                                                         | Event                                                                      | Intended behavior                                                                                                               | Test                                                                                                                                                                                                                                                                                                                                                                            |
| ---- | ------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| W1   | Workspace switch installing                                                                                   | Quit in the same window                                                    | Quit queues and runs after settle; both workspaces' navigation saved                                                            | `quit_waits_for_a_workspace_switch_and_saves_both_workspaces`                                                                                                                                                                                                                                                                                                                   |
| W2   | Workspace switch installing                                                                                   | Quit in another window                                                     | Quit waits (Q4) and saves the restored tab and pane                                                                             | `quit_from_another_window_waits_for_a_workspace_switch`                                                                                                                                                                                                                                                                                                                         |
| W3   | Workspace switch or L1 follow-up in flight                                                                    | Checkpoint due                                                             | Deferred until settle (Q7)                                                                                                      | `a_checkpoint_waits_for_a_workspace_switch`                                                                                                                                                                                                                                                                                                                                     |
| W4   | Workspace switch in flight                                                                                    | Close Window                                                               | Queued; runs after settle                                                                                                       | `a_window_close_queued_behind_a_workspace_switch_runs_once_it_settles`                                                                                                                                                                                                                                                                                                          |
| W5   | Quit requested or pending                                                                                     | New, select, next, previous, own close                                     | Refused, "application is quitting" (Q3)                                                                                         | `workspace_changes_are_refused_while_a_quit_runs`                                                                                                                                                                                                                                                                                                                               |
| W6   | New Workspace preparing                                                                                       | Shell fails                                                                | Workspace closed, one notice, window unchanged                                                                                  | `a_failed_new_workspace_closes_only_what_it_created`                                                                                                                                                                                                                                                                                                                            |
| W7   | New Workspace worker held                                                                                     | Quit                                                                       | Quit waits; the new workspace is saved                                                                                          | `quit_waits_for_a_new_workspace`                                                                                                                                                                                                                                                                                                                                                |
| W8   | Select issued                                                                                                 | Target closed first                                                        | Refused with "Workspace no longer exists"; window unchanged                                                                     | `selecting_a_closed_workspace_leaves_the_window_unchanged`                                                                                                                                                                                                                                                                                                                      |
| W9   | Last tab of the shown workspace closes; another holds tabs                                                    | Tab close or exit-close commits                                            | `LeaveWorkspace`: the empty workspace closes, the window moves to its remembered workspace; no Linux Quit                       | `closing_a_workspaces_last_tab_moves_to_the_previous_workspace`, `an_exit_close_of_a_workspaces_last_tab_moves_on`                                                                                                                                                                                                                                                              |
| W10  | Last tab closes (tab close, exit-close, lone pane close, tab moved away); another workspace exists, all empty | Same                                                                       | `LeaveWorkspace`: the emptied workspace closes and the window shows an empty sibling (EM1); the session survives; no Linux Quit | `closing_the_last_tab_beside_an_empty_workspace_shows_it`, `an_exit_close_beside_an_empty_workspace_shows_it`, `closing_the_last_lone_pane_beside_an_empty_workspace_shows_it`, `moving_the_last_tab_away_beside_an_empty_workspace_shows_it`, `a_linux_last_window_emptied_beside_an_empty_workspace_stays_open`, `two_windows_emptied_beside_an_empty_workspace_both_show_it` |
| W10a | Last tab of the session's only workspace closes                                                               | Same                                                                       | Today's emptied close (detach, session close, Linux Quit rule)                                                                  | existing emptied-close tests, plus `an_emptied_window_on_its_sessions_only_workspace_closes_with_the_session`                                                                                                                                                                                                                                                                   |
| W11  | Emptied close assessed (`Assessed` held) as `Detach`, including a Linux last window with an unviewed session  | A sibling workspace appears before commit                                  | The freshness check fails before any Quit conversion; reassessment answers `LeaveWorkspace`; the window stays                   | `a_new_sibling_turns_a_held_emptied_detach_into_leave_workspace`, `a_stale_emptied_detach_on_linux_does_not_quit`                                                                                                                                                                                                                                                               |
| W12  | Emptied close assessed as `LeaveWorkspace`                                                                    | The only sibling closes or moves to another session first                  | Stale; reassessment answers `Detach` or `Session`; the last-window rule applies                                                 | `the_only_sibling_closing_turns_a_held_leave_workspace_into_a_window_close`                                                                                                                                                                                                                                                                                                     |
| W12a | Emptied close assessed as `LeaveWorkspace` beside one populated sibling                                       | That sibling empties first                                                 | Stale; reassessment answers `LeaveWorkspace` again; the window shows the empty sibling (EM1); no Linux Quit                     | `a_sibling_emptying_keeps_a_held_leave_workspace_leaving`                                                                                                                                                                                                                                                                                                                       |
| W13  | Linux last window, `LeaveWorkspace` assessed                                                                  | Commit                                                                     | No Quit conversion; the window stays and follows                                                                                | `leaving_a_workspace_never_becomes_quit_on_linux`                                                                                                                                                                                                                                                                                                                               |
| W14  | L1 follow-up chose W2 because it held tabs                                                                    | W2 empties or closes before settle                                         | The follow-up keeps its recovery intent and owes the emptied close again at settle; the next pass resolves it                   | `an_l1_follow_up_whose_target_went_resolves_again`                                                                                                                                                                                                                                                                                                                              |
| W15  | `LeaveWorkspace` commit held, before its follow-up                                                            | Quit or checkpoint                                                         | Quit waits and checkpoints defer until the follow-up settles                                                                    | `quit_waits_for_a_leave_workspace_follow_up`                                                                                                                                                                                                                                                                                                                                    |
| W16  | Emptied close owed (L1)                                                                                       | Quit confirming                                                            | Absorbed by the Quit; a cancelled Quit leaves it owed and it runs next                                                          | `a_cancelled_quit_lets_an_owed_leave_workspace_run`                                                                                                                                                                                                                                                                                                                             |
| W17  | Two windows on W; A closes W's last tab                                                                       | Commit                                                                     | Both leave; W closed once; each lands on its own remembered workspace                                                           | `two_windows_on_an_emptied_workspace_both_move_on`                                                                                                                                                                                                                                                                                                                              |
| W18  | A on W, B on W2                                                                                               | A leaves W                                                                 | B unaffected                                                                                                                    | `leaving_a_workspace_leaves_other_windows_alone`                                                                                                                                                                                                                                                                                                                                |
| W19  | Own Close Workspace assessing or confirming                                                                   | Quit                                                                       | Cancelled or dismissed (Q2); the workspace survives and is saved                                                                | `a_quit_cancels_an_own_workspace_close`                                                                                                                                                                                                                                                                                                                                         |
| W20  | Own Close Workspace committing (commit held)                                                                  | Quit in another window, or a checkpoint                                    | Quit waits and the checkpoint defers through the commit and the direct follow-up (Q4, Q7)                                       | `quit_waits_for_a_committing_workspace_close_and_its_follow_up`, `a_checkpoint_waits_for_an_own_workspace_close`                                                                                                                                                                                                                                                                |
| W21  | Close Workspace of W from A; B shows W, or B shows W empty (EM1)                                              | Commit                                                                     | A and B both leave through `LeaveWorkspace`                                                                                     | `closing_a_shared_workspace_moves_both_windows`, `closing_a_workspace_shown_empty_moves_its_window`                                                                                                                                                                                                                                                                             |
| W22  | Close Workspace commit removes W, then teardown fails                                                         | Completion                                                                 | "Close failed" notice; the window still leaves W                                                                                | `a_failed_workspace_teardown_still_leaves_the_workspace`                                                                                                                                                                                                                                                                                                                        |
| W22a | Own Close Workspace commit goes stale and leaves the workspace in place                                       | Completion                                                                 | Hold released, no follow-up; the close assesses again                                                                           | `a_stale_own_workspace_close_releases_its_hold`                                                                                                                                                                                                                                                                                                                                 |
| W23  | Close Workspace of the populated one of two, the other empty                                                  | Commit                                                                     | The window shows the empty sibling (EM1); the session survives                                                                  | `closing_the_populated_workspace_shows_the_empty_one`                                                                                                                                                                                                                                                                                                                           |
| W24  | Close Workspace on the session's only workspace                                                               | Command or menu                                                            | Unavailable; core refuses `LastWorkspace` if reached                                                                            | `the_last_workspace_cannot_be_closed_as_a_workspace`                                                                                                                                                                                                                                                                                                                            |
| W25  | W moves to another session (another client), with tabs or empty                                               | Batch                                                                      | The window leaves W; core leaves W alone in its new session                                                                     | `a_populated_workspace_moved_away_is_left`, `an_empty_workspace_moved_away_is_left_and_kept`                                                                                                                                                                                                                                                                                    |
| W26  | Windows on different workspaces of one session                                                                | Close Session from one                                                     | Existing Q6 follow-up for that window; others take the session-gone path; memory pruned                                         | `closing_a_session_shown_on_two_workspaces`                                                                                                                                                                                                                                                                                                                                     |
| W27  | Windows on different workspaces of one session                                                                | Quit, then restore                                                         | Each returns to its workspace with its own navigation                                                                           | `two_windows_on_two_workspaces_restore_their_workspaces`                                                                                                                                                                                                                                                                                                                        |
| W28  | Saved memory names closed or moved workspaces                                                                 | Restore                                                                    | Those entries drop; duplicates collapse; the shown navigation keeps its fallback                                                | `restore_drops_stale_workspace_memory`                                                                                                                                                                                                                                                                                                                                          |
| W29  | Quake stash on W                                                                                              | W closes, checkpoint, then summon                                          | Shows its remembered or first workspace with tabs                                                                               | `a_quake_stash_on_a_closed_workspace_shows_another`                                                                                                                                                                                                                                                                                                                             |
| W30  | Window shows an empty workspace it selected (EM1)                                                             | Settle, Quit, checkpoint                                                   | No emptied close; Quit and checkpoint save it                                                                                   | `selecting_an_empty_workspace_shows_the_empty_state`                                                                                                                                                                                                                                                                                                                            |
| W31  | Close confirmation, reservation, or arrangement showing                                                       | Workspace switch                                                           | Refused by the guard                                                                                                            | `workspace_switches_respect_the_structural_guard`                                                                                                                                                                                                                                                                                                                               |
| W32  | PR B: sidebar toggle, position change, config reload, or a width change that docks or undocks it              | While a terminal has focus                                                 | Focus, process, and mouse mapping preserved; only the size changes                                                              | `sidebar_changes_keep_focus_mouse_mapping_and_processes`                                                                                                                                                                                                                                                                                                                        |
| W33  | PR B: switcher popover, sheet, or session popover open                                                        | Toggle, reload, or its anchor goes                                         | The popover closes; focus returns to the terminal                                                                               | `a_popover_whose_anchor_goes_closes_cleanly`                                                                                                                                                                                                                                                                                                                                    |
| W41  | PR B: a workspace row drag in progress                                                                        | The dragged workspace closes or moves away, or the window switches session | The drag cancels; nothing moves                                                                                                 | `a_workspace_drag_cancels_when_its_workspace_goes`                                                                                                                                                                                                                                                                                                                              |
| W42  | PR B: a workspace row drag in progress                                                                        | Its anchor row closes before the drop                                      | The drop lands at the anchor's former neighbour or the end, by ID; no other workspace moves                                     | `a_workspace_drop_whose_anchor_went_lands_by_id`                                                                                                                                                                                                                                                                                                                                |
| W34  | A commits `LeaveWorkspace` (commit held); Linux; an unviewed session                                          | Close Window in B                                                          | A still counts as open, so B closes without becoming Quit                                                                       | `a_window_leaving_a_workspace_still_counts_as_open`                                                                                                                                                                                                                                                                                                                             |
| W35  | Emptied close assessed in A                                                                                   | Quit confirming in B, then A reaches its commit                            | `LeaveWorkspace` commits and follows, and the Quit waits for its hold; `Detach` and `Session` behave as today                   | `a_leave_workspace_commits_while_a_quit_confirms`                                                                                                                                                                                                                                                                                                                               |
| W36  | Linux last window on the session's only workspace; an unviewed session                                        | Its last tab closes; the emptied close becomes Quit                        | The Quit saves neither the emptied session nor the window; the unviewed session is saved                                        | `a_linux_emptied_quit_saves_neither_the_emptied_session_nor_its_window`                                                                                                                                                                                                                                                                                                         |
| W37  | Emptied close of the session's only workspace assessing, assessed, or committing (held)                       | Checkpoint                                                                 | Saves neither the session nor the window; other sessions are saved                                                              | `a_checkpoint_while_an_emptied_close_assesses_saves_neither`, `a_checkpoint_after_an_emptied_close_assessed_saves_neither`, `a_checkpoint_while_an_emptied_close_commits_saves_neither`                                                                                                                                                                                         |
| W38  | Emptied close of the session's only workspace refused while a tab installs into it (install held)             | Checkpoint                                                                 | Saves the window and its session; a refused close is not departing                                                              | `a_checkpoint_before_a_refused_emptied_close_installs_saves_the_window`                                                                                                                                                                                                                                                                                                         |
| W39  | Linux last window, an unviewed session; its session closes elsewhere (switch target or refused close)         | Its emptied close cannot be assessed                                       | Session-gone path: the last-window rule still applies, so the window quits rather than removing itself (session row 7)          | `a_target_closed_after_the_commit_on_linux_quits_and_keeps_its_old_session`, `a_refused_emptied_close_whose_session_then_closes_on_linux_quits`, plus the Linux branches of the session-gone tests                                                                                                                                                                              |
| W40  | Linux last window, own Close Session follow-up could open nothing                                             | Its gone session owes an emptied close                                     | The Quit replaces that close, which no longer runs first and removes the window; the notice stays (Q6, row 35)                  | `a_close_session_follow_up_without_a_shell_on_linux_quits_with_its_notice`                                                                                                                                                                                                                                                                                                      |

## Observable success

- With one workspace and the sidebar hidden, every window looks and
  behaves as on `main`, plus the sidebar button.
- New Workspace from the session menu, palette, or sidebar shows a fresh
  shell in a new workspace; the switcher appears in every window on that
  session whose sidebar is hidden.
- Two windows on one session can show different workspaces and switch
  independently; each returns to its own tab and pane in each workspace.
- Closing a workspace's last tab lands on the previous workspace, or on
  an empty one's empty state when no other holds tabs; closing the last
  tab of a session's only workspace closes the window as today.
- The sidebar, its button, the switcher, and the popovers match the
  revised mockup on both sides and at every tab position, in fullscreen,
  Quake, narrow windows, and on Linux.
- Dragging a row reorders the session's workspaces in every window on it.

## Failure paths and risks

| Risk                                                       | Handling                                                                                                              |
| ---------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| An emptied detach wins over L1                             | Core decides `LeaveWorkspace` under the lock, at the commit's freshness check; W11, W12                               |
| A selected empty workspace triggers an emptied close       | Only removal, a gone workspace, or a gone session owes one; W30                                                       |
| A Linux last window quits when it only leaves a workspace  | Quit conversion only for `Detach` or `Session` effects; W13                                                           |
| Decorations flip as the sidebar shows                      | Title row ownership reads the configured tab position and platform                                                    |
| Geometry regressions in today's tab positions              | A hidden sidebar must equal today's `ChromeLayout` but for the corner slot; tests compare across positions and frames |
| A docked sidebar squeezes the terminal                     | Undock under 384 points; tiny-window tests                                                                            |
| A drop moves the wrong workspace after a concurrent change | Moves name the workspace and its anchor by ID; W41, W42                                                               |
| Status wakes too often (PR C)                              | Coalesced: one pending wake, one worker read at a time                                                                |
| Downgrade reads several entries per session                | Cap stays 256; older validation accepts duplicates                                                                    |
| Overlap with the #81/#407 attention work                   | PR C rebases onto whichever lands first and reuses its attention source if one exists                                 |
| Overlap with #44's ID migration                            | Mechanical rebase                                                                                                     |

## Non-goals

- Tree style (P4), nested tabs (N2), opening a workspace in a new
  window, dragging a workspace to another window, moving tabs onto
  workspaces (#32), per-window sidebar position, default key bindings,
  and server-mode workspace views (#45).
- Preserving a selection across a terminal resize (settled decision 9,
  tracked on [#373][issue-373]).

## Verification

| Concern                                                                                                                                                                                                 | Evidence                                                                                                                                                                                                                                                                                      |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Core: workspace close consent, last-workspace refusal, stale tickets; `LeaveWorkspace` resolution order and commit                                                                                      | `huterm-core` lifecycle tests, one per resolution branch                                                                                                                                                                                                                                      |
| Atomic create-with-shell rollback                                                                                                                                                                       | `huterm-core` mux test                                                                                                                                                                                                                                                                        |
| Commands: availability, defaults, picker filtered to the session                                                                                                                                        | Catalog and palette tests                                                                                                                                                                                                                                                                     |
| Switching keeps shells alive and restores per-workspace navigation and undo                                                                                                                             | Tier 2 window tests with shell acknowledgements                                                                                                                                                                                                                                               |
| One view's workspace change leaves another view's workspace, tab, pane, and selection alone (acceptance 3)                                                                                              | Tier 2 two-window test asserting each explicitly, plus a `check-viewers.ts` step through `invoke-in select_workspace` on macOS and Linux                                                                                                                                                      |
| Interleavings W1 to W38                                                                                                                                                                                 | One test per row, ordered by worker hooks and sequence waits, never sleeps                                                                                                                                                                                                                    |
| Navigation memory save, strict restore, cap, older files                                                                                                                                                | `huterm-state` schema tests; tier 2 restore tests                                                                                                                                                                                                                                             |
| Geometry: 2 sides × 5 tab positions, hidden, docked, undocked by width, fullscreen auto-hide, Quake, notch shelf, Linux frame with leading and trailing buttons, small windows, overflow (acceptance 1) | Pure `ChromeLayout` and `WorkspaceChrome::resolve` tests beside `all_placements_share_nonoverlapping_terminal_and_tab_bounds`; a hidden sidebar equals today's layout but for the corner slot                                                                                                 |
| Mouse mapping and sidebar resize with a panel beside the tab column                                                                                                                                     | Extend `application_mouse_coordinates_follow_all_tab_placements` and the sidebar resize tests                                                                                                                                                                                                 |
| Sidebar changes and reload keep focus, mouse coordinates, and processes (acceptance 2)                                                                                                                  | W32 tier 2 test; native steps in `smoke:macos-palette` and `smoke:linux-palette`: real key input reaches the same shell (same pid) after a toggle, a PTY mouse report lands on the expected cell after a toggle, sidebar rect, row click switches, switcher opens its popover, keyboard focus |
| Corner controls on both sides and every tab position; session switcher at either end                                                                                                                    | Pure placement tests; window tests opening each popover from each host                                                                                                                                                                                                                        |
| Drag reordering: order in every window, Escape cancels, W41, W42                                                                                                                                        | Window tests driving the drag through simulated mouse input                                                                                                                                                                                                                                   |
| Invalidation: a hidden workspace's count changes; another window's badge updates with its popover open                                                                                                  | Window tests                                                                                                                                                                                                                                                                                  |
| Popovers: keyboard, live updates, GPUI rendering under native menus                                                                                                                                     | Window tests on the switcher and session popovers                                                                                                                                                                                                                                             |
| Status (PR C): roll-up order, coalescing, two windows with different baselines, a publication during a read                                                                                             | Core and window tests                                                                                                                                                                                                                                                                         |
| Settings section, schema, coverage                                                                                                                                                                      | `settings.rs` section tests, schema check, coverage test                                                                                                                                                                                                                                      |
| Visual fidelity to the picked mockups                                                                                                                                                                   | Screenshots beside the mockups in each PR; `ux-review` (Codex computer use, scratch bundle) briefed with the revised mockup                                                                                                                                                                   |

## Open questions

- A window with no title row and no tab bar (Linux server decorations,
  Quake, or fullscreen, with one tab and `tabs.always_show = false`)
  has nowhere to hold the sidebar button. Recommended: accept it, as
  mockup C1 did for the dropdown; `toggle_sidebar` and
  `open_workspace_switcher` still work from the palette.

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
- 2026-11-07, after the UX review of PR A: an emptied window leaves its
  workspace whenever its session holds any other workspace, empty or
  not, instead of only when another workspace holds tabs. With one
  populated and one empty workspace, Close Workspace on the populated one
  showed the empty sibling (W23) while closing its last tab with Close
  Tab closed the window and deleted the session with the empty workspace
  the user had kept. `Detach` and `Session` now apply only to a session's
  only workspace. W10 splits into W10 (an empty sibling exists) and W10a
  (the only workspace), W11's event becomes a sibling workspace
  appearing, and W12 splits into W12 (the only sibling closes or moves
  away) and W12a (the only populated sibling empties, which keeps the
  leave). The follow-up target rule is unchanged.
- 2026-11-07, after the UX re-check of PR A: a tabless session a staying
  window shows is now captured. With the rule above, a window can be left
  on a session whose workspaces are all empty, and Quit dropped that
  session because capture skipped every tabless session. Restore already
  gives a restored tabless session one fresh tab in its first workspace,
  so its workspaces and names survive and the window reopens on the
  workspace it showed.
- 2026-11-07, after the code review of PR A: capture keeps a tabless
  session only when a saved window or Quake stash shows it, replacing a
  first rule that kept any tabless session an attachment held. That rule
  saved a session the user had just ended: a Linux last window whose
  emptied close of the session's only workspace became Quit still held
  its attachment, so the Quit saved the session and the window, and the
  relaunch brought both back with a fresh shell. A departing window, one
  that shows no tab, whose emptied close is owed, assessing, or
  committing or became the Quit under way, and whose session holds no
  other workspace, is now left out of the saved layout, and the desktop
  passes the sessions the saved layout shows to the runtime capture as
  its keep set, for Quit and checkpoints alike (W36, W37). A tear-off's
  destination session, whose window is not yet a saved record, is no
  longer saved either. A window settled on an empty workspace (EM1) is
  not departing, so it and its session are still saved, and neither is
  one whose emptied close the runtime refused, which stays saved while
  its tabs install.
- 2026-11-07, after Linux CI on PR A: removing the pre-assessment
  conversion also removed the only Quit conversion of the session-gone
  path, whose emptied close core cannot assess (`UnknownAttachment`, or a
  refusal whose session then closed), so a Linux last window removed
  itself and stranded unviewed sessions (W39). That close now applies the
  last-window rule before removing the window; with its session gone, no
  sibling workspace can make a stale answer. A Close Session follow-up
  that becomes Quit also clears the emptied close its gone session owed,
  which otherwise ran before the deferred Quit and removed the window and
  its notice (W40). The emptied close's freshness check enters the commit
  stage's worker hook, so a Linux Quit queued behind it commits second.
- Switch Workspace… on the empty state opens the `select_workspace`
  palette prompt in PR A; PR B's workspace menu replaces it. Dismissing
  the prompt, or any overlay or menu, in a window that shows the empty
  state returns focus to the button that opened it, else New Tab, so Tab
  keeps cycling the buttons.
- W16 when the Quit is confirmed rather than cancelled: the Quit absorbs
  an owed `LeaveWorkspace`, so the emptied workspace is saved and
  restored empty, and the window reopens on it in the empty state. This
  is accepted; the window is not departing, since its session holds
  another workspace.
- A Quake stash that follows its closed or moved workspace (W29) also
  moves its saved layout's active tab, tab history, focused panes, and
  remembered list to the followed workspace's, so a checkpoint or Quit
  before the summon keeps that workspace's remembered selection. W29's
  test relaunches from that checkpoint as well.

### PR B deviations

Recorded while implementing the 2026-11-08 revision:

- The user-facing "sidebar" keeps the internal name workspace panel
  (`workspace_chrome.rs`, `workspace_panel.rs`, `WorkspaceChrome`, the
  `workspace_*` view fields, and `workspace_panel_*` in saved state), because
  `sidebar_width`, `resize_sidebar`, and saved `sidebar_width` already name
  the vertical tab column. User documentation now calls that column the tab
  column.
- The corner controls' order on the right is the title-bar session control,
  then the switcher, the sidebar button, and the menu button; in a vertical
  column's new-tab row the button sits just inside the menu button.
- In a left tab column with a title row the sidebar button sits in the
  column's top strip after the traffic lights, and the switcher takes a row
  above the tabs; without a title row both share that row.
- The switcher's popover is non-modal: a press outside closes it and goes on
  to what it hit, as a press inside a menu opened from it does not. A press
  on the switcher closes it.
- Opening the popover, or the sheet, focuses the shown workspace's row, so
  the keyboard works at once and Escape (`sidebar_exit`) closes it.
- The sheet has no pin; the sidebar button, Escape, an outside press, or a
  workspace pick closes it.
- The drop target follows the rows' painted midpoints, and a release
  re-reads it, so a drop whose anchor went lands before the row now under
  the pointer (W42). A drag also cancels when its rows stop drawing.
- Rows drop the window badge: the first design's workspace menu marked
  workspaces other windows showed, and nothing shows that now.
- `toggle_sidebar` and the `sidebar_*` commands are always available;
  `focus_sidebar` is unavailable while the sidebar is hidden in a wide
  window.

Recorded while implementing the first PR B design, stacked on PR A. The
2026-11-08 revision replaced the panel, bar, dropdown, workspace menu,
session placements, and `[workspaces]` settings, so entries about those
are history. `Touched::chrome`, R1 rows, the theme-derived background,
and W33's rule that running a command closes open popovers carry over.

- Settings hides Position and Start collapsed under `show = "never"`
  through `x-huterm-visible-when`, as every dependent setting does, rather
  than dimming them; the model has no dimmed state. Their values stay in
  the file.
- Panel keys use a `WorkspacePanel` key context with eight
  `workspace_panel_*` commands (next, previous, first, last, show, rename,
  menu, exit), bound like the `Panel` and `menu` contexts, so they appear
  in the keybindings editor and can be rebound. Both arrow pairs move, so
  one set of bindings serves a vertical panel and a horizontal bar.
- `toggle_workspace_panel` and `focus_workspace_panel` are unavailable
  under `never`; `open_workspace_menu` stays available, since the empty
  state's Switch Workspace… opens it.
- The workspace menu's panel row reads Show Workspace Panel while the
  window is collapsed and Collapse Workspace Panel while a panel or bar
  shows, and is absent while a small window shows the dropdown on its own
  (SW1), where neither would change what shows.
- Rows show name and tab count (R1); the status dot is PR C.
- The bar's chips scroll with floating arrows; they draw no slim position
  indicator.
- `Touched` gains `chrome`, the sessions whose workspace list, workspace
  names, or tab counts changed; a rearrangement counts only when a
  workspace's tab count changed.
- A top bar over a title row draws inside the title row element, which
  already carries the row's gestures, window buttons, and corner controls.
- The panel's collapse buttons use Lucide's `panel-left-close`,
  `panel-right-close`, `panel-top-close`, and `panel-bottom-close`; the
  menu's panel row uses `panel-left-open` and `panel-left-close`.
- A new window counts the band in its initial size only under
  `show = "always"`, since a new window's session has one workspace.
- A window restored from a file saved before PR B has no saved collapse
  state and shows the panel; `collapsed` applies to new windows only.
- A focused workspace chip holds a fullscreen bar that hides with the tab
  bar revealed, as an open menu does, so keyboard focus never sits on a
  hidden control.
- W33: running any command closes an open menu, so collapsing the panel
  from the palette or the menu closes a session or workspace menu opened
  from it; a host that goes while its menu stays open, as when a narrow
  window turns the panel into the dropdown, hands the menu to the other
  host.
- Open question for the maintainer: with the dropdown showing and a
  one-tab workspace, a window without a title row (Linux server
  decorations, Quake, fullscreen) shows no workspace control, as mockup
  C1's note describes; the palette and `open_workspace_menu` still reach
  workspaces.

[issue-11]: https://github.com/jimeh/huterm/issues/11
[issue-26]: https://github.com/jimeh/huterm/issues/26
[issue-27]: https://github.com/jimeh/huterm/issues/27
[issue-373]: https://github.com/jimeh/huterm/issues/373
