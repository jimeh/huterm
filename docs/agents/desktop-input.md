# Desktop input

Rules for keyboard, mouse, scrolling, selection, tab dragging, panes, links,
and file drops in `huterm-gpui`. Smoke fixtures that exercise this input are in
[desktop smokes](desktop-smokes.md).

## Keyboard and text input

`KeyDownEvent::is_held` marks a held key's repeat. macOS sets it from
`isARepeat` and Wayland from GPUI's own repeat timer. On X11 the vendored
`x11-key-repeat` patch turns on XKB detectable auto-repeat and marks a press
whose key has had no release since; a server that refuses the flag leaves
every press unheld. With the flag on, X11 sends no release before each
repeat, so a held key's `KeyUp` arrives once, at its real release, as on
macOS, and the patch skips GPUI's 20 ms release filter, which would
otherwise drop a real release before a fast fresh press. Keystroke
interceptors see the flag through the `keystroke-held-flag` patch; keystroke
observers and modifier-only keystrokes always see `false`.

GPUI's macOS `key_char` resolves Option dead keys separately, and AppKit handles
marked text before GPUI shortcuts. Keep Option-only composition in caller-owned
`UCKeyTranslate` state after GPUI dispatch; never create native marked text for
Option dead keys. Use the selected input source's Unicode layout and leave
input methods without one on the native text path. Cancel local Option state on
commands, pending shortcut prefixes, focus/tab/policy changes, and layout changes.

GPUI replays unmatched chord prefixes through `on_key_down` and then its input
handler, bypassing raw keystroke observers. Track the actual pending sequence and
consume replay keys before they become text, including nonprinting strokes.
Timeout announces no pending input before replay; mismatch announces it after.
A replayed fallback action reports only its matched prefix's last key. Capture
GPUI's enabled fallback bindings when it resolves a sequence, not when the
prefix starts: selection can change conditional eligibility without changing
focus. Capture at timeout's pre-replay notification or the first wrapper action's
capture phase, then freeze through all replay actions, including keymap reloads.
Use GPUI's matcher to consume whole prefixes, including repeated keys. Public
keystroke interceptors omit modifier-only mismatches, so do not rely on them for
this capture. Ignore menu actions while GPUI still holds a pending sequence.
The later pending notification clears completed actions. Put shorter fallback
bindings before longer chords in config:
a newer short binding makes GPUI discard the longer pending match.
Mirror GPUI's cancellation on focus change;
window deactivation alone leaves its pending sequence alive. Native commits must
never be classified by their text. Accepted replay actions still skip the current
native event lookup.

`macos_option_as_alt = "left"` or `"right"` needs the Option key's side,
which GPUI's `Modifiers` drop. `handle_keystroke` runs inside `AppKit`'s
key-down dispatch, so `native_quit::option::current_key_flags` reads it from
`[NSApp currentEvent]`'s device-dependent bits, `0x20` left and `0x40` right,
after checking that the event is a key-down for this window whose Option bit
matches the keystroke's Alt. Anything else leaves the side unknown and
sends no Meta. A replayed chord prefix reads the key-down that triggered the
replay. `smoke:macos-input` posts Option key-downs with each bit.

`UCKeyTranslate` can emit text while starting another dead key, and completed
state can remain nonzero. Probe Space with NoDeadKeys on copied state and compare
against zero-state output; never send probe text to the PTY or decode state bits.

Serialize all native layout tests with their shared mutex. Parallel Carbon calls
caused SIGSEGV; the SDK marks `LMGetKbdType` as not thread safe. Production calls
stay on the main thread.

`Window::dispatch_key_event` draws a dirty window before it dispatches a key,
and each AppKit event flushes its effects first, so a key typed right after
one that moved focus, such as Escape closing a menu, meets a frame that
already paints the new focus. A Tier 2 test that dispatches two keys inside
one update models neither: the first key's emitted events, such as a menu's
`Dismissed`, have not run when the second arrives. Dispatch such keys in
separate updates, and use the overlays smoke's `native-batch` posts for
AppKit timing.

Route other printable input through `EntityInputHandler`. Consume recognized raw
Meta/Control/special chords even when their input queue is full, or AppKit can
insert a second interpretation. Cancel native preedit on the exact NSView outside
GPUI's update borrow, and skip deferred cancellation when the active view already
has newer preedit. Bindings and menu installation share one operation;
`smoke:macos-menus` reads actual NSMenuItem shortcuts.

### Kitty keyboard routing and identity

Use disambiguation (bit 1) in the displayed snapshot's masked Kitty flags to
decide whether to claim a Ctrl chord legacy cannot encode, such as Ctrl+1 or
Ctrl+;. Flag 4 alone does not claim these chords. Carry its
layout text as a legacy fallback if flags turn off before dequeue. Core always
chooses bytes against live flags. Synced panes encode independently; a legacy
source cannot supply a Kitty-only chord to a Kitty target because it never
claims that chord. Cmd/Super remains client-owned.

Keep raw Shift for named keys and Character identity. Map F1-F12 and Insert to
protocol keys. GPUI's optional `KeyIdentity` is separate from `Keystroke`, whose
equality drives bindings. The native patches derive active-layout unshifted
and shifted characters, held Shift, and the physical US character before GPUI
normalizes punctuation or removes Control text. Capture it in the interceptor
and use it in Huterm's post-matcher observer, preserving shortcut ordering.
GPUI sends unmatched replayed prefixes directly to key-down listeners and text
insertion, bypassing keystroke observers. macOS suppresses them before native
text insertion through `PendingShortcuts`; replayed actions are consumed by the
observer. Linux registers no terminal text input handler. Replayed prefixes
therefore never reach the PTY, so no replay identity is stored.

