# Session switching

Status: approved. Plans [issue #26][issue-26] (parent [#11][issue-11]).
The maintainer picked the mockups and settled the open questions on
2026-11-06; see [Settled decisions](#settled-decisions).

A window can switch to another session, come back to it with its own
navigation intact, and choose a session that another window already shows.
New Window keeps creating a fresh session. A session control at the far
right of the title bar shows the window's session and opens a session menu.
Embedded mode only: the server-mode New Window flow is designed here and
built with [#45][issue-45].

Builds on [#20][issue-20] (session ownership), [#21][issue-21]'s embedded
half ([session close](session-close.md)), [#22][issue-22]
([terminal viewers](terminal-viewers.md)), [#23][issue-23] (command
catalog), [#97][issue-97] (client window model), [#98][issue-98]
([hierarchy projection](hierarchy-projection.md)), and [#42][issue-42]
([session restore](session-restore.md)). [#27][issue-27] (workspace panels)
and [#32][issue-32] (workspace moves between sessions) build on it.

## Current state

Observed on `main` at `0a963ed1`:

- Each window creates a private session and workspace on its first spawn
  (`WindowModel::attach`). `new_window` documents "its own session and
  workspace", so embedded New Window already creates a fresh session.
- `Mux::retarget_attachment` switches an attachment atomically, revokes its
  viewers, and keeps both sessions, including with zero views.
  `attach_session`, `detach_session`, and `CloseRequest::Session` exist.
- `WorkspaceView::adopt_attachment` records an attachment, session, and
  workspace in the window model and runs a full reconcile, which removes
  views whose tabs left the attached session and installs the tabs it does
  not show. Restore's `reopen` and `apply_reopen` already re-apply each
  tab's focused pane, the activation history, and the active tab once
  installs are idle. The palette smoke's `attach-window` uses the same path.
- `HierarchyState` carries sessions, workspaces, tabs, and names, but no
  attachments. [Session close](session-close.md#attachment-events-move-to-26)
  moved attachment events here with the shape `AttachmentCreated`,
  `AttachmentRetargeted`, and `AttachmentDetached`.
- `rename_session` exists (Runtime scope, palette prompt, blank restores the
  automatic name). There is no create, select, or clear-name session
  command, and nothing in production retargets an attachment.
- The window menu button (`menu_button_placement`) already solves "title
  bar if there is one, else the tab bar": the macOS title strip in every
  tab position, the Linux drawn title row, the end of a top or bottom bar,
  or beside `+` in a vertical column; hidden with an auto-hidden bar.
- A reconcile that removes a window's last view owes an emptied close
  (`note_emptied`); core refuses it with `ViewNotEmpty` while the window's
  workspace holds tabs.
- `maybe_exit` quits only when no window, no session, and no quake
  registration remain. A session left unviewed after its last window
  closes keeps the process alive with no way back to it
  (session-close.md defers "reaching an unviewed session" to #26).

## Scope

In scope:

1. Attachment events in the hierarchy stream and attachments in
   `HierarchyState` and its snapshot.
2. Session commands in the catalog: create, select, rename (exists),
   clear name, and open the session menu.
3. Switching a window's session with per-session navigation memory.
4. The title-bar session control and its session menu, in every tab
   placement, fullscreen, Quake, narrow windows, and on Linux.
5. Choosing a session another window views, from the menu.
6. Failure handling: create failure, stale selection, and the window's
   own session closing during a switch.
7. The server-mode New Window design (not built).

Out of scope, with owners:

| Deferred                                                                                                                                                       | Owner                     |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------- |
| Server-mode New Window (existing sessions plus Create Session) and the disconnected state's transport                                                          | #45                       |
| Workspace switching, workspace panel, collapsed workspace dropdown, and a sidebar host for the session control with a `title_bar` / `sidebar` / `both` setting | #27                       |
| Moving workspaces between sessions, session drop targets                                                                                                       | #32                       |
| Persisting a window's navigation memory for sessions it is not showing                                                                                         | Follow-up (see decisions) |
| Smallest/largest sizing for a terminal shown at two sizes                                                                                                      | [#207][issue-207]         |

## Decisions

### Attachments join the hierarchy

- `huterm-protocol`: add `HierarchyEvent::AttachmentCreated { attachment,
session }`, `AttachmentRetargeted { attachment, session }`, and
  `AttachmentDetached { attachment }`. `HierarchyState` keeps an
  `attachments: BTreeMap<AttachmentId, SessionId>` with
  `session_attachments(session)`. `SessionClosed` drops that session's
  attachments, as `Mux::close_session` invalidates them. `from_sessions`
  takes the attachment map, and `digest` covers it. `Touched` gains an
  `attachments` flag. A retarget to an unknown session or of an unknown
  attachment requires resync, like other reducer conflicts.
- `huterm-core`: `attach_session`, `retarget_attachment` (only when the
  session changes), and `detach_session` emit their event under the same
  lock as the change. `subscribe_hierarchy` snapshots the attachment map.
  Restore's attach emits `AttachmentCreated` too, so subscribers follow.
- The reducer refuses, leaving state unchanged and requiring resync, a
  foreign-scope attachment, a duplicate creation, a retarget or detach of an
  unknown attachment, and an attachment that names an unknown session. A
  snapshot with such a reference is rejected the same way.
- Consumers: `Touched::attachments` names the sessions whose attachment
  set changed. `windows_to_visit` visits every window when it is set, which
  is cheap because attachment changes are rare. Session renames already mark
  names changed; the control and an open session menu join the title
  consumers (`TitleConsumers`, `refresh_title_consumers`), so names, counts,
  and badges update without polling.
- These are in-process events. The wire format stays [#44][issue-44]'s.

### Commands

| ID                           | Scope       | Arguments                                      | Effect                                                                                                                         |
| ---------------------------- | ----------- | ---------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `new_session`                | Window      | `name` (optional text, no prompt)              | Creates a session, workspace, and shell, then switches this window to it.                                                      |
| `select_session`             | Window      | `session` (required, prompt)                   | Switches this window to the session.                                                                                           |
| `rename_session`             | Runtime     | existing                                       | Unchanged.                                                                                                                     |
| `clear_session_name`         | Runtime     | `session` (optional, defaults to the window's) | `rename_session` with `None`; unavailable when the session has no custom name.                                                 |
| `open_session_menu`          | Window      | none                                           | Opens the session menu at the control, or at the window menu's fallback anchor when the control is hidden; first row selected. |
| `open_session_in_new_window` | Application | `session` (required, prompt)                   | Opens a window attached to the session without creating one; a failed open detaches only its own attachment.                   |
| `close_session`              | Runtime     | `session` (optional, defaults to the window's) | Assesses and closes the session with consent (settled decision 5).                                                             |

No default key bindings (settled decision 8). All of them appear in the
palette; `select_session` uses the existing session argument picker, which
filters by name, so long session lists stay reachable by keyboard. Each
picker target is a stable `SessionId`, never a position.

### Switching keeps both sessions and this view's navigation

`WindowRecord` gains `memory: Vec<SessionMemory>`, one entry per session the
window showed and left. Each holds the workspace, active tab, activation
history, each tab's focused pane (the restore `Focus` shape), and the
window's layout history for that session, so Undo never reaches a session
the window no longer shows. It is view state: never shared with other
windows, and pruned when its session closes. Everything but the layout
history is saved (see [Saved navigation memory](#saved-navigation-memory)),
so a restart returns each session to the workspace, tab, and pane the
window last showed there.
View-local pane zoom and per-tab pane focus history reset on a switch, as
they do when a view is rebuilt today.

The switch runs in four phases. `switch` on `WorkspaceView` holds the
phase, and the window counts as busy from the first phase to the last.

1. **Guard.** Refused, with no change, while the window is busy, closing,
   quitting, showing a close confirmation, reserved by a cross-window
   transfer or an arrangement (the full `check_available` structural
   guard), or during teardown; a no-op for the current session. Active
   reorder, pane, and divider drags and incoming drag previews are
   cancelled, as `new_tab` does.
2. **Prepare and commit (worker, one Mux lock).** Validate the attachment
   and the target session. Resolve the target workspace: the remembered one
   if it still belongs to the session and holds a tab, else the first
   workspace that holds a tab. Only when no workspace holds a tab, open a
   shell tab in the first workspace (creating one if the session has none).
   Every resource this switch created is recorded; on any failure before
   commit only those are closed, never the session, and the attachment is
   untouched. Then `retarget_attachment` commits. `new_session` creates the
   session, workspace, and shell first and closes the new session on any
   failure before its commit. Shell-producing switches count in
   `pending_spawns`, so Quit waits for them. Returns the committed sequence.
3. **Install.** The completion waits for the projection, stores the old
   session's `SessionMemory`, adopts the new session in the model, and runs
   reconcile with removals allowed and the emptied edge suppressed for this
   reconcile: old views drop (view destruction, not close), and the new
   session's tabs install. Callbacks from the previous session's views are
   ignored by tab and viewer generation, as replacement already does.
4. **Settle.** Once installs are idle, the remembered focus applies through
   the code `apply_reopen` uses (with no memory the first tab is active),
   the phase clears, and a checkpoint may run. Checkpoints defer while any
   window is switching, as they defer for `cross.reserved`.

Terminal viewports are runtime state, so each terminal shows the same
scroll position as every other viewer. Selection is view state and is lost
with the dropped view.

What failure leaves behind, by commit point:

| When                         | Cause                                                                               | Result                                                                                                                                                                                                                                  |
| ---------------------------- | ----------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Before commit                | Target closed or foreign                                                            | No change; notice "Session no longer exists".                                                                                                                                                                                           |
| Before commit                | The window's own session closed, so its attachment is invalid (`UnknownAttachment`) | No switch; the window takes the existing session-gone path once the phase clears.                                                                                                                                                       |
| Before commit                | Shell spawn failed                                                                  | Created resources closed; notice "Cannot create session: `error`" or "Cannot switch session: `error`"; window unchanged.                                                                                                                |
| After commit, before install | Target closed                                                                       | The attachment is invalidated; the window has nothing to show and takes the existing `UnknownAttachment` removal. The original session survives and stays in every menu. The preservation guarantee covers only failures before commit. |
| Install                      | Some subscriptions fail                                                             | Existing install failure handling: "Cannot show tab", retried on the next batch.                                                                                                                                                        |
| Install                      | Every subscription fails                                                            | The window keeps its attachment and shows its install notices; the emptied edge stays suppressed until the next batch that touches the workspace, as for any failed install.                                                            |
| Any                          | Teardown                                                                            | The worker sees the termination gate and changes nothing; a completion applies nothing.                                                                                                                                                 |

A tab whose shell exits while no window shows it records no exit-close,
because exit-closes come from a view's exit transition. A view installed
later sees the exited state without a transition and keeps the exited tab,
as a view installed after exit does today. This is deliberate, matches
[#22][issue-22]'s rule, and is documented with the switch.

Every other lifecycle rule stays: Close Window detaches when another
attachment holds the session and closes it otherwise; a window whose last
view goes away closes; switching never closes or detaches a session.

### Saved navigation memory

`huterm-state`'s `WindowLayout` gains `remembered: Vec<SessionNavigation>`,
each with the session, workspace, active tab, tab history, and focused
panes, the same fields the window saves for the session it shows. It is an
optional, additive field (`serde(default)`), which the schema's rules allow
without a version bump: an older build ignores it. It is bounded by
`MAX_SESSIONS` per window and validated like the window's own fields.
Capture writes the window model's memory, and a memory change moves
`WindowModel::revision`, so a checkpoint follows a switch. Restore scopes
each entry into the runtime, drops one whose session did not come back,
filters its tabs and panes to those that did (as it already does for the
shown session), and applies it the first time the window switches to that
session. Layout history stays unsaved, as it is for the shown session today.

### Restore with two windows on one session

Switching makes saved layouts with two windows on one session possible, and
restore cannot yet reopen them: it allocates one attachment per session
(`restore.rs` worker), skips later saved windows of a session it already
planned, and `close_unopened` closes the session of a window that failed
to open. This PR fixes all three: one attachment per saved window (and per
Quake stash), activation by saved-window identity, and failed-window
cleanup that detaches only its own attachment, closing the session only
when no other restored window or stash holds it.

Quake stashes hold an attachment with no native window, so "shown in N
other windows" counts window records, not attachments. A stash whose session
closed before its first summon is dropped, and that summon opens a fresh
profile session.

### A second window chooses a viewed session

The session menu marks sessions that other attachments view (from the
projection's attachments, excluding this window's). Selecting one switches
this window to it, so two windows show it, each with its own selection,
scroll intent, and navigation; size follows the controlling viewer, as #22
defined. "Open in New Window" (settled decision 3) opens a window
attached straight to the session, without creating a throwaway session.

### The session control is one component with several possible hosts

Decided with the maintainer on 2026-11-06: the switcher will move into the
workspace sidebar, or show in both places, once [#27][issue-27] lands, and
some users will never use workspaces. So:

- The control is a reusable component, not part of the title row. Its
  state (`SessionControlState`: the window's session, display name, whether
  the name is automatic, whether its menu is open, and later a disconnected
  flag) is derived from the projection and the window model in one
  function. Its commands (`open_session_menu`, `select_session`, and the
  rest) never depend on where it is drawn. The generic trigger (icon,
  truncating label, chevron, open state, accessible name) goes in
  `huterm-ui`, which stays free of catalog and theme types; `huterm-gpui`
  feeds it tokens and maps its click to `open_session_menu`.
- Placement is one resolved value, `SessionControlPlacement`, computed
  beside `menu_button_placement` and shared by rendering, title-row hit
  testing, pane-drop exclusions, menu anchoring, and smoke state. It
  carries the control's measured width. `TabStrip::new`'s boolean menu slot
  becomes trailing widths, so the strip's overflow, scrolling, insertion
  slots, and drop targets account for the control; `title_row_layout`'s
  trailing exclusion and title padding take it too; `split_new_tab_row`
  learns a second square control. Collapse to the icon depends on the
  host's available width (sidebar, notch shelf, Linux button groups), not
  the window width. #26 ships
  the title-bar host only. #27 can add a `Sidebar` variant and a
  `title_bar` / `sidebar` / `both` setting without changing the component,
  the menu, or the commands.
- The session menu anchors to whichever host opened it, so a second host
  reuses it unchanged.
- Nothing in #26's UI mentions workspaces. With one workspace per session,
  which is every session today, the control, the menu, and the prompts
  talk only about sessions, tabs, and windows.

### Where #26 draws it

Placement follows the window menu button, one slot further in, so the menu
button keeps the rightmost slot it was given in the UI overhaul:

| Window                                                                      | Control                                                                                                                                                                                                                                                                            |
| --------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| macOS windowed, any tab position                                            | Title strip, left of `⋯`. With `titlebar` tabs and a shown bar, at the bar's end left of `⋯`. Left column: the terminal column's title row, top right. Right column: beside `⋯` in the sidebar's top strip, where `sidebar_title_row` already draws `⋯`, truncating to the column. |
| Linux, drawn title row                                                      | Left of `⋯`, before the window buttons.                                                                                                                                                                                                                                            |
| Fullscreen, Quake, Linux server decorations; top or bottom bar              | End of the tab bar, left of `⋯`, where the title row's right end would be.                                                                                                                                                                                                         |
| Same, left or right column                                                  | Icon-only, in the new-tab row between `+` and `⋯`. The top of the column stays free for #27's sidebar host.                                                                                                                                                                        |
| Bar hidden (auto-hide, `always_show = false` with one tab and no title row) | Hidden; `open_session_menu` and the palette still work.                                                                                                                                                                                                                            |
| `window.session_button = false` (new, default true)                         | Hidden everywhere. With `menu_button = false` and the control shown, the control takes `⋯`'s slot.                                                                                                                                                                                 |

Look (mockup pick): a 26-point ghost control with a layers icon, the
session's display name, and a chevron, in the title row's text colour; an
automatic name uses the dim colour so a custom name stands out. It shows an
open state while its menu is open, as the menu button does. The name
truncates with an ellipsis at a maximum width; below a window width
threshold it collapses to the 26-point icon. Its accessible name is
"Session: `name`", and the tooltip shows the full name when truncated. The
title row's drag and double-click gestures exclude its bounds, as they
exclude `⋯` (`title_row_layout`), and pane drops onto the empty row exclude
it too.

### The session menu

Built from a `MenuSpec`, like the window and tab menus, so it honours
`macos_native_menus` (settled decision 10): Huterm's popover by default on
Linux and when the setting is off, a native menu otherwise. It gets its own
anchor, the control's bounds from `SessionControlPlacement`; `render_menu`
today anchors every non-pointer menu at the window menu button. Anchored
under the control, right-aligned, and opening upward from a bottom bar:

1. One row per session in runtime order: a check on this window's session,
   the display name (dim when automatic), a trailing count ("3 tabs"), and,
   when other windows show it, a window badge with their count. A session
   whose every terminal has exited shows "exited".
2. Separator, then New Session, Rename Session…, and Clear Name (present
   only while the window's session has a custom name).

`MenuSpec::Item` gains a secondary detail and a dim tone. The native
lowering keeps the check and the name, and puts the detail in the item's
subtitle where AppKit supports it, otherwise drops it. A native menu is a
snapshot, as native menus already are; Huterm's popover updates live.

The projection holds structure, not lifecycle, so "exited" comes from a
worker that reads each session's terminal status from the viewer registry
when the menu opens (no runtime round trip, as `capture_runtime` reads
directories). The menu opens at once and adds the tags when the read
answers.

Keyboard: `open_session_menu` selects the first row; arrows, Home and End,
typed letters, Enter, and Escape behave as in the window menu. Escape and
a pick return focus to the terminal. The menu reads the projection, so a
session created, renamed, or closed elsewhere updates an open menu; a
session that vanishes while selected moves the selection to its neighbour.

States:

- Only one session: its row, then a dim hint "No other sessions".
- Exited: the row's "exited" tag; selecting it switches, and the window
  shows its exited tabs.
- Disconnected (server mode, design only): the control shows a warning dot
  and "Disconnected"; the menu shows a banner and disables rows and actions.
- Errors: create and stale-selection failures raise a notice and leave the
  window as it was.

### Rename

Rename Session… runs `rename_session`, which collects the name in the
palette's existing argument prompt, prefilled with the current custom name,
as Rename Tab does. Blank restores the automatic name. Clear Name runs
`clear_session_name`. Labels update from `SessionRenamed`.

### Server-mode New Window (design only)

With a server, New Window opens a window whose content is a session chooser:
the server's sessions (name, tabs, other viewers) plus Create Session as the
default, Enter to accept, Escape to close the window. Embedded mode never
shows it. Built with #45, reusing the session menu's rows.

## Observable success

- The title bar shows the window's session name at its right end; renaming
  it anywhere updates every window showing it.
- Two windows can show one session, then switch independently; their
  shells keep running and each window gets its own tab and pane selection
  back when it returns.
- A session nobody views stays alive, shows in every session menu, and
  survives Quit through restore.
- A failed create or a vanished target leaves the window showing what it
  showed, with one notice.

## Failure paths and risks

| Risk                                                   | Handling                                                                                                                                                             |
| ------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| A switch removes every view and fires an emptied close | Install suppresses the emptied edge for its own reconcile; a remembered empty workspace is never chosen while another holds tabs, so core never detaches the window. |
| An owed exit-close for a tab during a switch           | The window's views drop, so it no longer shows the tab; another window may take it, and it stays owed otherwise (existing rule).                                     |
| A shell that exits while unviewed                      | Kept as an exited tab when next shown (see above).                                                                                                                   |
| Input queued in a dropped view                         | Dropped with the viewer, as for any view destruction; the revoked viewer's queue already discards.                                                                   |
| Transfers, arrangements, drags                         | Refused or cancelled by the structural guard in phase 1.                                                                                                             |
| Checkpoint or Quit mid-switch                          | Checkpoints defer while a window switches; shell-producing switches count in `pending_spawns`, and a Quit's teardown gate makes a pending switch change nothing.     |
| Undo after a switch                                    | Layout history moves into `SessionMemory`, so Undo acts only on the shown session.                                                                                   |
| Two windows on one session at restart                  | Restore gives each saved window its own attachment (above).                                                                                                          |
| Quake windows                                          | Switching allowed; the record keeps its profile; stashes count as no window (settled decision 6).                                                                    |
| Accumulating unviewed sessions                         | Shown in every menu; reaching them after the last window closes and ending them follow settled decisions 4 and 5.                                                    |

## Delivery

One PR, with commits landing in this order so each one's tests pass before
the next: attachment events (protocol, core); session commands; switching,
navigation memory, and the phase guard; restore with several windows per
session; the control, its placement geometry, and the session menu; smoke
steps, docs, and the glossary. Settled decisions 3 and 5 each add one
commit, and saved navigation memory adds one beside restore. Release
Please's commit override can split the changelog entry after merge.

## Verification

| Concern                                                                                                                                                                                                    | Evidence                                                                                                                                                                                                                             |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Reducer applies attachment events; refuses foreign, duplicate, unknown, and dangling references without change; `SessionClosed` drops attachments; snapshot and digest cover them; lag resnapshot recovers | `huterm-protocol` hierarchy tests                                                                                                                                                                                                    |
| Core emits each event once, under the lock, and snapshots attachments; restore attach emits                                                                                                                | `huterm-core` mux hierarchy tests                                                                                                                                                                                                    |
| Switching away and back keeps both sessions' shells alive and usable                                                                                                                                       | Tier 2 test: a shell acknowledgement typed in each session before and after each switch                                                                                                                                              |
| Switch and return restores active tab, history, focused pane, and per-session undo                                                                                                                         | Tier 2 window test                                                                                                                                                                                                                   |
| Two windows on one session switch independently without losing PTYs or navigation (acceptance 1)                                                                                                           | Tier 2 test, plus a `check-viewers.ts` step through `invoke-in select_session` on macOS and Linux                                                                                                                                    |
| Create failure and stale selection leave the attachment usable (acceptance 3)                                                                                                                              | Tier 2 tests with the worker hook holding and failing each phase: target closed before commit, source closed before commit, spawn failure on `new_session` and on an empty target, target closed after commit, every install failing |
| No emptied close during a switch; remembered empty workspace falls back                                                                                                                                    | Tier 2 tests                                                                                                                                                                                                                         |
| Queued Close and Quit, checkpoints, transfers, arrangements against a held switch                                                                                                                          | Tier 2 tests                                                                                                                                                                                                                         |
| Restore with two windows on one session, distinct navigation, the active window, one window failing to open                                                                                                | Tier 2 restore tests                                                                                                                                                                                                                 |
| Saved navigation memory round-trips, older files without it load, bounds and dangling entries are refused or dropped, and a switch after restore lands on the remembered tab and pane                      | `huterm-state` schema tests; tier 2 restore test                                                                                                                                                                                     |
| Close Session: consent, own-session switch-away, other windows removed, last session closes the window                                                                                                     | Tier 2 tests                                                                                                                                                                                                                         |
| Open in New Window: shares the session, failure detaches only its attachment                                                                                                                               | Tier 2 tests                                                                                                                                                                                                                         |
| Last window close: macOS reopen lands on the most recent unviewed session; Linux close becomes Quit                                                                                                        | Tier 2 tests per platform cfg                                                                                                                                                                                                        |
| Commands: availability, palette prompts, `clear_session_name` unavailable without a name                                                                                                                   | Catalog and palette tests                                                                                                                                                                                                            |
| Placement for each platform, tab position, fullscreen, Quake, narrow and hidden hosts, both toggles                                                                                                        | Pure placement tests beside `menu_button_placement`; strip geometry tests with the trailing widths                                                                                                                                   |
| Title-row drag and double-click on the empty strip still work, and pane drops exclude the control                                                                                                          | Window tests on `title_row_layout` and drop mapping                                                                                                                                                                                  |
| Menu keyboard, focus return, live updates, vanished selection, native lowering                                                                                                                             | Window tests on the menu view and `MenuSpec` lowering tests                                                                                                                                                                          |
| Native title bar in narrow, fullscreen, and every tab placement, including bottom tabs and both Linux decoration modes and button sides (acceptance 2)                                                     | `smoke:macos-palette` and `smoke:linux-palette` steps: the control's rect, a real click that opens the menu, keyboard activation, and the strip drag; screenshots compared with the picked mockup                                    |
| Accessible name, role, and expanded state                                                                                                                                                                  | The existing accessibility harness if #415 has landed by then; otherwise inspection, recorded as a gap                                                                                                                               |
| Visual and interaction quality                                                                                                                                                                             | `ux-review` through Codex computer use on a scratch bundle                                                                                                                                                                           |

Static inspection, automated runs, and manual visual evidence are reported
separately in the handoff.

## Settled decisions

Picked by the maintainer on 2026-11-06 from the
[mockups](#the-session-control-is-one-component-with-several-possible-hosts):

1. Look: V1, the layers icon, name, and chevron ghost control; P1, the
   session menu built on the window menu component; R1, renaming in the
   palette prompt.
2. Order: the session control left of `⋯`, which keeps the rightmost slot.
3. Open in New Window: a menu row action (the selected row's inline button,
   Cmd/Ctrl-Enter) and an `open_session_in_new_window` command (Application
   scope, `session` argument with prompt).
4. Last window closed while unviewed sessions remain: macOS keeps running,
   and a Dock reopen opens a window on the most recently viewed unviewed
   session, tracked by the desktop across window removal. Linux turns that
   last Close Window into Quit, which keeps the window's own session for
   restore too.
5. Close Session: a consent-gated `close_session` command (Runtime scope,
   optional `session` defaulting to the window's) and a menu row, using
   `CloseRequest::Session`. Closing the window's own session switches the
   window to its most recently viewed other session first, or closes the
   window when none remains. Other windows on the closed session take the
   existing session-gone path. Stashes on it are dropped.
6. Quake windows may switch sessions and keep their profile.
7. Navigation memory is saved (changed from the recommendation): restore
   returns each remembered session to its workspace, tab, and pane.
8. No default key bindings.
9. `[window] session_button`, default true, with schema, Settings row, and
   docs.
10. The session menu honours `macos_native_menus`.

[issue-11]: https://github.com/jimeh/huterm/issues/11
[issue-20]: https://github.com/jimeh/huterm/issues/20
[issue-21]: https://github.com/jimeh/huterm/issues/21
[issue-22]: https://github.com/jimeh/huterm/issues/22
[issue-23]: https://github.com/jimeh/huterm/issues/23
[issue-26]: https://github.com/jimeh/huterm/issues/26
[issue-27]: https://github.com/jimeh/huterm/issues/27
[issue-32]: https://github.com/jimeh/huterm/issues/32
[issue-42]: https://github.com/jimeh/huterm/issues/42
[issue-44]: https://github.com/jimeh/huterm/issues/44
[issue-45]: https://github.com/jimeh/huterm/issues/45
[issue-97]: https://github.com/jimeh/huterm/issues/97
[issue-98]: https://github.com/jimeh/huterm/issues/98
[issue-207]: https://github.com/jimeh/huterm/issues/207
