# Clickable terminal links and file drops

Status: proposed implementation plan, following read-only investigation on
2026-09-09. Product implementation has not started.

## Outcome and settled direction

Make terminal URLs clickable with a held modifier, including when only the
last few characters of a wrapped URL are visible. Dropping files or folders
inserts their absolute paths with backslash escaping, without executing them.
Both features must work with Alacritty and Ghostty on macOS and Linux.

Settled decisions:

- Resolve links on demand from canonical terminal state. Do not eagerly
  annotate all visible logical lines or introduce a scrollback-wide link cache.
- Support plain-text URLs and explicit OSC 8 hyperlinks through one desktop
  interaction path. Explicit destinations take precedence over text detection.
- Default to Cmd on macOS, adding Shift when application mouse reporting is
  active. This preserves ordinary mouse interaction in programs such as tmux
  and herdr. Use Ctrl and Ctrl+Shift respectively on Linux.
- Make the base link modifier configurable. Keep Shift as the application
  mouse bypass modifier, consistent with Huterm's existing mouse routing.
- Use backslash escaping for dropped paths, for example
  `/Users/jimeh/Documents/_Projects/Murder\ Burgers`.
- Keep OS URL opening, native drops, hover state, and gestures in GPUI. Core
  owns buffer inspection; protocol values must remain dependency-neutral.

The remaining defaults below are implementation proposals, not previously
confirmed user preferences. They allow work to proceed without inventing a
general link-action framework.

## Proposed scope and defaults

Start with HTTP and HTTPS destinations, including OSC 8 links whose labels
differ from their targets. Do not interpret bare domains, filesystem text,
custom schemes, or semantic-history commands in this delivery. Validate the
scheme and reject control characters before passing a destination to GPUI's
OS opener. Never interpolate a URL into a shell command.

Proposed configuration, with names finalized alongside existing config types:

```toml
[terminal]
links = true
link_modifiers = "cmd" # Default is "ctrl" on Linux.
```

Accept nonempty combinations of `cmd`, `ctrl`, `alt`, and `shift`, using
hyphen-separated syntax. Reject duplicates, unsupported tokens, and bare keys.
On macOS, reject any combination containing `ctrl` with a diagnostic explaining
that GPUI converts Control-left into Right and removes Control. Do not accept
a setting that can highlight a link but cannot activate it. Linux continues to
accept `ctrl`; no native Control-click override is included in this delivery.
Match the exact configured combination outside application mouse mode. When
mouse reporting is enabled, require the union of that combination and Shift.
If Shift is already configured, do not require an additional modifier.
Require this extra Shift even when viewing scrollback of a mouse-enabled
terminal, so the chord does not change as the viewport moves.

Successful reload applies to existing views and cancels pending link gestures
and hover results. Invalid reload keeps the previous settings. Configuration
must not become a keyboard binding that consumes normal Cmd/Ctrl shortcuts.

Underline only the hovered link while the effective modifiers are held, use a
pointing-hand cursor, and show its complete destination in a viewport-clamped
preview. Modifier changes with a stationary pointer must update this state.
Use a paint overlay rather than changing glyph-layout cache keys. Preview text
may be visually elided; the stored destination must remain complete.

For drops, support multiple items in supplied order, separated by one space,
with one trailing space. Do not add a leading space, newline, directory slash,
or Enter. Preserve the supplied absolute spelling and symlinks; do not resolve
paths with filesystem I/O or canonicalization. Remote-shell path translation
and automatic file upload are outside scope.

## Why lookup is the appropriate cache boundary

The adapters in `crates/huterm-core/src/engine/alacritty.rs` and `ghostty.rs`
cache visible `Arc<TerminalRow>` values using render damage. These rows have
neither stable logical-line IDs nor content revisions covering offscreen
continuations. BufferPoint coordinates are relative to the live bottom.
Both engines advance the terminal generation after every processed output
chunk and resize, but scrolling alone does not advance it.