Synthetic or unpatched events fall back to `Keystroke`: letters keep Shift,
and punctuation uses the reported character. That fallback cannot recover an
unshifted punctuation key or a non-ASCII physical alternate precisely. Option
composition and IME commits stay `Text`; they never enter the key encoder.

## Text fields

Text fields take their keys under `TextField`; the keymap rules, the field
verbs, and the palette's fallthrough are in
[the desktop client guide](desktop-client.md#keymap-commands-and-menus). A
right press opens the field's context menu through `huterm_ui`'s
`ContextMenu` action. GPUI's `track_focus` focuses an element on a press of
any button, so a disabled field prevents the default for Right as well as
Left, or a right press would focus it. The field's own bubble listener runs
before that focus listener, so the field focuses itself before it
dispatches, and it dispatches deferred, after its update ends, so the
owner's handler can read it.

## Mouse buttons and application mouse reporting

Ghostty treats mouse encodings 1005 and 1006 as mutually exclusive; the last
enabled format wins. Read its active behavior through the retained native probe
rather than retaining independent client flags. Encode mouse events at dequeue
time against live modes and dimensions. GPUI owns physical-button lifetimes and
may admit one overflow release per accepted button, consuming ownership before
enqueueing the release.

GPUI's X11 backend remaps Shift vertical wheel lines to horizontal-only deltas.
Restore those deltas to vertical only on the local scroll path on Linux. Leave
application horizontal wheel reports and macOS deltas unchanged until native
macOS device evidence justifies normalization there.

On macOS, GPUI remaps Control-left independently at press and release,
clearing Control and discarding the original button identity. Match releases
to held logical buttons first; an unmatched Left/Right release closes the held
opposite Left/Right only on macOS. Simultaneous Control-left and physical Right
can collapse into one logical button, so their physical release order cannot be
recovered. Do not extend this fallback to Middle or Linux. Finalize local
selection text on blur as well as release before stopping the drag.

A native context menu takes the release of the right press that opened it,
so the terminal never sees it. `MouseState` records a local right press as
held, and the next right press would return early as a duplicate; the menu's
session calls `TerminalView::release_menu_press` when it closes.

Application mouse coordinates use ChromeLayout's full terminal origin, including
horizontal offsets from vertical tabs. When hiding a retained TerminalView,
release accepted application gestures, finalize selection, and forget canceled
physical buttons: the eventual release may reach a different tab. Ignore stale
pointer callbacks for hidden views; keep focus and window-activation cleanup
subscriptions on each TerminalView. Ghostty's side of mouse format and tracking
state is in [the engine guide](terminal-engines.md#color-and-mouse-state).

A view displays the snapshot's grid, which differs from the grid it requests
while another viewer controls the terminal and until the snapshot for a new
layout arrives. `GridGeometry` maps the pointer onto the displayed grid:
application reports, link points, and selection points use displayed-grid cells
clamped to its nearest edge. Only the visible area, where the displayed grid
overlaps the requested grid's painted bounds, starts a report; it is measured in
pixels, so a viewport narrower than one cell still reports on the part it
paints. Outside it the pointer keeps the padding's local behavior. Every mouse
report carries the displayed snapshot's geometry revision, and the runtime
discards a press, hover, or wheel report whose grid has changed since; releases
and owned motion are never checked. A snapshot with a new revision forgets the
motion deduplication sample, so a discarded hover does not suppress the next one
in the same cell. A local layout change forgets the hover point as well as the
link result: no snapshot follows it while another viewer controls.

## Pointer tracking, scrolling, and selection

GPUI shows the cursor of the last-painted hovered hitbox that set one, and
only an occluding hitbox hides those beneath it. The terminal root sets
`terminal_cursor`: the I-beam, the arrow on the scrollbar or while the
program takes the mouse in an active window, unless Shift is held, and a
hand on a link. Focus does not count, because a press focuses its pane
before `application_mouse` routes it; an unfocused pane over vim shows the
arrow. The wheel follows the same rule through `application_wheel`: it
reaches the program in an unfocused pane while the window is active, and
never moves focus. Presses, motion, and releases still need the pane
focused, and Shift still scrolls locally. A wheel over a neighbouring pane
during one of Huterm's own drags reaches that pane's program, as local
scrolling would. A held gesture keeps its owner's cursor until release, whatever
Shift does meanwhile, and a scrollbar drag keeps the arrow off the strip.
Modifier events reach only the focus path, so the workspace root refreshes
the hovered pane's cursor when Shift changes; that root listener is the
window's only modifier listener (see "Modifier pane drags"). Anything drawn
over a terminal that takes input without occluding, such as a revealed pane
bar, a toast, or the palette scrim, sets its own cursor, or the I-beam shows
through it. Huterm's own drags do not use GPUI's drag, so
`WorkspaceView::drag_cursor` paints a window cursor last while a divider,
sidebar, pane, or tab drag runs: the closed hand from a pane or tab drag's
press on, before the threshold and in a tab's pane mode, however it started.
Every path that clears a drag notifies the view, so the next frame drops the
override. Tabs and pane bars have no hover hand. A cursor change
with no other effect, such as Shift over vim, re-renders only when the shape
differs from the last one drawn.

GPUI's `on_hover` also re-tests hover at paint against the last pointer
position the window saw, which goes stale once the pointer leaves the window.
An element that appears where the pointer last was reports hovered with no
pointer movement. Hover that changes state, such as a toast pausing its
expiry, enters only from a pointer move (`on_mouse_move`) and takes only the
leave from `on_hover`.

Derive scrollbar geometry and label text from the displayed snapshot offset.
Growing the grid pulls rows out of terminal history. Let the runtime engine
anchor its shared viewport across output and resize; never also compensate the
client's offset for history growth or shrinkage. Offset zero follows live output.

Protocol selection ranges include both endpoints. Keep a mouse-down anchor
without exposing a range until dragging reaches another cell; use that same
optional range for highlighting and text extraction.

GPUI's `click_count` picks the selection unit: two clicks select words and
three or more select logical lines. The runtime expands them on the owner
thread with Ghostty's `select_word_between` and `select_line`, and a word
press inside a recognized link takes the whole link, because Ghostty's word
boundaries include `:`. A unit selection has no range until the first reply,
so key contexts and highlighting wait for it, but Copy and the CopyOrPaste
right-click treat it as selected, or a right-click would paste. Keep one
expansion request in flight per gesture and send only the latest head when it
completes. Match each reply to its gesture and viewer epoch; a release, blur,
or hide caches the text only once no request is in flight or wanted. A Copy
while expanding asks for the range itself, because the right-click clears the
selection straight after.

Use the same inset track for painting and drag mapping. Indicator visibility
depends on recent interaction, including at offset zero; schedule its hold
deadline and frame-driven fade so idle terminals redraw it. Keep the label
background opaque before applying the indicator's fade opacity.

GPUI element `on_mouse_move` filters by hover. Register drag tracking through
`Window::on_mouse_event` during canvas paint to receive movement outside the
window. Mount that canvas before a drag starts; conditionally adding it after
mouse-down races the first outside-window move before the next paint, especially
under Xvfb. Transparent macOS titlebars extend the content area; use the shared
terminal viewport inset for PTY sizing, scrollbar geometry, and mouse input.

Within that viewport, `TerminalLayout` owns padded grid bounds and dimensions
for painting, PTY sizing, and selection coordinates. Keep scrollbar geometry
relative to the outer viewport. Clip grid painting to its bounds because a
snapshot from before a resize may arrive after the viewport has shrunk, and
because another viewer may control a larger grid. Page scrolling moves the
smaller of the requested and displayed row counts, so a clipped view never skips
rows it cannot show.

Shared relative scroll requests can follow output that advances the runtime
viewport. Benchmark expected offsets use command plus pre-operation runtime
viewport/history; retain the client prediction separately. Clamp pending relative
UI intents after an authoritative completion so a saturated history boundary
does not leave scroll debt that affects later reversal. Clear any satisfied
pending scroll intent after authoritative completion, including absolute/live
intents, so later invalidation cannot replay a stale target.

## The drag model

Tab, pane, and sidebar row drags share one model (`windows/drag.rs`; see
[the plan](../plans/unified-drags.md)). Divider, tab-column, sidebar-resize,
and scrollbar drags, and OS file drops, keep their own paths.

- `WorkspaceView::drag` holds the window's one `Gesture` from the press on.
  Its `Payload` is a `Tab`, a `Pane`, a `Workspace` row, or a `Session` row,
  and its `Press` applies the one `DRAG_THRESHOLD`.
- `resolve_drop` (`drag/gesture.rs`) is the only target resolver. It
  returns a `DropTarget`: a pane zone, a tab-bar `Slot`, the `TitleRow`, a
  `Spring` key, a sidebar `RowSlot`, a sidebar `Workspace` row under a tab
  or pane, or another window's content (`Window`) under a workspace row.
  It runs as the source (`Side::Source`) and as another window's preview
  (`Side::Incoming`), and keeps each payload's rules: a tab reorder
  projects onto the bar outside pane mode, a lone pane offers no slot or
  title row in its own window, and only the dragged pane's own tab offers
  a centre.
- `operation` is the only table from a payload and a target to a Mux
  operation, `None` refusing the drop. Local drops commit through
  `commit_drop` and cross-window ones through `dispatch_transfer` with the
  same `Operation`; `DesktopRuntime::commit_drop` runs each under the Mux
  lock. A tear-off is the table's title-row drop into the new window.
  Cross-window code reads tab, pane, and workspace gestures, never a
  session row's: a busy window still takes a row press, so a row gesture
  can exist while a tear-off's window opens, and restoring after a failed
  transfer reads only a tab or pane gesture.
- `Hover` holds what a drag over one window shares with a destination's
  preview (`cross_window::Incoming`): the target, `SpringLoad` over a
  `SpringKey` (a tab or a workspace row), the spring-back as `Shown`
  workspace and tab pairs, the bar scroll to restore, and the tab strip's
  autoscroll. `ListScroll` scrolls a sidebar list under a dragged row, and
  under a tab or pane over the workspace list.
- The capture listeners send motion and release to `update_drag` and
  `finish_drag`; Escape, blur, menus, switches, and window close end a
  gesture through `cancel_drag`. Escape ends a tab press at once and every
  other press only once it drags (`Gesture::escape_cancels`), so an
  unmoved pane press still lets Escape reach the terminal. A row drag that
  ends without a drop puts nothing back; a tab or pane drag restores the
  bar scroll, springs back, and refocuses the terminal unless an overlay
  holds focus.

A new payload or target goes into the model, the resolver, and the table;
a new drag must not grow its own record or capture branch.

## Tab reordering and the sidebar

Tab reorder uses source-window capture listeners registered before terminal
mouse handlers. Project movement onto the tab-bar axis and clamp preview/drop
geometry, including release outside the window, except in pane mode (see
"Pane drags, tab nesting, and spring-loaded tabs"). Stop propagation when Escape
cancels a drag: GPUI then skips raw keystroke observers. Keep terminal focus
during the drag to avoid false application focus-out/in reports. TabStrip owns
pixel geometry for rendering, reveal, wheel input, and drag slots;
WorkspaceView owns its only scroll offset. Include that offset in drop
mapping. Edge autoscroll runs on the window animation clock with bounded elapsed
time. Manual scrolling must not pin the active tab. A sidebar resize pushes
the new terminal geometry into every retained view and resizes only the shown
ones; hidden views resize before activation. Resolve window-size caps
without overwriting the preferred width. Commit canonical Mux order on a
worker and reorder retained views without selecting or recreating them.

Keep the sidebar resize handle entirely inside sidebar bounds. A grab zone
straddling the terminal can start terminal selection or application mouse input
before resize capture consumes release, leaving that terminal gesture stuck.

## Panes and divider drags

A press in a pane focuses it from a capture listener on the pane's element,
because the terminal's own press handling calls `prevent_default`, which stops
GPUI's focus tracking. A divider's grab zone is 8 points across its line,
clipped to its split. Each zone is an occluding element with the resize
cursor that claims a left press itself: occlusion keeps the panes beneath
from seeing it, so no terminal starts a selection or reports the press, and
overlays drawn later (toasts, a revealed fullscreen tab bar, menus and
dialogs) keep theirs. A window-level capture listener would claim presses
those overlays own. A pane's scroll pill is drawn inside its terminal, below
the zones, so a zone wins where a pane is narrower than the pill. The
window's capture listeners own motion and release once a drag starts.

macOS GPUI turns a Left release with Control held into a Right release, so
Control pressed mid-drag would leave a drag running. The release listener
asks `pane_grab::release_ends_drag`: a Left release ends any of the window's
drags (divider, sidebar, the drag gesture of any payload, tab scrollbar),
and on macOS so
does a Right release while no real Right press is held. While a drag runs,
the press capture listener records a macOS Right press in
`drag_right_press` and stops it, so no context menu opens and cancels the
drag; its release clears the record and ends nothing. Linux has no remap,
so Right never ends a drag there. `smoke:macos-input` posts a real Left
down, a Control `flagsChanged`, and a Left up carrying Control over a
divider drag.

A drag holds its boundary, the sum of the weights before it, and applies it
over the split as other changes leave it, so another window's resize of a
sibling boundary carries through. Dragging runs the preview through the
normal geometry path, so both shells reflow, clamped so every pane keeps 4
columns by 2 rows in this window. Release commits once with `resize_split`.
One queue per window runs commits in order, so two resize commands cannot
land out of order. The view keeps showing a commit's weights until the
projection reaches it, so the divider never snaps back; a rejected commit
drops them silently. Escape, blur, switching tabs, a zoom change from either
scope, a close request, and a layout change that removes the split or
changes its children cancel the drag. Resize commands commit the same way.
While the window has a pane arrangement in flight, a divider press starts no
drag and resize commands are refused, since both would compute from the
weights the arrangement is about to replace.

Zoom hides the tab's other panes as a hidden tab hides its view: `hide`
releases accepted gestures and finalizes the selection, and the hidden panes
keep requesting their split geometry, so zoom resizes only the zoomed pane.
Every path that sets pane visibility, tab selection and the Quake visibility
sync alike, asks `zoom_shows`.

## Pane drags, tab nesting, and spring-loaded tabs

A press on a pane title bar's title, not its buttons, focuses the pane and
starts a pane gesture once it moves past the drag threshold. The window's
capture listeners own its motion and release, as they do for a reorder, and
the drag keeps terminal focus, so no application sees a focus report. Each
motion resolves the target from the last drawn frame (`drawn_panes`, only
while it shows the active tab): an edge or centre zone of a pane from
`drop_zone`, a tab bar insertion slot, or an inactive label's spring zone
from `TabStrip::label_zone`. The bar targets cover the strip's bounds, not
the window buttons or title-row controls beside it. Escape cancels only a
press that has become a drag; before that it reaches the terminal. The centre is
offered only on a pane in the dragged pane's own tab, because a swap
exchanges panes within one tab. A zone whose result would leave a pane under
the minimum size is drawn refused and takes no drop. A release commits one
Mux operation through the move commands' `commit_pane_move`.

A workspace switch cancels every drag first (`switch_workspace`), as a
session switch and New Tab do; only a drag's own spring load of a workspace
row switches under it (see "Workspace drags").

A lone pane's title bar, enabled by `title_bar_always_show`, uses the same
spring-load and edge-drop path to join another tab. Its own pane is skipped,
and tab insertion slots are excluded even after spring loading: core treats
`MoveDestination::NewTab` in its own workspace as a no-op. In a one-tab window
with a hidden tab bar, a lone drag has no targets.

With the bar undrawn and `Presentation::Hidden`, a split tab's pane can drop
on the Huterm-drawn title row to open a new tab at the end of the workspace.
`title_row_layout` shares the rendered row bounds and control exclusions with
hit testing and the drop highlight: the macOS traffic-light inset, Linux
leading/trailing button groups, and the menu button slot stay out. The row
requires `title_row() > 0`, so fullscreen, Quake, and Linux server decorations
offer no target. It neither spring-loads nor autoscrolls. Incoming tab and
pane drags can use the same row, including a lone pane from another window.
Capture owns motion and release;
a press begun on a pane never arms the title row's window-move gesture.

A reorder enters pane mode only over a pane of the active tab, which is not
the dragged tab, at least `PANE_DROP_DEPTH` (32 points) past the bar's inner
edge. Its preview then returns to the tab's slot, dimmed, and the bar does
not autoscroll; an edge nests the tab with `move_tab_to_pane`, and a centre
or refused zone does nothing. Elsewhere within the source window a reorder
projects onto the bar as before, so a sloppy reorder cannot become a nest.
Blur and an opening menu cancel a reorder as they cancel a pane drag, so a
release afterwards nests nothing.

A pane drag resting on an inactive label arms `SpringLoad`, which reports one
deadline, `SPRING_LOAD_DELAY` (500 ms), through the window's animation
schedule and requests no frames. At the deadline the pointer is resolved
again against the strip as drawn now: still the armed tab's spring zone, the
tab activates through `select_tab` with focus; another tab's, the delay
restarts; anything else disarms. Autoscroll re-arms each tick, so a
scrolling bar never springs. The drag records the tab it sprang to, and
`select_tab` drops a pane drag for any other switch, so only the drag's own
spring load switches tabs under it. A drag that ends without a drop, by
Escape, a release without a target, blur, an overlay opening, a close
request, or a refused commit, switches back to the tab it started in when
the sprung tab is still active, without taking focus from an overlay. A
render that finds the dragged pane gone from its tab, or an unexpected
active tab, defers the cancel, so a drag also ends while the pointer rests.

### Cross-window drags

`windows/cross_window.rs` coordinates source-owned capture with destination
previews. Resolve each event's source-local point through `window_locator`;
never use the cached mouse position or GPUI's X11 window origin. AppKit converts
through NSWindow and asks the window server for the topmost window, so other
applications occlude Huterm. X11 retains one main-thread private connection and
its interned EWMH stacking atom, retiring the connection after a failed request.
It uses TranslateCoordinates and the EWMH client stack, falling back to
activation order when that stack is missing, empty, or incomplete over viewable
windows. Exclude unmapped windows before checking completeness, so a hidden
Quake window cannot discard the native stacking order. Other
applications do not occlude targets on X11. GPUI runs through XWayland in
Wayland sessions, where this is best effort.

Preview callbacks carry a gesture generation and sample number and validate both
at delivery. Every new gesture starts a fresh generation, including from the
same source. All local drag endings, including tab-navigation interruption, end
the shared record, clear the destination cue with spring-back, and notify.
Destination spring loading selects without moving keyboard focus and springs
back on cancellation. Its own animation schedule owns stationary autoscroll and
spring deadlines. Hidden, minimized, busy, closing, confirming, and transitioning
Quake windows take no drop. Pane centres never accept a cross-window swap.

Resolve again at release. Reserve both windows before dispatching a worker, and
keep the destination busy until every moved pane still projected in its
workspace has an installed viewer, or one of those panes' own installs fails. An
unrelated subscription failure in the same tab must neither settle the transfer
nor exclude a moved pane that has not been subscribed. A pane closed or moved
elsewhere by another client no longer holds that reservation. Only a successful
move clears local zoom; cross-window moves add no layout-history entry. A pane
close resolves its tab from this window's retained views first, then from
projection membership, in request validation, the runtime request, confirmation
pruning, and retargeting after a failed assessment. That accepts a source close
after the projection moved its retained view away, and a destination close
before the pane's view installs. Queued source pane closes follow their pane IDs
and resume after destination installation. A failed move restores the
destination preview selection and scroll without taking focus. A failed
moved-pane install releases the reservation, activates and shows the destination
(including a hidden tear-off host), resumes queued closes, and posts "Cannot
show moved tab" with "Try Again" to reconcile every missing view in that window.
Every other non-stale failed install that leaves a projected pane without a
view, including a repeated retry of a moved pane, posts "Cannot show tab" with
the same action. A stale subscription error, where this window's attachment no
longer reaches the terminal, posts a notice only for a pane of the pending
transfer; teardown and results for panes no longer held here post none. These
notices persist, like the other notices with a recovery action, until reconcile
finds no failed pane still projected here without a view. Projected tabs keep an
empty-view destination open, and install failure never stops terminals. An
emptied source closes without stopping its terminals.
Tear-off creates a shell-less regular destination first and rolls back only its
empty session on failure. A lone tab never tears off; Move Tab to New Window is
unavailable in its palette, tab menu, and command invocation with the same reason.
A pane tears off unless it is the only pane of the window's only tab
(`pane_tear_off_availability`, shared by the release, the cues, Move Pane to New
Window, and the pane menu), moving through `move_pane` to `NewTab` in the new
window's workspace. Release drops the gesture before the window opens, so
`tear_off` captures its spring-back and bar scroll in `transfer_back` and
`transfer_scroll`; every failure, including a failed move in
`dispatch_transfer`, restores them. While a release would tear a pane off, the
source pane draws S1 (`pane_tear_off_cue`): the terminal colour at 55% over its
drawn frame, a full-contrast dashed outline, and the centred pill, compact in a
pane too narrow for the label. Modifier-grab drags get it too. A workspace
row that would tear off draws the same cue on itself (W1, `sidebar_rows`).
The tear-off pill follows the labels and add button in the bar's free trailing
space. A horizontal bar too short for its label, such as a narrow title strip,
gets the icon-only pill there. Only when neither fits does it cover the strip's
trailing end with an opaque fill.
`tear_off_cue` is the release's outcome, which drop labels publish;
`tear_off_pill` gates the drawn bar pill and S1, which give way while the
macOS drag preview is visible. The bar pill marks a tab tear-off; a pane drag
gets S1 instead, and the bar pill only while its pane is not drawn, as after
spring-loading another tab (`bar_tear_off_pill`).

The macOS drag preview (`drag_preview.rs`) is a `PopUp` window per drag, opened
when the drag first needs it, ordered out while the pointer is back inside the
source, and closed when the drag record ends. Every record change calls
`drag_preview::sync`, which defers and coalesces; its mode is a pure function
of where the pointer is, the source's `macos_drag_preview`, and the platform.
The chip mode (over another eligible terminal window) keeps the in-window
chip's 12-point offset and flips; the miniature and the dimmed chip centre on
the event's screen point, which the drag record samples through
`window_locator::screen_point` at each event. Only a natively visible chip
preview stops the destination drawing its own chip, so a failed or still
queued show keeps every in-window cue. The locator searches below the
preview's window number. On macOS 27.0 `windowNumberAtPoint:` already skips
the preview, which ignores mouse events, even from the top of the stack;
starting below it keeps that from depending on the flag. Code that indexes
or counts windows during a drag (`smoke_state`, drop labels) enumerates
terminal windows only, through `terminal_windows`.

Keep the source slot's dashed state border at full contrast while dimming its
contents. Every drag title chip measures its size, flips across each overflowing
pointer axis, and clamps inside the window. The chip floats over terminal
content, and the pill's last fallback covers the strip's tab labels, so both
paint from `floating_colors`, which ignores `background_opacity`, not from the
translucent `tab_colors`.

Native input fixtures use window-addressed AppKit events, including negative
source-local coordinates, and XTest root coordinates on X11. Publish per-window
identities and cross-window drop labels. X11 smoke coordinates use the client's
TranslateCoordinates root origin, since window-manager frame geometry can differ.

### Workspace drags

See [the plan](../plans/workspace-drags.md) for the interleavings and their
tests.

- A tab or pane over another workspace's sidebar row targets
  `DropTarget::Workspace`; a drop moves it to that workspace's end and the
  window keeps showing its workspace. The item's own workspace's row takes
  nothing, and the shown row takes drops but never springs. Rows resolve
  from `workspace_row_bounds` only while the list draws and the pointer is
  inside its visible scroll bounds, since rows scrolled out keep stale
  bounds. The corner switcher takes nothing.
- A dragged workspace or session row lifts: `lifted_row` draws a copy of
  the row, centred on the pointer and kept within the list's visible part,
  while the drag is inside its window, and the overlay draws no chip for
  it. The copy goes inside each list's own element tree, because a
  popover's list draws above the window's overlay; a canvas places it in
  window coordinates there. Other windows and the floating preview show
  the chip, as for tabs. A tab's drag preview takes the tab's own shape
  in its slot (`tab_bar::tab_body`), so a pill's preview is pill-sized.
- A row spring is a visibility switch that keeps the gesture.
  `spring_load` checks `check_switch_guard` (the switch guard without its
  tab-drag refusal), then records the expected `Shown` (workspace, no tab)
  before `show_workspace_focused` runs, because the switch's `select_tab`
  calls `interrupt_pane_drag`, then records the tab it shows. An incoming
  spring passes `focus = false`, so a destination never takes focus.
  `spring_back` shows the origin workspace before selecting its tab.
- A tab or pane gesture whose workspace is no longer shown (`drag_away`)
  stays valid while the window holds its views hidden, is checked each
  frame like a pane drag, resolves with another window's rules (no reorder
  projection, no centre, no lone-pane rule), and draws the chip. A slot or
  title row in the shown workspace is then `MoveTab`, not a reorder. Its
  cross-window payload and its tear-off availability (`tears_off`) come from
  its own workspace through `all_tab_views` and the projection, not the
  shown one. A failed drop springs back only once the window is no longer
  busy, since a spring-back to another workspace is a switch.
- A pane moved into another workspace of the same window keeps its view, as
  a whole tab does: `adopt_moved_panes` moves the `PaneView` between held
  workspaces before the per-workspace passes (see the desktop client
  guide). A pane that lands in another session's workspace gets a fresh
  view there and focuses once it installs (`restore_focus`). Either way the
  move records an undo entry, as Move Pane to New Tab into another
  workspace does. A committed move that empties a hidden origin closes it
  unless pinned (`close_emptied_hidden`).
- A workspace row routes through the cross-window sampler. Over another
  window it slots into that window's list, or takes its content
  (`Window`), and commits `MoveWorkspace` into that session through
  `dispatch_transfer`; `PendingTransfer::show` makes the destination show
  it, and `finish_transfer_install` waits for panes that land in any
  workspace the window holds, hidden ones included. A source that showed
  it follows through its emptied close, and a pane close it queued during
  the move runs there, since its record still names the workspace. A busy
  source refuses the release and shows no tear-off cue, since a transfer
  would end its own operation's `busy`. A window on the workspace's own
  session takes it only in its list. A row outside the window never
  edge-scrolls its own list. In the destination, an incoming tab, pane, or
  workspace over the drawn workspace list scrolls it at its edges, as a
  local one does (`Incoming::advance_list`), and retargets the preview as
  the rows move.
- Workspace tear-off (`tear_off_workspace`) moves first, with
  `Mux::move_workspace_to_new_session` and an attach under one lock, then
  opens the window, which adopts the attachment and installs. The source
  holds a transfer reservation from the start, so no checkpoint saves it
  half moved. A failure puts the workspace back in its slot (`put_back`),
  else last, and closes the new session only when it is empty under the
  same lock; otherwise it detaches it, so nothing another client put there
  stops. A session's only
  workspace never tears off. Dropping a row drag (`drop_row_drag`) also
  ends its cross-window record.

### Workspace move commands

See [the plan](../plans/workspace-move-commands.md). The keyboard and palette
equivalents of the drags commit the operation a drop would, through the same
completions: Move Tab to Workspace through `commit_tab_move` with
`Operation::MoveTab`, and Move Workspace to Session and Move Workspace Up or
Down through `move_workspace`. Both hold the window busy, and every workspace
move and every tab move between workspaces also holds Quit and checkpoints
until its completion reconciles (`hold_move`). A window's `busy` alone stops
only its own commands: Quit honors it only in its host window, and
checkpoints never read it. A busy window takes no workspace row drop, since
the move would end the other operation's `busy` and share its hold slot.
Pane moves between workspaces still hold only `busy`. Neither command
follows its item: the window keeps its workspace unless the move emptied it,
and then follows or closes through its emptied close as after a drop. A
command cancels a row or pane drag first; a tab drag refuses it
(`check_runtime_available`). The `sidebar_move_*` keys act on the focused
row, a workspace or a session list row, keep its focus, and do nothing at the
list's end. Unlike a cross-window drop, a command reserves no destination
window, which reconciles and installs the moved views as for any other
client's change; a pane close queued in the source during a move to another
session ends quietly, as R1 settled for hidden workspaces.

## Modifier pane drags

Under `[panes] drag_modifier`, each drawn pane of a split active tab gets a
grab layer over its terminal, drawn after the terminal and its dim layer and
always present, so a press never depends on the frame that armed it. It has
no `occlude`, so hover, the wheel, and other presses reach the terminal
beneath; its bubble `on_mouse_down` runs after the title bars, divider zones,
toasts, and palette drawn later have declined a press, and it consumes a
match, which then never reaches the terminal. It sets the open hand only
while armed, which wins over the terminal's cursor and loses to anything
drawn later that sets its own.

macOS GPUI turns every Control-left press and release into Right with
Control cleared, and `Window::modifiers` follows the remapped event, so a
combination with Control arrives as a Right press carrying the rest.
`PaneGrab` keeps its own `held` record, which no remapped event writes: the
root div's `on_modifiers_changed` sets it, the window's capture listeners
correct it from free `MouseMove`s, `Scroll`, `MouseExited`, and releases,
activation resets it from `Window::modifiers`, and a closing native menu
clears it, since AppKit's menu loop swallows `flagsChanged`. While a button
is held, macOS GPUI's `synthetic_drag` replays the last move every 16 ms
with that move's original modifiers, so a move with a pressed button never
writes `held`: it would bring back modifiers already let go and re-arm the
panes after the release. A Left release records its modifiers instead, and
a Right release that ends any window drag records them plus Control, since
GPUI remaps only while Control is down. A window-level
`window.on_modifiers_changed` never runs: GPUI dispatches the event only
along the focused element's path, so with nothing focused only a pointer
event updates `held`. `grab_press` accepts that Right press only while
`held` holds the whole combination, and a drag it starts is `remapped`.
Like every window drag, it ends on Right or Left, whichever arrives (see
"Panes and divider drags").

Arming recomputes on every pointer refresh from `pane_bar_pointer`, so
anything that claims the pointer or the window disarms. The badges wait
`GRAB_BADGE_DELAY` on the animation schedule and stay hidden after a key
press or an activation until `held` changes, so pane keybindings that share
the modifiers flash none. The key press comes from an app-wide keystroke
interceptor, not an observer: GPUI keeps a shortcut prefix pending and
returns before it notifies observers. Window tests set
`pointer_reveal.hovered_in_tests` to sample the pointer from real events;
`smoke:macos-input` covers the real remap with posted Command and Control
`flagsChanged` events.

## Pane title bar modes

`title_bar_always_show` extends title bars to a tab's lone pane through
`pane_geometry::layout_panes`, so reserved height, requested grids, hit testing,
and hover overlays agree. Its bar omits zoom but keeps split and close buttons;
focus accent, dimming, indicators, and modifier grab layers keep their split-tab
rules. Split buttons focus their bar's pane and use `invoke_interactive`, with
the same availability checks and failure notices as menu commands. The shared
button-width calculation accounts for the leading marks, drops both split
buttons before zoom, and always keeps close.

With `[panes] title_bar = "hidden"` no pane has a bar, so panes move by
command or modifier drag; a bar's position never changes its drag or drop
geometry. Under `title_bar = "hover"` a bar has its rectangle but draws, and
takes input, only while `PaneBarReveals` reveals it: after the pointer rests
in its band for `title_bar_hover_delay_ms`, while a pane drag or its menu,
Huterm's or a native `NSMenu`, holds it, or for a second after a keyboard
focus change. Its fade and leave hold come from `[panes]` through
`PaneBarTiming`; the shared
`Reveal` takes them as a `RevealTiming`, and the fullscreen tab bar keeps
`RevealTiming::default()`, so changing a pane key never retimes the tab bar.
A keyboard reveal survives a refresh against the frame before its bar draws,
as when the focus move also unzooms.

The window samples the pointer in the tab bar's deferred pointer refresh and
treats it as nowhere while a terminal owns a gesture, during a pane, divider,
reorder, or sidebar drag, under any menu or dialog, and over the fullscreen
tab bar's trigger edge or revealed bar, so a selection crossing a band never
reveals one. `sync_tab_layout` gives the active tab's revealed bars to their
terminals as `PointerOverlays::pane_bar` from the layout each frame draws,
and every other view none, so a resize or tab switch never leaves a stale
band; every press, motion, link, and drop path reads it like the tab bar's
overlay, and a gesture the terminal already owns still wins.

The `focus_accent` line and the `inactive_dim` and
`inactive_dim_background` layers are plain backgrounds drawn over the
terminal. GPUI gives a div a hitbox only for listeners, a
cursor, hover styles, or `occlude`, so they must carry none: every press,
wheel, hover, link, and drop then reaches the terminal beneath.
`a_dimmed_pane_takes_the_wheel_and_a_selection_through_its_dim_layer` fails
if the dim layer starts taking input.

The sync and zoom indicator (`windows/pane_indicators.rs`) draws the
same way: tessellated paths in a `canvas`, and state chips, with no
listeners, cursor, hover style, or `occlude`. It draws after the divider
lines and the rounded terminal corner beside a vertical tab bar, and the
focus cue draws after it, inset by the look's width wherever it meets one of
the indicator's lines. Under a revealed fullscreen tab bar it draws before
the bar, which covers the panes. While the sync look draws the dividers, the
plain border-coloured divider lines are left out, so none shows through a
dash's gap; the grab zones stay. GPUI blends overlapping path coverage
darker, so divider bands stop at the outline's inner edge and at the band
they meet at a T-junction.

## Pane input sync

A tab's `input_sync` lives in core; each window fans its own typing out.
`TerminalView::sync_input` runs after translation in `handle_keystroke`,
`send_composed_text`, and the Paste command, so a copy is exactly what the
focused pane got, and it emits `SyncedInput` only while the projection says
the tab syncs. The window sends that structured input to each other pane
the layout places and `zoom_shows` shows, through `receive_synced_input`,
which never syncs again. Each runtime encodes it against its own modes.
Mouse reports, focus, and file drops call `admit_input` directly and stay
with one pane; keep any new typing path on `sync_input` and any new pointer
path off it. In Tier 2 tests, live terminals redraw their window often
enough to hide a missing `cx.notify()`, so a test that a toggle redraws
passes either way.

GPUI reports a macOS window as hovered only while it is active, so hover bars
reveal only in the active window. The Linux test platform never reports a
hovered window, so window tests feed `pane_bar_pointer` directly;
`smoke:macos-input` covers the real pointer path with injected mouse-moved
events.

## Links and file drops

GPUI suppresses hitbox hover while the last input was a key press. Native file
drags deliver only `FileDrop` events, so the `file-drop-pointer-modality` patch
counts them as pointer input; otherwise a drop after typing hits no target.

The vendored GPUI X11 patch resolves native file-URI lists as a whole before
emitting any FileDrop event. X11 file-drop type negotiation must find
text/uri-list in inline offers and XdndTypeList even when text/plain appears
first. Each conversion owns a fresh requestor window;
sources may use CurrentTime=0, so timestamps cannot identify stale replies.
Destroy the requestor on
leave, replacement, failure, completed drop, and destination close. Preserve raw
URI path spelling through percent decoding: URL normalization changes symlink
`..` semantics. External drag entry retires link press ownership and releases
accepted application mouse gestures before synthetic file-drop motion.

GPUI keeps ExternalPaths in an application-wide active drag, and destroying its
native destination window does not clear it. Track the current destination from
WorkspaceView's typed drag capture, including chrome and confirmation overlays.
At actual assessed window removal, clear that window's drag with
App::stop_active_drag; do this after macOS's deferred fullscreen cleanup, just
before remove_window. Closing a different window must not cancel the drag.

Huterm tab reordering uses its own capture state, not GPUI's active drag. Keep
native two-window coverage: close a Ready destination without Exited, then prove
that a different payload and ordinary input reach the surviving terminal.

Link-owned macOS Left presses can receive a Right release when Control changes
mid-gesture. Preserve separately held Right ownership; consume an unmatched
remapped release as cancellation because GPUI erased its Control modifier.

Retained exited history uses the base link chord even if its final snapshot
still has application mouse tracking enabled.
