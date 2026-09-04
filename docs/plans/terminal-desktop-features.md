# Terminal desktop features plan

Status: proposed for implementation.

## Outcome

Turn the proof of concept into a usable native desktop terminal without
weakening the runtime boundary or the renderer's measured performance. The
completed work provides:

- Responsive scrollback with precise wheel and trackpad input, a draggable
  position indicator, keyboard navigation, and bounded snapshot work.
- Native paste plus text selection and copy across visible scrollback.
- Startup configuration for font family, font size, and terminal colors.
- A macOS menu bar with the commands the application supports.
- An Apple Silicon `Huterm.app` bundle identified as `app.huterm.dev`.
- Consistent human-facing `Huterm` naming.

The runtime continues to own PTYs and canonical emulator state. Each GPUI
client owns its viewport, selection gesture, clipboard access, appearance, and
window state.

## Scope and non-goals

This milestone includes macOS and Linux behavior for scrolling, selection,
clipboard actions, and configuration. It packages only the macOS application.

Configuration loads once during startup. Opening the settings file does not
imply live reload. The first package is an unsigned development `.app`; signing,
notarization, a DMG, automatic updates, and release publication remain separate
work.

The first selection implementation is linear character selection. Word and
line selection, rectangular selection, hyperlinks, terminal mouse reporting,
search, and accessibility nodes remain outside this milestone. Selection may
auto-scroll through existing history, but output that changes the selected
terminal generation clears the highlight rather than silently rebasing it.

Full immutable viewport snapshots remain the client contract. Changed-row
deltas or an overscanned client cache are follow-up options only if the corrected
request and render path still misses its measured budget.

## Settled decisions

- Use `Huterm` for product names, titles, menus, documentation, diagnostics,
  and `TERM_PROGRAM`.
- Keep Rust packages, the executable, config directory, and repository name as
  lowercase `huterm`; keep environment variables conventionally uppercase.
- Use `app.huterm.dev` for GPUI application identity and macOS bundle metadata.
- Store config at `$XDG_CONFIG_HOME/huterm/config.toml`, falling back to
  `$HOME/.config/huterm/config.toml` on macOS and Linux.
- Keep appearance and viewport state client-owned.
- Preserve terminal color meaning in snapshots so clients apply their themes.
- Use GPUI actions for menu commands and application keybindings.
- Use a pinned `cargo-packager` behind a discoverable Mise task.

## Current seams and constraints

The protocol already distinguishes paste from composed text, and core already
applies bracketed-paste mode. GPUI only needs to read the platform clipboard and
enqueue the existing paste input.

The renderer and desktop view assume fixed font and cell dimensions. Font
configuration must replace those constants with one measured grid-metrics value
shared by initial window sizing, PTY resize, rendering, scroll input, scrollbar
geometry, and selection hit testing.

Cells contain resolved RGB foreground and background colors. Core has therefore
discarded whether a color was a terminal default, an indexed palette entry, or
explicit RGB, preventing client-specific themes.

Scroll input rounds every nonzero fractional delta to a row, drops an in-flight
snapshot when another event arrives, and observes replies only through the 16 ms
refresh loop. A deferred target can take two polling intervals before paint.
The renderer also compares rows at the same screen index after a viewport shift,
rebuilding rows whose content merely moved.

The retained row and glyph-layout caches are part of the performance baseline.
Selection and the indicator must add paint work without invalidating text
layouts.

## Design

### Configuration and grid metrics

Add a private `config` module to `huterm-gpui` for path discovery, defaults,
TOML parsing, validation, and initial-file creation. It exposes plain Rust values
rather than GPUI types. A generated macOS file starts with:

```toml
[font]
family = "Menlo"
size = 14.0

[theme]
foreground = "#c5c8c6"
background = "#1d1f21"
cursor = "#ffffff"
selection = "#264f78"
ansi = [
  "#1d1f21", "#cc6666", "#b5bd68", "#f0c674",
  "#81a2be", "#b294bb", "#8abeb7", "#c5c8c6",
  "#666666", "#d54e53", "#b9ca4a", "#e7c547",
  "#7aa6da", "#c397d8", "#70c0b1", "#eaeaea",
]
```

Reject unknown keys, malformed colors, an ANSI palette with a length other than
16, empty font names, non-finite sizes, and sizes outside `6.0..=96.0`. A
missing file uses platform defaults: Menlo on macOS and the generic `monospace`
family on Linux. An invalid file also uses defaults, but reports the exact path
and error in the status area and on stderr.