A visible suffix can remain unchanged while its offscreen URL prefix changes.
Reusing the visible row therefore does not prove that a cached destination is
valid. A whole-generation cache is safe but invalidates on unrelated output.
A content-keyed logical-line cache still needs to read the full logical line
to construct its key. Ghostty tracked references preserve locations, not
content revisions, and do not supply a common cache contract with Alacritty.

Cache only the latest interaction result, including a negative result, against
the exact displayed snapshot and request identity. Reuse it within its resolved
range. A negative result is reusable only for the queried cell unless the
resolver explicitly certifies a larger range. Keep any additional matcher
cache private and bounded; it is not required for the first implementation.

Upstream source supports both approaches, but does not establish Huterm's
performance. Ghostty resolves the hovered logical line under its renderer
mutex; iTerm2 cancels superseded hover lookups; Alacritty bounds searches past
the viewport. WezTerm lazily annotates logical lines and maintains implicit-link
scan flags. Huterm will reuse the on-demand principle without acquiring engine
or Mux locks from GPUI callbacks. See the source references below.

## Link lookup and snapshot coordination

Extend the existing asynchronous snapshot operation with an optional viewport
cell to resolve. Proposed names are illustrative, not a required public API:

- Request: existing optional scroll command, optional hovered cell, and client
  request identity.
- Reply: existing complete snapshot plus a link lookup outcome associated with
  that exact snapshot. Outcomes distinguish match, no match, scan limit, and
  unavailable with a bounded diagnostic. A request without lookup is distinct
  from a lookup that found no match.
- Match: owned destination, inclusive buffer range or explicit visible spans,
  and source kind, plain text or OSC 8. Preserve enough visible label/cell
  information to validate a held gesture across snapshots. No native references
  cross the boundary.

The runtime applies the requested scroll, builds the snapshot, and resolves
the requested cell before processing more terminal mutations. Convert the cell
using the actual returned viewport and grid dimensions, never a predicted
scroll offset. Reject out-of-grid coordinates rather than clamping them onto
a link. Buffer inspection must not scroll, resize, or consume render damage.

Lookup is an optional capability with a nonfatal error boundary. Convert
extraction, mapping, matcher, and bounded-buffer failures into the lookup's
unavailable outcome, retaining the successful snapshot. Never propagate a
lookup-only error into the outer snapshot `Result`: `complete_snapshot_request`
sets the runtime closing gate on that error, and the client latches snapshot
failure. Actual snapshot failures retain that existing fatal behavior. Surface
lookup diagnostics without calling `ScrollController::fail()` or emitting a
terminal `Failed` event. Bound their frequency and do not retry immediately
without a new eligible interaction or content update.

Adapters expose bounded logical-line text with a byte-to-cell mapping, wrapping
boundaries, and explicit hyperlink information to a shared core resolver.
Preserve hard breaks, combining characters, wide-cell spacers, and valid URL
punctuation. Never join separate hard lines. Inspect OSC 8 at the target before
running the plain-text matcher. Bound explicit destinations and highlight
expansion as well as plain-text extraction.

This pairing avoids a generation retry loop: requesting lookup from an old
snapshot and rejecting it whenever output arrives could starve hover during
continuous output. Instead, display the returned snapshot and lookup together.
Coordinate this through the existing snapshot pump, not a second competing
snapshot consumer. Ordinary requests carry no lookup unless link interaction
is enabled by the current modifiers.

Hover and modifier changes must explicitly make the request pump runnable even
when terminal content and scroll offset are unchanged. Update the coalesced
lookup intent, mark snapshot work pending through the controller's existing
invalidation mechanism or an equivalent explicit request reason, and call
`start_snapshot_if_needed`. Register GPUI modifier-change handling so pressing
the chord over a stationary pointer requests lookup immediately. Releasing it
clears feedback and invalidates pending hover intent without requiring a new
snapshot. Do not synthesize a scroll command to trigger work. Controls are
serviced on the runtime loop with a 2 ms idle poll; measure end-to-end latency
instead of assuming the async reply is immediate.

