# Application mouse input plan

Status: proposed for implementation after review.

Issue: [#5: Add mouse input support for terminal applications][issue].

## Outcome and agreed behavior

Enable clicks, wheel scrolling, and dragging in mouse-aware terminal
applications such as htop and editors on macOS and Linux. Keep PTY encoding and
canonical terminal modes in core, with pointer interaction policy in GPUI.

When the user is viewing scrollback, mouse interaction stays local even if the
application has enabled reporting. Application interaction resumes at the live
viewport. This decision is agreed; a click on historical text must not act on
an unseen application screen.

Shift provides a local selection and scrollback override at the live viewport.
The scrollbar always stays local. Ordinary shells receive no mouse reports
unless they enable reporting.

## Scope

Implement the following tracking modes and independent encoding formats using
Alacritty's existing parser and canonical mode state:

| Setting | Behavior |
| --- | --- |
| Tracking disabled | No application mouse reports |
| 1000 | Button presses and releases, including wheel presses |
| 1002 | The above plus motion while a button is held |
| 1003 | The above plus motion without a held button |
| Legacy encoding | Byte-based coordinates and legacy release code |
| UTF-8 1005 | Extended coordinates encoded as UTF-8 |
| SGR 1006 | Decimal coordinates and distinct press/release terminators |

Include left, middle, and right buttons, vertical and horizontal wheel input,
and Control, Alt/Option, and Shift modifier encoding. GPUI reserves Shift for
local interaction when starting gestures; the protocol and encoder can still
represent it for other clients and modifier changes during an existing gesture.

UTF-8 1005 is recommended for this implementation because Alacritty already
tracks it and its encoding requires a small, bounded extension to the legacy
path. The compatibility cost is focused byte and coordinate-boundary tests.

Defer X10 mode 9, highlight tracking, URXVT 1015, pixel reporting 1016, extra
physical mouse buttons, and alternate-screen wheel-to-arrow translation. Do not
add another escape-sequence parser, new dependencies, IPC, or configurable
mouse bindings for this feature.

Use the [xterm mouse tracking specification][xterm] as the wire-format reference.

## Existing implementation and ownership

The relevant source paths are:

- `crates/huterm-protocol/src/lib.rs`: input types and snapshot modes.
- `crates/huterm-core/src/engine.rs`: Alacritty mode projection.
- `crates/huterm-core/src/input.rs`: mode-sensitive input encoding.
- `crates/huterm-core/src/terminal.rs`: runtime input and ordered PTY writes.
- `crates/huterm-gpui/src/desktop.rs`: pointer handlers, layout, focus, and input
  buffering.
- `crates/huterm-gpui/src/scroll.rs`: local viewport and fractional scroll state.

Core currently calls `encode_input` with `engine.modes()` when processing each
input message. Preserve that boundary and the writer shared with terminal
replies. Snapshot modes guide client routing but are not authoritative for
encoding a queued event.

GPUI currently handles left-button selection and local wheel scrolling. Its
selection coordinate helper returns a `BufferPoint` that includes history
offset. Application input needs a separate coordinate helper.

The desktop pending queue is bounded by both event count and bytes and can
reject any event when full. `RuntimeClient::send_input` also returns `Busy`.
Mouse reporting must account for both before accepting gestures.

## Input contract and core encoding

Add dependency-neutral protocol types with these meanings. Exact Rust names
can follow the implementation conventions:

- `MouseTracking`: disabled, buttons, button motion, or all motion.
- `MouseEncoding`: legacy, UTF-8, or SGR. Encoding alone never enables tracking.
- `MousePosition`: zero-based column and row relative to the live terminal grid.
- `MouseButton`: left, middle, or right.
- `MouseAction`: press, release, motion with an optional held button, or one
  wheel step in a specified direction.
- `MouseInput`: position, action, and existing protocol `Modifiers`.
- `TerminalInput::Mouse` and separate tracking/encoding fields in
  `TerminalModes`, defaulting to disabled tracking and legacy encoding.

Keep impossible wheel-release combinations out of the action type. Core remains
stateless with respect to held buttons; gesture ownership belongs to the client.

Project Alacritty's current bits without introducing a second mode owner. A
reset of an inactive tracking mode must not clear another active mode: for
example, `?1003h` followed by `?1000l` leaves all-motion tracking active.
`TerminalEngine::process` increments the generation for each processed output
batch, including mode-only output. Test that the resulting snapshot exposes
mode changes and terminal reset correctly.

Core must suppress reports when tracking is disabled and suppress motion that
the current tracking mode does not request. Convert zero-based coordinates to
one-based wire coordinates with checked arithmetic. Legacy coordinates are
limited to 223 and UTF-8 coordinates to 2015 in one-based units. Core first
clamps to current terminal dimensions, then to the current encoding's coordinate
limit. Apply this consistently to presses, motion, wheel steps, and releases.
This keeps releases encodable after a resize or format change without trusting
a stale client snapshot. The tradeoff is that positions beyond a legacy format's
limit report at its last representable cell. Never wrap coordinates.

GPUI converts positions to grid cells without knowing wire-format limits.
Keep SGR's distinct release terminator and legacy/UTF-8 release semantics
explicit; wheel actions produce presses only.

Do not change the public send operation into a synchronous PTY write. `Busy`
means the input was not accepted and can be retried; a stopped runtime ends the
gesture and reports the existing terminal failure state.

## GPUI routing and gesture lifecycle

Introduce a private `mouse.rs` module for routing decisions, held-button state,
motion deduplication, and application wheel accumulation. Keep GPUI event
registration and view updates in `desktop.rs`. Use ordinary value inputs for
the routing state so tests can exercise production decisions without a window.

For a new gesture, evaluate routing in this order:

1. Give scrollbar interaction priority and keep it local.
2. Start no application gesture in the titlebar, padding, or outside
   `TerminalLayout.bounds`. Preserve existing local behavior, including clamped
   selection from padding.
3. Keep input local if Shift is held, reporting is disabled, or the displayed
   viewport is scrolled back. Also suppress new application input while a
   requested scroll away from live is awaiting its snapshot.
4. Otherwise route the event to the application using live-grid coordinates.

Track individual buttons so releasing one does not clear another. Motion uses
the most recently pressed application-owned button that remains held.
Choose ownership at button-down and preserve it until release. A local drag
must not turn into application input when Shift is released or the viewport
returns live. A release for an application-owned press must not disappear
because Shift was pressed or the pointer crossed the scrollbar.

If the viewport leaves live during an application gesture, finish its accepted
button presses at the last valid position, then suppress application drag
events until the physical buttons are released. Do not start a local selection
halfway through that gesture. The same cancellation rule applies on focus loss.
Observe window deactivation as well as element blur. GPUI 0.2.2 eventually
reports deactivation through blur, but that notification occurs during drawing;
use its window-activation observer for cancellation that must not wait for a
frame. Make cleanup idempotent and queue releases before any focus-out report.

Keep the existing window-level movement handler and register inside/outside
release handlers for all three supported buttons. GPUI's `on_mouse_up` and
`on_mouse_up_out` are complementary. Do not add a third window-level release
handler unless native evidence shows it is needed; if added, prevent duplicate
delivery. The pinned macOS and X11 backends have release paths, but native
outside-window delivery remains an acceptance check. Clamp continuation
coordinates to the grid edge. Clear local held-button state after cancellation
so a later physical release does not generate another report.

Core interprets each event using the mode when it dequeues that event. While
reporting is disabled, it emits no mouse bytes, including cleanup releases.
When the client observes disabled tracking, clear its gesture state and require
a fresh press before reporting button motion. Never synthesize a press merely
because reporting became enabled.

A disable/re-enable cycle entirely between client snapshots does not invalidate
the client's gesture. Core handles subsequent events using the re-enabled mode.
A queued event processed during the disabled interval is suppressed. Pairing is
therefore guaranteed only while reporting remains enabled, not across mode
changes controlled by the application. Test both observed and coalesced
transitions. No mode revision counter or core gesture ledger is required for
this contract.

For hover motion in mode 1003, report cell changes only. For held motion in
1002/1003, also account for button and modifier changes when deduplicating.
Flush or cancel pending motion at gesture, routing, mode, and focus boundaries.

## Wheel normalization and input backpressure

Maintain separate fractional accumulators for application wheel input and local
scrollback. Convert pixel deltas using current cell height or width, and retain
fractional line deltas until they reach a whole step. Reset the relevant
remainder on direction reversal, routing changes, mode changes, and focus loss.
Preserve the existing local scroll controller's behavior. For diagonal
application input, emit vertical steps before horizontal steps consistently.
Reject non-finite deltas and invalid cell dimensions before arithmetic or
integer conversion. Verify macOS Shift-wheel with both a physical wheel and a
trackpad: a possible platform axis remapping must not defeat vertical local
scrolling. Add normalization only if native input evidence confirms it is
needed; do not assume every horizontal Shift delta came from a vertical wheel.

Bound application motion before it enters the runtime queue. Retain at most one
unsent motion sample per uninterrupted compatible run and replace it with the
latest position. Use the existing refresh/retry loop to make progress even
without paint callbacks. Never move a motion sample across a press, release,
wheel event, keyboard input, paste, or focus input.

Refactor enqueue results to distinguish acceptance from a status-display
change. Gesture state must not interpret the current boolean return from
`enqueue_input` as proof that a press was accepted.

Keep the normal pending-input limits. Permit exactly one release for each
currently application-owned button to exceed them, including cleanup releases.
There are three supported buttons, so the hard event limit becomes the normal
limit plus three and the hard byte limit becomes the normal limit plus three
accounted mouse-release sizes. A release consumes that button's ownership
immediately, preventing a duplicate release from using this allowance.

All ordinary admissions, including new presses, must check `>=` for event
capacity and the projected byte total, counting any queued overflow releases.
They cannot bypass earlier pending input. Reject an unadmittable press with the
existing visible input-buffer error and suppress its subsequent motion/release.
This fixed allowance avoids a reservation ledger while bounding repeated
press/release cycles under a stalled writer. Preserve FIFO order through `Busy`
retries. Disconnection clears the queue and all gesture state.

Cap application wheel expansion at 32 total steps per platform event, across
both axes, before allocating or looping over steps. Keep only the fractional
remainder; discard excess whole steps. If normal queue capacity is exhausted,
discard unadmitted wheel steps without a modal or persistent input-buffer error.
Already admitted steps remain in order. This deliberately trades wheel distance
under overload for bounded work, with no held-button state to repair. Cover the
cap and queue-full behavior with tests; tune the constant only with native input
evidence. Do not add a step-count protocol variant for this first implementation.

## Implementation sequence and evidence

1. Add protocol types, mode projection, and core encoding. Test exact bytes for
   all supported formats and actions, modifiers, disabled tracking, coordinate
   limits, resize clamping, and format changes between press and release. Feed
   actual mode escape sequences through the engine to verify enable, disable,
   reset, inactive-mode reset, and format transitions.
2. Add the GPUI mouse state and coordinate helpers, then connect real pointer
   handlers. Test grid/padding/titlebar boundaries, live versus displayed
   scrollback, pending viewport changes, Shift transitions, multi-button
   gestures, element blur, window deactivation, and duplicate/outside releases.
   Verify the actual GPUI handler path on Linux early, before expanding the
   implementation.
3. Integrate motion coalescing, wheel normalization, and the bounded release
   allowance. Test event saturation with mouse events and byte saturation with
   mixed text/paste and mouse input. Cover immediate and buffered presses,
   repeated press/release cycles under `Busy`, duplicate cleanup, queue bounds,
   motion/keyboard/button ordering, fractional and non-finite deltas, wheel caps,
   axis direction, observed/coalesced mode transitions, and disconnection. Prove
   that every admitted press retains a release path while the runtime remains
   available and reporting remains enabled.
4. Add a deterministic PTY integration test whose child configures raw input
   with echo disabled before enabling reporting and announcing readiness. Cooked
   line discipline would buffer reports until newline and echo them back into
   the emulator. Read an exact byte count and return a printable hex result.
   Synchronize through child readiness and observed modes rather than sleeps,
   and bound waits so failures cannot hang the suite. Verify silence after
   disable by following a suppressed report with a known keyboard sentinel and
   asserting that the child receives only that sentinel. Also verify mouse and
   keyboard delivery order while reporting is enabled.
5. Run native htop clicks and wheel scrolling, plus dragging in a mouse-enabled
   editor on macOS and Linux. Exercise release outside the window, focus loss,
   Shift override with physical wheel and trackpad, scrollbar use, scrollback,
   resizing, and return to a shell.
   Record host, application versions, commands, and observed results. Xvfb
   startup or synthetic routing tests alone do not prove native capture.
6. Update user-facing documentation with supported modes and local interaction
   rules. Run `mise run check` during implementation, focused Cargo tests through
   `mise exec -- cargo test`, and `mise run verify` before broad handoff. Run
   `mise run bench:scroll` to verify the existing scroll regression budgets.
   Keep any unavailable native-platform evidence explicitly outstanding.

Tests must assert observable routing and encoded output. Prefer a new test
failing at its intended assertion before implementation, and confirm the runner
actually executes it. Do not add a new benchmark framework for this feature;
bounded queue tests and the existing scroll benchmark cover the identified
traffic and regression risks. Focus and cancellation progress must also be
testable without requiring a paint callback.

## Alternatives and compatibility

Encoding reports in GPUI would duplicate terminal-mode interpretation and make
future clients reproduce the same logic. Preserve structured input and core
encoding instead. Keeping all new routing logic inside `desktop.rs` would make
gesture transitions harder to test; a small private module is sufficient.

This is an additive in-process protocol change. There is no serialized protocol
or persistent-state migration. Update all exhaustive constructions and relevant
input accounting, retain keyboard/paste behavior, and preserve the protocol's
empty dependency set. No feature flag is planned.

## Remaining native verification

- Verify outside-window motion and release for all three buttons on macOS and
  the Linux backend under test. Existing backend support makes this an
  acceptance check, not a prerequisite to designing a new capture mechanism.
  Any failure needs a concrete reproduction before changing platform plumbing.
- Check macOS Shift-wheel deltas with both a physical wheel and a trackpad.
  Axis remapping is an unverified possibility; preserve vertical local scrolling
  without turning legitimate horizontal application input into vertical input.
- Exercise fast wheel input with the initial 32-step cap. Record any normal
  gesture that hits it before changing the limit.

No unresolved product decisions remain. These native checks may expose
implementation work, and must remain outstanding until observed on the named
platforms.

[issue]: https://github.com/jimeh/huterm/issues/5
[xterm]: https://www.invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-Mouse-Tracking
