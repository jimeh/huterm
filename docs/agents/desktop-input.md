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

`UCKeyTranslate` can emit text while starting another dead key, and completed
state can remain nonzero. Probe Space with NoDeadKeys on copied state and compare
against zero-state output; never send probe text to the PTY or decode state bits.

Serialize all native layout tests with their shared mutex. Parallel Carbon calls
caused SIGSEGV; the SDK marks `LMGetKbdType` as not thread safe. Production calls
stay on the main thread.

Route other printable input through `EntityInputHandler`. Consume recognized raw
Meta/Control/special chords even when their input queue is full, or AppKit can
insert a second interpretation. Cancel native preedit on the exact NSView outside
GPUI's update borrow, and skip deferred cancellation when the active view already
has newer preedit. Bindings and menu installation share one operation;
`smoke:macos-menus` reads actual NSMenuItem shortcuts.

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

Derive scrollbar geometry and label text from the displayed snapshot offset.
Growing the grid pulls rows out of terminal history. Let the runtime engine
anchor its shared viewport across output and resize; never also compensate the
client's offset for history growth or shrinkage. Offset zero follows live output.

Protocol selection ranges include both endpoints. Keep a mouse-down anchor
without exposing a range until dragging reaches another cell; use that same
optional range for highlighting and text extraction.

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
starts a `PaneDrag` once it moves past the tab drag threshold. With
`[panes] title_bar = "hidden"` no pane has a bar, so panes move only by
command; the bar's position never changes its drag or drop geometry.

Under `title_bar = "hover"` a bar has its rectangle but draws, and takes
input, only while `PaneBarReveals` reveals it: after the pointer rests in its
band for `HOVER_INTENT` (200 ms, the scrollbar's), while a pane drag or its
menu, Huterm's or a native `NSMenu`, holds it, or for a second after a
keyboard focus change. The window samples the pointer in the tab bar's
deferred pointer refresh and treats it as nowhere while a terminal owns a
gesture, during a pane drag, or over the fullscreen tab bar's trigger edge or
revealed bar, so a selection crossing the band never reveals one. A
revealed bar reaches its terminal as `PointerOverlays::pane_bar`, which every
press, motion, link, and drop path reads like the tab bar's overlay; a
gesture the terminal already owns still wins. The test platform never
reports a hovered window, so window tests feed `pane_bar_pointer` directly.

The window's
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

A reorder enters pane mode only over a pane of the active tab, which is not
the dragged tab, at least `PANE_DROP_DEPTH` (32 points) past the bar's inner
edge. Its preview then returns to the tab's slot, dimmed, and the bar does
not autoscroll; an edge nests the tab with `move_tab_to_pane`, and a centre
or refused zone does nothing. Everywhere else a reorder projects onto the
bar as before, including a release outside the window, so a sloppy reorder
cannot become a nest. Blur and an opening menu cancel a reorder as they
cancel a pane drag, so a release afterwards nests nothing.

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