Keep at most one snapshot/lookup request in flight per visible view and one
replaceable pending intent. Pointer movement within a cell does not enqueue
more work. When a reply's hover identity is obsolete, the snapshot may still
be applied under existing snapshot ordering rules, but its link result must
not be applied. Tab hiding, focus loss, resize, scroll intent, and config reload
invalidate the relevant hover identity. Keep interaction request identity
separate from snapshot identity: publishing a reply's own snapshot must not
invalidate that reply before its paired lookup is applied.
Do not immediately retry a scan-limit or operational failure in a tight loop.

A new displayed snapshot invalidates an older lookup even if its terminal
generation is equal: scrolling can change the viewport without changing that
generation. Conversely, an invalidation notification alone need not discard
a link while its paired snapshot is still displayed. Recompute when publishing
the next snapshot. A held click can survive replacement when the new paired
lookup proves the same target and visible link content at the same grid span,
as specified below. Snapshot allocation or generation change alone is not a
reason to cancel it.

Keep limits as named internal constants initially. Select concrete byte and
row budgets using the long-line fixtures before implementation is accepted.
Reaching a limit must produce no activatable truncated target. Configure the
matcher to avoid unbounded backtracking. Start with bounded synchronous work
on the engine owner; introduce a background detector only if measured latency
requires it. Any worker must receive immutable owned text, never engine handles.

## Mouse gesture ownership

Decide the left-button owner at press and retain it until release or explicit
cancellation. An effective link chord with a confirmed hovered link consumes
the complete gesture before application mouse routing and local selection.
While lookup is pending, a chord-click must not open later from an unsolicited
async result. Suppress that ambiguous gesture and require another click after
feedback is available. With a confirmed no-match result, retain ordinary local
selection behavior; Shift still bypasses application reporting.

Open only on release over the same link with the required modifiers still held,
provided the pointer has not crossed the drag threshold. Capture the destination,
source kind, visible link text/cell mapping, and viewport spans at press. While
held, request paired lookups for the press cell on subsequent content snapshots.
Preserve the gesture only if every applied replacement proves that the target,
source kind, visible link content, and spans still match. Use viewport spans
for this comparison, not live-bottom-relative buffer coordinates, which can
shift after unrelated output. At release, validate against the lookup paired
with the currently displayed snapshot and open only the captured destination.
Do not defer activation until some later reply or substitute a new destination.

Cancel on a changed destination or label, moved span, no-match/unavailable/
scan-limit result, or replacement snapshot lacking a matching lookup. Also
cancel on Escape, blur, tab change, scroll intent, resize, required-modifier
release, or configuration change. Unrelated output outside the link must not
cancel an otherwise validated gesture. Consume Escape while canceling a
link-owned press, before raw terminal input observers or text handlers can
forward it; with no link-owned press, preserve existing Escape handling.
Cancellation must continue to consume the eventual mouse release and must not
allow the canceled gesture to re-arm when a later lookup succeeds.

Do not steal a gesture already owned by an application when modifiers change
mid-drag. Deliver its owed release through the existing mouse ownership rules.
Likewise, changing modifiers during a selection must not turn it into a link
click. Preserve macOS Control-left normalization, scrollbar capture, tab
reorder capture, and the existing hidden-view guards.

Links remain usable in retained exited history. File drops remain unavailable
there. Close confirmation owns focus and prevents either interaction.

## File-drop implementation

Use GPUI's `ExternalPaths`, `can_drop`, and `on_drop` on the terminal target.
Validate against the terminal viewport and visible/live state; do not allow
tab chrome or a confirmation overlay to route a drop into the terminal.
Focus the accepted target and reuse paste's scroll-to-bottom behavior.

