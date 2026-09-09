# Tab-bar visibility and fullscreen reveal

Status: implemented on 2026-09-09. Linux unit and production smoke validation
passed for both engines. macOS CI and physical display validation remain pending.

## Outcome

Hide the tab bar when a window has one tab by default. Add an opt-in fullscreen
mode that reveals the bar as an animated overlay when the pointer reaches its
attached edge. Showing and hiding that overlay must never resize the terminal
grid or send a PTY resize.

Support top, bottom, left, and right placement, both terminal engines, Linux
fullscreen, and both macOS fullscreen modes. Keep this behavior in the desktop
client; core session, tab, and terminal ownership do not change.

## Agreed behavior

Add these settings beside `tab_position` in `[window]`:

```toml
[window]
always_show_tab_bar = false
auto_hide_tab_bar_in_fullscreen = false
```

`always_show_tab_bar` controls whether a single tab reserves a visible bar.
Fullscreen auto-hide takes precedence when enabled, including when
`always_show_tab_bar` is true.

| Presentation | Tab count and settings | Result |
| --- | --- | --- |
| Windowed, or fullscreen without auto-hide | One tab, always-show false | Hidden; terminal reclaims bar space |
| Windowed, or fullscreen without auto-hide | Two or more tabs, or always-show true | Visible; bar space reserved |
| Fullscreen with auto-hide | Any nonzero tab count | Hidden until edge hover; revealed bar overlays terminal |

Fullscreen edge hover reveals even a single tab, keeping the new-tab button
reachable. Leaving fullscreen restores count-based visibility. Creating the
second tab or closing back to one can resize the grid in reserved presentation;
neither operation introduces reserved bar space while fullscreen auto-hide is
enabled. No-PTY confirmation hosts must not acquire an empty tab bar.

The macOS notch-height region is a reveal target for a top bar. The revealed
bar sits below the notch and overlays the terminal. This plan does not place
tab controls beside or behind the camera housing. Windowed native titlebar
controls retain their existing behavior when the tab bar is hidden.

Both settings participate in normal config reload. Existing configs receive
the new defaults; users who want the previous single-tab presentation can set
`always_show_tab_bar = true`.

## Interaction defaults

Start with a roughly 150 ms slide and a 300 ms dismissal delay after the pointer
leaves the revealed bar and activation region. These timings are tuning values,
not additional configuration settings. Use a narrow edge target on displays
without a notch; select its exact width during native verification.

- Slide inward from the configured edge. Reverse an in-progress animation
  smoothly if hover changes.
- Keep the bar open while dragging a tab or resizing a vertical sidebar.
  Recompute dismissal after the gesture ends.
- Do not reveal over an existing terminal drag or application mouse gesture.
  Preserve delivery of its release before allowing a reveal.
- Preserve terminal keyboard focus. Bar pointer input, including wheel events,
  must not reach the terminal behind it. Hidden bar bounds must not intercept
  ordinary terminal input beyond the activation region.
- Paint an opaque themed backing beneath the bar so terminal content does not
  show through its current translucent styling.
- Dismiss or reset transient reveal state on window deactivation, placement
  changes, and fullscreen transitions. Close confirmation remains above the
  overlay and suppresses reveal interaction.
- Preserve tab scrolling and preferred sidebar width across hide/show. Resizing
  an overlay sidebar changes its width without changing terminal dimensions.

## Current implementation and ownership

[`WindowConfig`](../../crates/huterm-gpui/src/config.rs) owns window settings.
[`WorkspaceView` and `ChromeLayout`](../../crates/huterm-gpui/src/desktop/windows.rs)
currently reserve bar space for every placement. `WorkspaceView` renders the
bar and owns tab scrolling, reorder capture, sidebar resizing, and the window
refresh pump. [`TabStrip`](../../crates/huterm-gpui/src/desktop/windows/tab_strip.rs)
owns tab geometry. The retained
[`TerminalView`](../../crates/huterm-gpui/src/desktop.rs) also derives its bounds
through `ChromeLayout` for painting, input, and PTY sizing.

Extend shared layout with hidden, reserved, and overlay presentation. Keep the
visibility decision in `WorkspaceView` and propagate the effective layout
policy to retained terminal views before activation or resize. Terminal views
must not independently infer visibility from incomplete tab-count information.

Keep overlay animation and dismissal state private to the window. Advance them
through the existing refresh pump, requesting frames while animation progresses.
`ChromeLayout` provides stable terminal bounds plus bar and activation geometry;
animation affects only bar painting and its corresponding hit bounds.
`TabStrip` continues to own scrolling, reveal, and drop geometry, using the
displayed overlay position consistently. Paint the overlay above the terminal
and below confirmation UI.

Animating the existing reserved layout is rejected because it would either
retain unused space or repeatedly resize the grid. A separate native popup
window is also unnecessary for the agreed below-notch rendering and would add
focus, stacking, and lifecycle coordination.

## macOS edge detection

