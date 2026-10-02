# Desktop input

Rules for keyboard, mouse, scrolling, selection, tab dragging, links, and file
drops in `huterm-gpui`. Smoke fixtures that exercise this input are in
[desktop smokes](desktop-smokes.md).

## Keyboard and text input

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

Application mouse coordinates use ChromeLayout's full terminal origin, including
horizontal offsets from vertical tabs. When hiding a retained TerminalView,
release accepted application gestures, finalize selection, and forget canceled
physical buttons: the eventual release may reach a different tab. Ignore stale
pointer callbacks for hidden views; keep focus and window-activation cleanup
subscriptions on each TerminalView. Ghostty's side of mouse format and tracking
state is in [the engine guide](terminal-engines.md#color-and-mouse-state).

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
snapshot from before a resize may arrive after the viewport has shrunk.

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
geometry, including release outside the window. Stop propagation when Escape
cancels a drag: GPUI then skips raw keystroke observers. Keep terminal focus
during the drag to avoid false application focus-out/in reports. TabStrip owns
pixel geometry for rendering, reveal, wheel input, and drag slots;
WorkspaceView owns its only scroll offset. Include that offset in drop
mapping. Edge autoscroll runs on the window animation clock with bounded elapsed
time. Manual scrolling must not pin the active tab. Pass the same preferred
sidebar width to ChromeLayout in WorkspaceView and TerminalView; synchronize
retained views on resize and before activation. Resolve window-size caps
without overwriting the preferred width. Commit canonical Mux order on a
worker and reorder retained views without selecting or recreating them.

Keep the sidebar resize handle entirely inside sidebar bounds. A grab zone
straddling the terminal can start terminal selection or application mouse input
before resize capture consumes release, leaving that terminal gesture stuck.

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