GPUI translates native file-drag updates into mouse movement and release
events. Detect active external drags before ordinary terminal mouse handlers,
and prevent synthetic movement/release from becoming application mouse input,
selection, or a link click. Preserve cleanup of any previously owned gesture;
do not merely discard an application release that is still owed. Do not
interfere with Huterm's internal tab-reorder drag type.

Create a pure path-list formatter. Require absolute paths and valid UTF-8;
reject control characters, including newlines, tabs, ESC, and DEL, instead of
silently changing a filename. Reject the entire drop if any item is invalid.
Escape shell-significant ASCII characters with backslashes, including spaces,
quotes, backslashes, dollar signs, backticks, glob characters, separators,
parentheses, braces, and shell expansion characters. Test the exact chosen
set against zsh and bash without executing user-supplied paths. Do not claim
universal quoting for every shell or interactive program.

Submit the resulting string as one `TerminalInput::Paste` through the existing
bounded input queue. Do not use the clipboard, raw key simulation, or direct
PTY writes. Check admission and report invalid/oversized/closed/full outcomes
through existing status feedback. Bound formatting allocation by the paste
capacity and reject oversized drops as a whole. Successful admission does not
guarantee delivery if the terminal subsequently exits, matching normal paste.
Core remains responsible for bracketed-paste encoding at dequeue time.

## Implementation sequence

1. Implement the path formatter and native drop target as a bounded change.
   Add formatting, admission, and raw-PTY coverage before native drop smokes.
   Document the proposed trailing-space and unsupported-filename behavior.
2. Add the shared bounded link resolver and adapter extraction methods. Prove
   complete offscreen resolution and mapping with identical fixtures for both
   engines. Verify lookups do not consume damage or mutate the viewport.
3. Extend snapshot requests/replies and the single desktop request pump.
   Test stale replies, replacement intents, same-generation scrolls, output,
   scan limits, errors, and retained exited history before mouse integration.
4. Add modifier configuration, hover decoration, preview, and gesture ownership.
   Test stationary-pointer modifier changes and application mouse capture.
5. Run native smokes and performance comparisons, then update README/config
   examples and relevant agent guidance with verified implementation constraints.
   Keep upstream source observations separate from verified Huterm behavior.

Prefer separate file-drop and link implementation commits or PRs. Do not add
an engine dependency patch, stable line IDs, a general mouse binding language,
or a persistent link index merely to complete this feature.

## Verification and acceptance

New behavioral tests must run through production entry points and be observed
failing at their intended assertion before the implementation passes where
practical. Existing green tests alone do not establish coverage.

Core fixtures for both engines must cover plain URLs, OSC 8 precedence, only
the final characters visible, URLs ending below the viewport, hard breaks,
punctuation, Unicode, wide cells, malformed destinations, scan limits, offscreen
prefix mutation, resize/reflow, history eviction, and alternate-screen changes.
Verify lookup leaves the viewport, future row damage, and old snapshots intact.
Inject a lookup-only failure through the combined request and assert that the
snapshot still arrives, the runtime remains open, and subsequent snapshots and
PTY input succeed. Keep a separate regression proving actual snapshot failure
still closes the runtime. Test unavailable and scan-limit outcomes for bounded
diagnostics and absence of immediate retries.

Desktop state tests must cover stationary modifier changes, captured mouse
mode, no-match selection, pending lookup clicks, press/release cancellation,
out-of-grid points, pointer movement across links, stale async replies, hidden
tabs, confirmation overlays, reload, queue bounds, and continued lookup progress
during continuous output. Use a fake OS opener to assert exact targets and
prove canceled gestures never open anything.
Explicitly complete a press/release across multiple snapshots caused by unrelated
output, with the same link content and geometry, and assert one opening of the
captured URL. Change only an offscreen destination prefix or an OSC 8 target
while preserving its visible label and assert cancellation. Cover label/span
changes, missing lookup results, and no re-arming after cancellation. Assert
Escape cancellation sends neither Escape nor an unmatched mouse release to
the PTY, while ordinary Escape without a link gesture still reaches it.