The existing [`native_fullscreen` adapter](../../crates/huterm-gpui/src/native_fullscreen.rs)
returns display safe-area insets for non-native fullscreen. Native fullscreen
relies on AppKit to position content inside the safe area. Apple documents this
distinction in [`NSScreen.safeAreaInsets`][apple-safe-area]. Preserve that terminal
inset even while the bar is hidden.

Content-local hover alone may not observe the notch-height region in native
fullscreen. Investigate a read-only screen-coordinate pointer sample in the
existing macOS adapter, driven by the window refresh pump. Restrict activation
to the active fullscreen window and its current display. Convert screen and
window coordinates explicitly; account for display origins and scale changes.
Use current display geometry rather than fixed notch dimensions.

Verify physical-edge activation and movement from the notch region into the
bar on a real notched display. Native menu-bar reveal may affect pointer
delivery or cover app content. Test coexistence rather than changing system
menu-bar policy. If the proposed sampling cannot satisfy this behavior, record
the observed limitation before choosing a different native mechanism.

## Implementation sequence

1. **Add config and reserved visibility.** Update defaults, bundled config,
   config documentation, and parsing tests. Add the shared presentation policy
   and count-based geometry. Synchronize retained views on tab creation, close,
   activation, and reload. Verify one-to-two-to-one transitions for horizontal
   and vertical placements before adding animation.
2. **Prove the fullscreen activation boundary.** Exercise the existing observed
   fullscreen state and macOS screen geometry. Establish physical-edge and
   notch-region detection with native evidence before relying on it for the
   finished interaction. Handle screen changes and deactivation explicitly.
3. **Add overlay presentation and animation.** Implement window-owned reveal
   state, shared overlay geometry, themed backing, stacking, and input capture.
   Integrate tab scrolling, reorder, and sidebar resize without changing
   terminal bounds. Test interruption, dismissal, and active-gesture behavior
   while implementing them.
4. **Complete lifecycle and reload behavior.** Exercise fullscreen entry/exit,
   config reload while revealed, placement changes, tab count changes, close
   confirmation, and retained tab activation. Reset transient state without
   losing preferred sidebar width or terminal gesture ownership.
5. **Run native acceptance checks and document the feature.** Extend existing
   fullscreen/input smokes where they can prove behavior. Tune timing using
   real UI evidence, document the settings, and run the repository verification
   gate before implementation handoff.

## Verification and acceptance

Use focused Rust tests for config defaults and overrides, the visibility matrix,
layout bounds for all four placements, and reveal/dismiss transitions with
controlled elapsed time. Assert behavior rather than internal enum layout.
Cover small windows and safe-area insets. Verify that hidden and overlay
presentations have identical terminal bounds at every animation position.

Extend production desktop smoke coverage to observe terminal rows, columns,
and resize activity before, during, and after reveal. Equal final dimensions
alone cannot rule out an intermediate resize. Run against both engines. Check
that pointer and wheel input over the overlay do not appear in a raw-PTY
recorder, while input outside it remains usable. Verify drag release ownership,
new-tab interaction, and the one-to-two-to-one reserved-layout transition.

Use `mise run smoke:linux-fullscreen` and `mise run smoke:linux-input` on Linux,
and `mise run smoke:macos-fullscreen` and `mise run smoke:macos-input` on macOS,
with focused additions appropriate to their existing fixtures. Xvfb's limited
frame delivery can prove state and PTY invariants but cannot establish smooth
animation. Use native visual checks for slide direction, stacking, timing,
menu-bar coexistence, and notch-region activation. Include a notched display,
an unnotched display, and moving between displays where hardware is available;
state any unverified hardware cases.

Run `mise run check` during implementation and `mise run verify` before broad
handoff. This plan itself requires only
`mise run lint:docs:files -- docs/plans/tab-bar-visibility.md`.

## Implementation evidence and remaining native checks

The implementation uses a 2-point edge target, a 150 ms slide, and a 300 ms
hide delay. Shared layout keeps the terminal bounds independent of overlay
progress. A clipped opaque backing and terminal event guards isolate overlay
input while preserving releases owned by an existing terminal gesture.

The fullscreen smoke now checks all four placements, one-to-two-to-one tab
transitions, a revealed new-tab button click, terminal focus, raw-PTY pointer
isolation, and unchanged resize-request counts across reveal and dismissal.
Linux also checks wheel isolation. These checks run for both engines; macOS
uses the existing native mouse-event seam in both fullscreen modes.

Linux validation cannot establish macOS menu-bar coexistence, physical notch
activation, animation quality, or movement between physical displays. Synthetic
macOS mouse events do not move the physical cursor sampled by AppKit. The smoke
centers the physical cursor on the tested display and restores it on exit, so
the synthetic content events do not conflict with physical edge detection.
Physical notch and display traversal checks remain necessary before claiming
hardware coverage.

## Unresolved questions

No product decisions block implementation. Native verification must establish
whether screen-coordinate sampling reliably covers the notch region in both
macOS fullscreen modes, including system menu-bar reveal. Exact activation
width and animation timings remain subject to that verification.

[apple-safe-area]: https://developer.apple.com/documentation/appkit/nsscreen/safeareainsets