`HUTERM_CONFIG_FILE` overrides discovery. Settings creates parent directories
and writes the documented default only when no file exists, then asks the OS to
open it. It never overwrites an existing file, including an invalid one.

Resolve the regular font before opening the window. Derive width, height,
baseline, underline, and strikeout positions from that font and size. Store them
in one immutable `GridMetrics` shared by the view and renderer. Report fallback
from a missing font once rather than repeatedly resolving it.

### Client-resolved terminal colors

Replace resolved cell RGB fields with a dependency-neutral value like:

```rust
pub enum CellColor {
    DefaultForeground,
    DefaultBackground,
    Cursor,
    Indexed(u8),
    Rgb(Rgb),
}
```

Core maps every Alacritty case explicitly. Unmodified ANSI names, including
bright names, and indexed colors remain `Indexed`. Foreground,
bright-foreground, and dim-foreground map to `DefaultForeground` with cell
intensity flags preserved. Background and dim-background map to
`DefaultBackground`; cursor maps to `Cursor`. If OSC 4 or another dynamic color
sequence overrides the relevant Alacritty color-table slot, core emits its
explicit `Rgb`. Explicit true color remains `Rgb`.

GPUI resolves semantic values through the theme immediately before preparing
backgrounds, glyph colors, decorations, and the cursor. Snapshot an effective
cursor override separately from cursor shape and position so an OSC cursor color
takes precedence over the configured role. Theme resolution covers all 256
indexed colors: configured entries 0 through 15, then the standard color cube
and grayscale ramp.

Color must not enter glyph-layout cache keys because it does not affect glyph
geometry.

### Scroll controller

Add a private `huterm-gpui` scroll controller owning:

- Desired and displayed bottom offsets.
- The latest history limit.
- A signed precise-pixel remainder.
- One in-flight snapshot request and one deferred latest target.
- Scrollbar drag state and the active-style fade deadline.

`TerminalSnapshot` reports the actual clamped viewport used to construct it.
Displayed offset always comes from that returned viewport.

Accumulate precise wheel and trackpad pixels, moving whole rows only after the
remainder crosses measured cell height. For GPUI line deltas, one reported line
equals one terminal row and a magnitude of three moves three rows. Reversing
direction replaces an incompatible remainder so an old gesture cannot delay the
new one.

Start a request as soon as desired whole-row offset changes. Replace the
polling-only response with `async_channel::bounded(1)`: `SnapshotRequest`
exposes an awaitable receive path, and a GPUI entity task awaits it, applies the
result on the UI thread, calls `cx.notify()`, and starts a deferred request in
that completion callback. Every snapshot, including invalidation-triggered
requests, uses this path. The 16 ms pump may drain and coalesce terminal events,
but it never observes snapshot completion.

Never discard a reply because newer input arrived, and never allow more than one
snapshot request in flight. An intermediate response may display, followed
immediately by the deferred latest target. This provides continuous progress
without a control-channel backlog or another timer tick.

Invalidation and scrolling share the coordinator. A new terminal generation
marks current viewport content dirty without creating a concurrent request.
Typing or pasting sets desired offset to zero and requests the live view.

When above the live bottom and history grows, increase offset by the observed
history-size delta to keep content pinned. Clamp when history clears or old rows
are evicted. Exact pinning after the history buffer is already full is not
guaranteed by the current contract; that requires stable row identity or a
core-reported cumulative scroll count.

Align retained rows by screen shift:

```text
(new_offset - old_offset) - (new_history_size - old_history_size)
```

For static scroll this reduces to viewport-offset difference. Reuse an
overlapping prepared row only after its cells match; prepare exposed or changed
rows. Rebuild after resize, incompatible generation changes, or jumps at least
as large as the viewport. The history term prevents reuse of wrong rows while a
viewport is pinned above growing output.

Do not add prefetch or deltas before measuring. If full snapshot construction
remains the bottleneck, add an overscanned contiguous row-window request next.
Do not move viewport state into canonical emulator state.

### Scroll position indicator

Paint a narrow right-edge overlay without reserving width or changing PTY
columns. Hide it when there is no history.

Derive thumb length from visible rows divided by total buffered rows, with a
minimum usable size. Position it from displayed offset and history limit,
with a 2 px top inset and an 8 px bottom inset. Add 2 px of internal track
padding above and below the thumb. Use the padded travel range for drag
mapping too, and reduce the insets proportionally in very short windows.