Test idle hover and stationary modifier activation with no PTY output and no
scroll changes: the pump must submit one lookup and remain bounded under
replacement intents. Cover modifier release while a request is in flight.
Config tests must reject macOS `ctrl` and `cmd-ctrl` combinations, retain the
working configuration on invalid reload, and accept the Linux Ctrl defaults.

Drop tests must cover multiple paths, spaces, quotes, backslashes, expansion
characters, Unicode, symlink spelling, invalid/control-character paths, size
limits, admission failure, exited targets, and drag cancellation. Raw-PTY
fixtures verify exact bytes with bracketed paste both on and off, no appended
Enter, and no leaked application mouse reports. Reject unsupported paths before
paste encoding can strip or reinterpret control bytes.

Extend or add discoverable Mise smokes for real AppKit and X11 modifier/click
delivery and native file-drop delivery. Run automated smokes before computer
use. Then verify Finder and a Linux file manager against both engines, including
mouse-aware applications. A direct GPUI callback test does not prove native
file-manager delivery. Native Wayland DnD requires a compositor session and
separate evidence; an Xvfb run cannot establish it.

Record a release baseline before changes using existing renderer/scroll tasks,
with separate Cargo targets for baseline and feature worktrees. Compare:

- Ordinary output and scrolling with link modifiers inactive.
- URL-heavy output with hover enabled, including continuous output.
- Repeated motion within one cell and across cells and links.
- A very long wrapped line, including content that exceeds the scan limit.

Retain existing benchmark gates. Add lookup elapsed time, end-to-end hover
latency, pending-request counts, and PTY responsiveness evidence. Require no
URL scans without active interaction, one in-flight request plus one pending
intent, and no unbounded retries or runtime stalls. Choose numerical lookup
budgets from the measured baseline and record them in the implementation;
do not present source inspection as proof of a frame-time target.

Run focused tests during implementation, `mise run check`, and
`mise run verify` before broad delivery. Use the existing build wrapper and
Ghostty preparation tasks. Run `mise run license` if dependencies change.
Report native platform gaps and benchmark host limitations explicitly.

## Source references

Local investigation centers on `crates/huterm-core/src/engine.rs`, both engine
adapters, `terminal.rs`, `crates/huterm-protocol/src/lib.rs`, and GPUI's
`desktop.rs`, `mouse.rs`, `renderer.rs`, and `input_queue.rs`.

- [Ghostty on-demand lookup and capture modifiers][ghostty].
- [iTerm2 cancellable hover lookup][iterm].
- [Alacritty bounded search and activation revalidation][alacritty].
- [WezTerm implicit-link invalidation and scan cache][wezterm].
- [WezTerm bounded logical-line expansion][wezterm-lines].

[ghostty]: https://github.com/ghostty-org/ghostty/blob/448062571c5edf010b7490d06869b88b5ebf8f80/src/Surface.zig#L4298-L4430
[iterm]: https://github.com/gnachman/iTerm2/blob/d65c5f096bf515d34c22e5fa4fcd42d0989ad951/sources/TerminalView/PTYTextView%2BARC.m#L464-L546
[alacritty]: https://github.com/alacritty/alacritty/blob/d692748d3f61253ebe9f5094320120d22f6a046f/alacritty/src/display/hint.rs#L230-L329
[wezterm]: https://github.com/wezterm/wezterm/blob/d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b/wezterm-surface/src/line/line.rs#L488-L630
[wezterm-lines]: https://github.com/wezterm/wezterm/blob/d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b/term/src/screen.rs#L992-L1047

## Unresolved questions

No architectural question blocks implementation. HTTP/HTTPS-only opening,
exact base-modifier matching, trailing space after drops, and rejection of
unsupported filenames are proposed product defaults for review. Scan limits
and numerical performance budgets must be settled from measurements before
the implementation is accepted. Native Wayland verification depends on an
available compositor environment.
