# Cross-window tab and pane drags

Status: in progress. Delivers
[#31](https://github.com/jimeh/huterm/issues/31) under the
[workspaces plan](workspaces-windows-tabs.md); Jim settled the product
decisions below on 2026-11-03.

## Objective

A tab dragged from one window's tab bar can drop into another Huterm window:
on its tab bar, its title row, or an edge of a pane in its active tab. A pane
dragged by its title bar or modifier grab can drop into another window the same
ways. A tab released outside every Huterm window tears off into a new window.
Terminals, processes, IDs, history, pane layouts, and runtime-owned view state
survive every move. A cancelled or failed gesture changes nothing.

## Decisions

| Topic | Decision |
| --- | --- |
| Cues | The mockup Jim chose: the destination draws the existing accent insertion marker, pane drop region, or title-row region, plus the title chip at the pointer. The source keeps the dragged tab's slot dimmed with a dashed outline while the pointer is outside the source window. Outside every Huterm window, the source bar shows a `↗ New window` pill at the strip's trailing end when a release would tear off. |
| Payloads | Tabs and panes both cross windows. Tabs: bar slot, title row, pane edges (nest, as in-window pane mode). Panes: bar slot (new tab), title row (new tab), pane edges of the destination's active tab. No pane centre (swap) across windows. No pane tear-off. |
| Spring-loading | A tab or pane drag resting on an inactive label in another window spring-loads it there after `SPRING_LOAD_DELAY`, without taking keyboard focus. A drag that ends without a drop in that window switches it back when the sprung tab is still active. |
| Empty source | Moving a window's last tab or last pane away closes the source window through the existing emptied close: no prompt, no terminal stops. Tearing off a window's only tab is a no-op and shows no pill. |
| Quake | A visible Quake window is a source and a destination. Tear-off from it opens a regular window. Emptying it closes it as its last-tab close does. Hidden Quake windows are never targets. |
| Linux | Same features on X11. Wayland sessions run through XWayland because GPUI is built without its Wayland feature; best effort there. A torn-off window opens where the window manager places it. |
| Command | Add `Move Tab to New Window` to the command catalog, palette, and tab context menu, with no default binding. It shares the tear-off path. Moving to an existing window by command is a follow-up. |

## Constraints

- Moves stay inside one runtime. Resolve destinations by runtime-scoped IDs;
  reject a destination window whose runtime differs, with a notice. Today all
  windows share one `DesktopRuntime`, so this is a guard, not a feature.
- Use the existing atomic Mux operations: `move_tab`, `move_pane`
  (`MoveDestination::Beside` and `NewTab`), and `move_tab_to_pane`. A move into
  another session already revokes the moved terminals' viewers; the destination
  installs fresh viewers through `view_install`, and the source drops its views
  through reconcile without closing anything.
- Commit on a worker, as `finish_reorder` and `commit_pane_move` do; never
  block the GPUI thread on the Mux lock.
- Never read or update another window's view while that window is on GPUI's
  update stack. Coordinate through the `Desktop` global and deferred updates.
- Keep `huterm-protocol` and `huterm-core` free of GPUI types. Core changes,
  if any, stay generic membership operations.
- Follow `docs/agents/desktop-input.md`: capture listeners own motion and
  release, Escape stops propagation, the closed hand shows from press to
  release, every path that clears a drag notifies its window, and terminal
  focus never produces a focus-out report mid-drag.

## Approach

### Cross-window drag session

Add an app-level drag record on `Desktop`, written only by the source window:
a drag generation, the payload (`Tab { window, workspace, tab }` or
`Pane { window, workspace, tab, pane }`), the current destination window, and
the latest pointer sample in that window's coordinates. The source keeps owning
the gesture through its existing capture listeners, which already receive
motion and the release outside its bounds. Each motion:

1. Locates the topmost Huterm window under the pointer (below).
2. If it is the source, the existing in-window reorder or pane-drag logic runs
   unchanged.
3. If it is another eligible terminal window, the source records it as the
   destination and notifies that window on a deferred update tagged with the
   generation and pointer sample. The destination resolves its own target from
   its own last drawn frame and stores it with that tag; a callback whose tag
   is older than the record's, or whose generation has ended, changes nothing.
4. If it is an ineligible window (Settings, About, a closing, busy, or
   confirming terminal window, a window with a modal overlay), there is no
   target and no tear-off.
5. If no Huterm window is under the pointer, a tab drag from a window with two
   or more tabs is in tear-off; anything else has no target.

While the pointer is outside the source window, the source suspends its own
autoscroll and pane-mode targeting. When the destination changes or the drag
ends, the previous destination clears its cue and runs its own spring-back.
Closing either window mid-drag clears the record: a closed source ends the
drag; a closed destination drops the target.

### Destination hit testing and spring-loading

The existing target helpers assume the payload lives in their own window:
`pane_drop_target` needs a local `pane_drag` and the payload's tab in
`self.tabs`, slot and title-row targets require that tab to have several
panes, `pane_drop_change` takes the window's own workspace as the source, and
`SpringLoad`/`SpringBack` treat the payload tab as the origin. Extract
payload-independent hit testing (zone, slot, title row, spring label, size
refusal) that takes the payload's identity from the drag record, and keep three
things separate: the payload's source membership, the destination's origin tab
for spring-back, and the destination's sprung tab. Incoming previews never move
keyboard focus: destination spring-load and spring-back select with
`focus = false`, and `select_tab`'s rule that drops a local pane drag does not
apply to incoming drags.

A cross-window pane drop accepts a lone pane (its tab has one pane) on the
destination's slot or title row, since the new tab lands in another workspace.
The destination title row takes a drop only under the conditions that
[#365](https://github.com/jimeh/huterm/pull/365) set: bar hidden and a
Huterm-drawn row.

The destination's frame scheduling, autoscroll, spring deadline, retarget after
a frame, and fullscreen tab-bar reveal read the incoming drag as they read the
local `reorder` and `pane_drag`, so a stationary pointer over an overflowed
destination edge keeps scrolling, scrolling re-arms spring-loading, and an
inactive fullscreen window's revealable bar reveals under an incoming drag.
Quake eligibility uses the window's settled on-screen state, not
`QuakeRecord::visible`, which is the transition target.

### Window location

Put the hit test behind a small trait so window tests can fake it. Its input is
the source window and the source-local pointer from the event, never a cached or
global cursor, so window-addressed synthetic events in smokes work the same as
real ones: `fn locate(source, point) -> Located` returning the topmost window
with its local point, or nothing.

- macOS: convert the point to screen coordinates through the source
  `NSWindow`, find the topmost window there with
  `NSWindow windowNumberAtPoint:belowWindowWithWindowNumber:` so another
  application's window in front of Huterm occludes it, map the number to a GPUI
  window, and convert the screen point into that window's content coordinates
  natively. The same screen point places a torn-off window.
- X11: translate the point to root coordinates through the source X window
  (`TranslateCoordinates`, as `quake/x11.rs` does), order Huterm windows by
  `_NET_CLIENT_LIST_STACKING`, and translate the root point into each
  candidate in turn, testing it against the content size. Never use
  `Window::mouse_position` (a cached field) or `Window::bounds` origins (which
  GPUI's X11 backend can report parent-relative). Treat a missing property or
  an empty or incomplete stack (`window_stack` can return `Some([])`) as
  unusable and fall back to the model's activation order. Other applications'
  windows do not occlude on X11; document that limitation.
- Skip minimized and hidden windows, including Quake windows that are hidden
  or mid-transition.

### Commit and transfer ownership

Release resolves the final target at the release point, not from the last
motion's record: the source runs the destination's hit test for the release
sample synchronously (the destination is not on GPUI's update stack while the
source handles its event) and rechecks eligibility.

Before dispatching the worker, the source reserves both endpoints: it marks
itself and the destination `busy` with a transfer, the same flag other
structural operations set, so close requests, other structural commands, and
new drags in either window queue behind it. A destination that cannot be
reserved (gone, closing, busy, confirming) cancels the drop. The reservation
ends only at the transfer's completion, below, in both windows, including when
either window closed meanwhile; Quit sets `terminating`, which makes the worker
refuse with `Stopped`. The worker commits one generic Mux operation through
`DesktopRuntime`, not through `WorkspaceView::commit_pane_move`, which clears
local zoom before success and records layout history:

- Tab on slot: `move_tab(source_ws, tab, destination_ws, before)`.
- Tab on title row: `move_tab(..., None)`.
- Tab on pane edge: `move_tab_to_pane(source_ws, tab, Beside { target, side })`.
- Pane on slot or title row: `move_pane(from, NewTab { workspace, before })`.
- Pane on pane edge: `move_pane(from, Beside { target, side })`.

Completion has two stages. Projection: once the projection reaches the
commit's sequence, the source releases its reservation and reconciles; emptied
close takes over when it has no views left. On success, both windows clear
view-local zoom on the tabs the move touched. Installation: the destination
keeps a pending transfer keyed by the moved pane IDs and releases its
reservation, selects the tab now holding them (for a nest, the surviving
target tab, since the moved tab is removed), focuses the moved pane or, for a
moved tab, the pane the source focused, activates its window, and resumes any
deferred close only when those panes' views have installed. An install of a
moved pane that fails after a successful move releases the reservation, shows
and activates the destination, resumes queued closes, and posts a persistent
"Cannot show moved tab" there with "Try Again" to reconcile every missing view
in this window. Panes no longer
projected in the destination workspace also release the reservation. Projected
tabs keep an empty-view destination open; install failure never closes terminals.
A failed move releases both reservations, leaves everything in place, keeps zoom
and history unchanged, and posts a notice in
the source window, which keeps focus.

Cross-window moves are not recorded in either window's layout history;
existing pruning already rejects cross-session snapshots. Pending pane closes,
exit-closes, and input sync follow the moved panes through their existing
core-owned and pane-keyed state; tests prove it.

### Tear-off and Move Tab to New Window

Open the window first, then move:

1. Create a private session and workspace and its attachment, and open a
   shell-less window attached to them (`open_window_inner(cx, false)` plus
   `adopt_attachment`, or an equivalent). The window marks a pending transfer
   that suppresses its emptied close until the transfer settles. Prefer showing
   it only once the moved tab's views install, if GPUI allows that without a
   flash; otherwise show it at once.
2. Commit `move_tab(source_ws, tab, new_ws, None)` on a worker.
3. On success, the new window installs, selects, focuses, and activates.
4. If window creation fails, nothing has moved: delete the new session, post a
   notice in the source. If the move fails, close the new window and its empty
   session, post a notice. The tab stays in the source either way.

Size the new window like the source window's content. On macOS place it so the
pointer sits over its tab bar near where the tab was grabbed, clamped to the
display's visible frame; on Linux let the window manager place it. The command
path uses the default placement on both platforms. Neither path runs while
quitting, terminating, or with a close confirmation open.

### Cues

- Destination: reuse `drag_overlay`/`target_elements` with the chip at the
  destination-local pointer.
- Source: while the pointer is outside the source window, draw the dragged tab
  dimmed in its original slot with a dashed outline and keep the bar's scroll
  where it was. While a release would tear off, draw the `↗ New window` pill at
  the trailing end of the strip (top/bottom/titlebar: right end; left/right: the
  bottom). With the bar hidden there is no lone-tab tear-off, so no pill.
- The closed hand stays from press to release in the source window. A pane
  drag's chip follows the pointer only in the window under it.
- Publish the cross-window target in smoke state, extending `drop_label`
  (for example `window:<n>:slot:2`, `window:<n>:title_row`,
  `window:<n>:pane:0:left`, `tear_off`, `none`).

## Failure paths

| Case | Handling |
| --- | --- |
| Escape, blur of the source, an opening menu, a close request, quit | Cancel; both windows clear cues; destination springs back. |
| Release over no target, an ineligible window, or a refused zone | Cancel. |
| Destination closed, busy, or detached at release | Reservation fails; cancel with no move. Once reserved, a close request in either window waits for the transfer's completion, so the move and a session close cannot interleave. |
| Motion callback arrives after a newer sample or after the drag ended | The generation and sample tag make it a no-op. |
| Release away from the last motion's target | The release point is resolved again before commit. |
| Source empties before the destination installs | The source's emptied close stops nothing; the destination's pending transfer completes on install. |
| Install fails after a successful move | Only a moved pane's own failed install releases the reservation; an unrelated failure in the same tab cannot settle it or block an unattempted moved pane. Show and activate the destination, resume queued closes, and post "Cannot show moved tab" with "Try Again". Every other non-stale failed install that leaves a projected pane without a view, including a repeated retry, posts "Cannot show tab" with "Try Again"; a stale subscription error posts a notice only for a pending moved pane. These notices persist until no failed pane stays projected in the window without a view. Projected tabs keep the window open; install failure stops no terminal. Panes that leave its projected workspace no longer hold the reservation. |
| Pane close arrives between transfer projection and completion | Resolve retained views first, then projection membership, throughout close validation, pruning, and failed-assessment retargeting. Queue the source close while its view is retained and forward it by pane ID; a destination pane with no view can still be closed. |
| Stale anchor or target pane | Core rejects before changing membership; notice in the source. |
| Source tab or pane closed or moved mid-drag | Existing validity checks end the drag. |
| Window creation fails during tear-off | No move happened; the new session is deleted; notice. |
| Move fails after the new window opened | The new window and its empty session close; tab intact; notice. |
| Different runtime | Refused before any mutation; notice names the reason. |
| Exited tab or pane | Moves like a live one; its retained history moves with it. |

## Non-goals

- Pane tear-off into a new window.
- Dropping onto a particular workspace, and moving workspaces (#32).
- Moving a tab to an existing window by command.
- A floating drag image outside Huterm windows.
- Moves between server processes; there is no server yet.
- Undo for cross-window moves.
- Native Wayland: GPUI is built without its Wayland feature.

## Verification

| Concern | Evidence |
| --- | --- |
| No partial transfer, live processes, stable IDs under stale targets | Core tests for cross-session `move_tab`, `move_pane`, and `move_tab_to_pane` with stale anchors, missing destinations, and foreign runtimes: membership unchanged on error. A PTY integration test that a cross-session `move_tab` keeps the child alive and output flowing. Extend existing tests where they already cover part of this. |
| Window-level behavior | Two-window GPUI window tests with a fake locator and a new harness helper that gives each window its own private session (the current helper attaches every window to one session, which would hide viewer revocation). Cases: tab to slot in all four placements and with an overflowed, scrolled destination strip (including a stationary pointer at its edge); title row with the bar hidden, and no title-row target with it drawn; tab nest at a pane edge; pane to slot, title row, and edge, including a lone pane; pane centre refused; spring-load and spring-back in the destination with a controllable clock, without focus moving; Escape; release away from the last motion's target; a stale motion callback after cancel; release over an ineligible window; destination closed before release, and a close requested in either window during the transfer (both orderings); last tab moved closes the source without terminating, including before the destination installs; a zoomed destination receiving a pane; a failed transfer keeping zoom and history; no history entry after success; pending pane close, exit-close, and input sync following a moved pane; lone-tab tear-off no-op; tear-off success, injected window-creation failure, and injected move failure; Move Tab to New Window; Quake source and destination; focus and activation only after install. |
| Locator | Unit tests for stack ordering, an empty or partial X11 stack falling back to activation order, and skipped hidden, minimized, and mid-transition windows. |
| Native macOS | `smoke:macos-input`: open a second window and publish per-window smoke state (window identity, geometry, tabs, terminal IDs, drag target), since the state now reads only the first window. Drive window-addressed NSEvent drags, as the smoke already does, with locations outside the source window: a tab to the other window's bar, a pane to its pane edge, Escape mid-drag, and a tab tear-off to empty screen. Assert targets, window and tab counts, and unchanged terminal IDs. Never post HID-level events that move the host cursor. |
| Native Linux | `smoke:linux-input`: run the cross-window steps under Openbox (startup, readiness, and cleanup as the other Linux smokes do) so the EWMH stack path runs, with a second window and per-window state; drive XTest drags through `xdotool`. |
| Experience | `ux-review` through Codex computer use on a scratch bundle and config: cues, placements, Quake, light and dark themes. A manual XWayland check in `vm:linux:dev` near the end (Jim approved). |
| Gates | `mise run check`, focused `cargo test -p huterm-gpui` and `-p huterm-core`, `mise run test:ghostty-releasesafe` if core changes, `mise run verify` before handoff. |

## Documentation

Update `docs/agents/desktop-input.md` (cross-window drags, the locator and its
platform limits), `README.md` or `docs/keybindings.md` for the new command,
`docs/plans/workspaces-windows-tabs.md` (drag transfers no longer deferred),
and this plan's status when it ships.