Support dragging, page-sized track clicks, Scroll Page Up, Scroll Page Down, and
Scroll to Bottom. Show a short label such as `1,284 lines up` at the bottom-right
of the viewport, with clearance for the expanding track. Hide the
label at zero while allowing the thumb to fade normally. Derive both the
thumb and label from the displayed snapshot offset, render the label on an
opaque surface, and fade the whole indicator after two seconds without input.
The fade lasts 400 ms and also applies at live bottom. Hovering or dragging
keeps the indicator visible and restarts the delay when interaction ends.
While visible, hovering expands the thumb and reveals a faint track over 180 ms
with an ease-out animation. Expand the click target immediately. After four
seconds without hovering or dragging, animate back to the narrow indicator.
Dragging holds the expanded state even outside the window. The overall idle
fade may hide it sooner. A fully faded indicator does not capture clicks or
reveal on hover.
Continue drag tracking outside the window and clamp at either history endpoint.

Reserve local scroll shortcuts before terminal key translation. Hide local
scroll affordances when alternate-screen content has no scrollback. Terminal
mouse reporting remains later work.

### Selection and copy

GPUI owns selection state and mouse interaction. Represent endpoints in
dependency-neutral buffer coordinates derived from returned viewport, visible
row, and column:

```rust
pub struct BufferPoint {
    pub rows_from_live_bottom: usize,
    pub column: u16,
}
```

Store terminal generation with the range. This keeps selection stable while
scrolling an unchanged buffer and detects stale output. The existing generation
advances for every processed PTY byte batch, including title-only, bell, or
query-response batches. Invalidation is intentionally conservative: any such
change clears selection rather than risking stale text.

Add a read-only runtime request accepting generation and ordered range. Core
validates both, maps to private emulator coordinates, and extracts from canonical
scrollback. Core owns soft-wrap, tab, combining-character, wide-character, and
trailing-blank semantics; it does not retain client selection state.

On release, request and retain selected text with the visual range. Copy writes
it to the platform clipboard. A stale or invalid range clears selection and
leaves the clipboard unchanged.

Paint selection after cell backgrounds and before glyphs, decorations, and
cursor. Selection-only changes must not rebuild rows or layouts. Dragging beyond
an edge asks the scroll controller for bounded automatic scrolling and updates
the endpoint after each returned viewport.

Unmodified left drag selects because terminal mouse reporting is absent. When
mouse reporting arrives, Shift-drag remains explicit local selection.

### Clipboard actions and menus

Add actions for About, Paste, Copy, Settings, Toggle Full Screen, Scroll Page
Up, Scroll Page Down, Scroll to Bottom, Hide Huterm, Hide Others, Show All, and
Quit. Menus and keybindings use the same handlers. About presents a platform
dialog attached to the terminal window, not another top-level window. Visibility
actions call GPUI's application methods.

Paste reads plain text and sends existing `TerminalInput::Paste`, preserving
newlines and letting core apply bracketed paste. Copy is available only with
valid extracted text.

Default bindings are:

- macOS: `Cmd+C`, `Cmd+V`, `Cmd+,`, and `Ctrl+Cmd+F`.
- Linux: `Ctrl+Shift+C`, `Ctrl+Shift+V`, and `F11`.
- Both: Shift-modified Page Up and Page Down, plus Scroll to Bottom.

Plain `Ctrl+C` remains terminal input.

Install handlers on the focused terminal element and let every reserved action
stop propagation, even when unavailable. Copy with no selection is a no-op but
must not send `0x03`. The global observer translates only when GPUI reports no
handled action and also rejects exact reserved chords defensively. Test this
contract: GPUI reports an action to the observer only after its handler stops
propagation.

The macOS menu bar contains:

- Huterm: About Huterm, Settings..., Services, Hide Huterm, Hide Others, Show
  All, and Quit Huterm.
- Edit: Copy and Paste.
- View: Scroll Page Up, Scroll Page Down, Scroll to Bottom, and Toggle Full
  Screen.
- Window: Minimize and Zoom, which GPUI implements directly.

Do not add disabled commands for windows, tabs, or splits the app cannot create.

### Naming and macOS packaging

Use human-facing `Huterm` consistently in source and maintained documentation.
Update `TERM_PROGRAM`, but do not rename crates, the binary, uppercase
variables, or the repository.

Pin `cargo-packager` through Mise after the three-day release-age policy permits
the selected version. Add `package:macos` to produce Apple Silicon `Huterm.app`
with:

- Bundle and GPUI application identifier `app.huterm.dev`.
- Version sourced from Cargo package metadata.
- Developer Tools application category.
- A committed `.icns` icon.
- Required `Info.plist` values and resources.

Keep packaging configuration in one root-level file or manifest section and a
runtime `APP_ID` constant. GPUI's macOS `set_app_id` is a no-op, so `Info.plist`
is authoritative for the packaged process; the constant supplies other
platforms and diagnostics. A package test compares the manifest, built
`CFBundleIdentifier`, and runtime-reported constant with `app.huterm.dev`. Do
not duplicate it in hand-written scripts.

A Finder launch starts its shell in the user's home, not an arbitrary inherited
directory. On macOS, pass `-l` as the shell's first argument for login-shell
initialization. Do not synthesize login `argv[0]` with
`CommandBuilder::from_argv`; portable-pty resolves and reuses its first entry as
the executable path. For a packaged macOS launch only, if `LANG`, `LC_CTYPE`,
and `LC_ALL` are all absent, set `LANG=en_US.UTF-8` without overriding an
inherited locale. A future working-directory setting can override home.

Add direct, deliberately pinned `serde`, `toml`, and `async-channel`
dependencies for configuration and awaited replies. They follow the repository
release-age, lockfile, source, and license checks.

## Implementation sequence

1. Normalize naming, define runtime `APP_ID`, and add the package drift check.
2. Add config and validation. Resolve the font once and replace fixed geometry
   with shared metrics. Verify PTY, window, and renderer dimensions.
3. Preserve semantic colors and resolve them in GPUI. Cover defaults, indexed
   colors, true color, reverse video, dim text, named roles, and OSC overrides.
4. Add returned viewport metadata, the request coordinator, and awaited reply
   wakeups. Prove fractional input, bounds, progress, clamping, and invalidation.
5. Add viewport-aware row reuse and the indicator. Build `bench:scroll` and meet
   its budget before selection work.
6. Add actions, paste, Settings, scroll shortcuts, and macOS menus. Prove app
   chords do not leak while terminal control keys still do.
7. Add buffer-coordinate selection, core extraction, paint, auto-scroll, and
   copy. Prove stale-generation handling.
8. Add pinned packaging and its Mise task. Verify metadata, launch, settings,
   login environment, working directory, and packaged shutdown.
9. Run broad checks, performance comparisons, manual desktop validation, and
   independent code review. Record exact revisions and environments.

## Verification strategy

### Focused automated coverage

- Config: missing, valid, malformed, unknown, and out-of-range values; override;
  default creation; refusal to overwrite.
- Protocol and engine: every semantic role, indexed cube and grayscale edges,
  OSC overrides, returned viewport clamping, and buffer-coordinate bounds.
- Scroll controller: signed fractional accumulation, reversal, multi-line
  deltas, limits, rapid replacement, two replies without a timer tick, one
  in-flight request, scroll during an in-flight invalidation request, pinned
  growth, a full history ring, and return to bottom on input.
- Renderer: use distinct nonblank row content and rebuild counters to prove
  correct reuse for static movement and pinned growth. Prove selection-only
  changes with a new snapshot allocation do not rebuild layouts, while resize
  or changed content does.
- Indicator: no history, top, middle, bottom, minimum thumb, drag, track clicks.
- Selection: both range directions, wraps, hard breaks, tabs, combining and wide
  cells, trailing blanks, viewport changes, auto-scroll, stale generations, and
  clipboard preservation on failure.
- Input: platform and local chords, unavailable Copy still consuming its chord,
  both paste modes, and plain `Ctrl+C` terminal input.
- Packaging: `Info.plist`, identifier and `APP_ID` equality, version, icon,
  executable, architecture, home startup, login initialization, and UTF-8 locale
  in a sanitized Finder-like environment. Capture the shell argument vector and
  assert `-l` directly rather than inferring it from profile effects.

New behavioral tests should fail at their intended assertion before the
implementation makes them pass. Existing suites are regression evidence, not
proof of new behavior.

### Scroll performance evidence

Add release-mode `bench:scroll`. An environment-gated scripted driver injects a
deterministic sequence into the same controller entry points GPUI uses, not a
benchmark-only approximation. Prepare 10,000 uniquely numbered nonblank rows
and drive single-row movement, fractional deltas, pages, and large thumb jumps
through a 100 by 32 viewport. A second phase keeps output flowing while pinned
and injects scroll input during an invalidation-triggered request, exposing any
timer-dependent reply handling.

Attach monotonic request sequence and requested offset to each sample. Record
runtime duration around `TerminalEngine::snapshot`, client preparation and paint
encoding, and time from desired-offset injection to paint with the matching
returned offset. Keep timing in environment-gated diagnostics, not protocol.
Report:

- Requests started, completed, coalesced, maximum concurrent, and queued.
- Rows reused and rebuilt.
- Snapshot, preparation, and paint-encoding timings.
- Desired-offset-to-matching-paint latency by sequence and returned offset, plus
  timer-wait time so polling regressions are visible.

After warm-up on the recorded environment, require:

- At most one snapshot in flight and no unbounded queue.
- A static one-row move rebuilds at most the exposed row.
- Median combined snapshot, preparation, and paint CPU below 8 ms and p95 below
  the 16.7 ms 60 Hz frame budget.
- P95 desired-offset-to-matching-paint below 33.4 ms under continuous input.
- In same-host before/after runs with five warm samples, at most 15% increase in
  median combined `bench:renderer` preparation and paint time, and no component
  more than 25% slower.

State that CPU preparation and paint encoding do not prove GPU presentation.
Record revision, profile, OS, hardware, display scale, and GPUI version.

On macOS, use Instruments or equivalent counters during sustained trackpad and
scrollbar use. Confirm 60 Hz presentation without request accumulation. If
headless Linux lacks presentation callbacks, gate completion-time snapshot CPU,
input-to-snapshot latency, wakeup delay, returned offsets, and queue behavior
there. Keep row reuse under deterministic unit tests. Apply the combined paint
CPU and input-to-paint budgets only when a host produces enough paint samples.

### Repository and manual validation

Run focused Cargo tests while implementing, then:

```sh
mise run verify
mise run bench:renderer
mise run bench:scroll
mise run package:macos
```

On macOS, test unpackaged and packaged builds with static history, continuous
output, and an alternate-screen program. Exercise wheel and trackpad, direction
changes, pages, thumb dragging, bottom jump, cross-viewport selection, copy,
multiline paste, Settings, menus, fullscreen, and quit with a live child.

Change font, size, and every theme role, restart, and confirm rendering, hit
testing, PTY pixel size, scrollbar, and selection stay aligned. Launch from
Finder and confirm identity, menu name, home directory, login shell, UTF-8
locale, config path, and no orphan after quit.

On Linux, repeat clipboard, scroll, selection, config, and Xvfb smoke coverage.
Packaging remains macOS-only.

## Risks and fallback points

- GPUI menu and clipboard APIs are pre-1.0. Keep them inside `huterm-gpui`.
- Font metrics vary by platform and fallback. Resolve once and use everywhere.
- A theme must not override explicit true color or OSC-updated colors.
- Output can invalidate selection. Prefer an explicit stale result.
- The current contract cannot perfectly pin after the 10,000-row ring starts
  evicting. Do not add row IDs unless validation justifies them.
- If corrected full snapshots miss budget, add bounded overscan before general
  deltas. Retain full snapshots as compatibility and rollback until proven.

## Acceptance criteria

The milestone is complete when:

- Wheel, trackpad, keyboard, track, and thumb scroll without starvation,
  backlog, or visible jumps after input stops.
- Every snapshot reply wakes GPUI directly; scroll progress never depends on the
  16 ms event poll, including while output arrives.
- The indicator represents returned viewport and can return to live bottom.
- The benchmark meets queue, reuse, CPU, latency, and regression budgets.
- Paste works in normal and bracketed modes.
- Selection and copy preserve terminal semantics and reject stale ranges.
- Restarted font and theme config controls all geometry and colors, with useful
  invalid-file errors.
- macOS menus expose working standard actions and reserved chords never leak.
- Apple Silicon `Huterm.app` reports `app.huterm.dev`, starts a login shell in
  home with UTF-8 locale, and cleanly stops its child.
- Maintained prose says `Huterm`; lowercase technical names and uppercase
  variables keep conventional spelling.
- `mise run verify` passes, both platforms have planned evidence, and dependency
  changes pass license and source policy.

## Open questions

None. Signing, notarization, universal binaries, live config reload, stable row
identity after scrollback eviction, and terminal mouse reporting are explicit
follow-ups rather than blockers.
