# macOS 14 compatibility cleanup

Status: audited on 2026-09-23. The minimum-version change is approved for the
[Quake scheduling work](quake-event-scheduling.md). Items below are tracked
work, not a claim that the cleanup has been implemented.

## Minimum-version policy

Require macOS 14.0 or later. Align the packaging declaration in `Cargo.toml`,
`minimumMacosVersion` in `scripts/release-macos.ts`, package tests, and the
updater smoke bundle in `scripts/check-native-updater.ts`. Set the compiler and
linker deployment target for direct Cargo builds and both universal slices;
changing `LSMinimumSystemVersion` alone is insufficient. State the requirement
in the installation and development documentation.

Before this change, packaging declared 10.15.7, while the freshly built arm64
idle benchmark reported Mach-O `minos 11.0`. This observation does not establish
the Intel slice's minimum. Verify both rebuilt slices independently.

The Quake clock can use `NSScreen::displayLinkWithTarget:selector:`, available
since macOS 14, without a pre-14 implementation. Keep this clock scoped to
active Quake animation and preserve GPUI's existing frame ownership.

## First-party cleanup in this PR

After enforcing the new minimum, remove two availability branches in
[`native_fullscreen.rs`](../../crates/huterm-gpui/src/native_fullscreen.rs):

- `screen_safe_area` checks `respondsToSelector: safeAreaInsets`. The getter is
  available since macOS 12, so a valid `NSScreen` on a supported system always
  implements it. Remove the selector check and update its safety comment.
- `screen_notch_shelves` checks `respondsToSelector: auxiliaryTopLeftArea`.
  Both auxiliary-area getters are available since macOS 12. Remove the selector
  check and revise the comment describing an unavailable API.

Keep their nil-screen handling and empty notch-area handling. Hidden/offscreen
windows and display removal can leave a window without a screen; displays
without a notch still return empty auxiliary areas. These are runtime and
hardware conditions, not old-OS compatibility.

Include these two fullscreen cleanups with the Quake pump replacement, as
approved on 2026-09-24. Verify them with the focused fullscreen/notch tests and
native fullscreen and Quake smokes.

## Vendored GPUI candidates

Do not expand the Quake change into a general vendor cleanup. If updating GPUI
later, review these paths under `third-party/vendor/gpui-0.2.2/src/platform/mac`:

| Location | Candidate | Disposition |
| --- | --- | --- |
| `window.rs`, blur setup | Pre-macOS-12 private WindowServer blur fallback | Unreachable with Huterm's new minimum. Remove only through the vendor workflow with native visual checks. |
| `platform.rs`, screen capture | macOS 12.3 availability guard | Redundant for Huterm if the optional feature is enabled; retain upstream code until a relevant vendor change. |
| `window.rs`, native tabbing | `addTabbedWindow:ordered:` selector check | API exists since macOS 10.12. Review together with the surrounding tabbing capability logic, rather than deleting checks indiscriminately. |

## Checks that must remain

- GPUI's macOS **15.3** fullscreen/titlebar guards still distinguish supported
  macOS 14 and 15.0–15.2 systems. Raising the minimum to 14 does not remove them.
- GPUI's `CVDisplayLink` release workaround addresses observed crashes. It is
  not a pre-14 availability path and is outside this cleanup.
- Preserve native object, class, view, layer, display identity, observer, and
  resource validation. API availability does not guarantee successful resource
  creation or correct object lifetime.
- Preserve Quake clock creation failure, invalid timing, display migration,
  stale-callback, cancellation, and no-screen handling. The X11 unknown-refresh
  fallback is unrelated to macOS compatibility.
- Preserve Sparkle loading checks and `minimum_macos = "10.13"` in
  `scripts/sparkle-source.json`. That value describes the pinned upstream
  artifact and is verified against its metadata; it is not Huterm's policy.

## Verification

For the minimum-version change, run package/release fixture checks, inspect the
generated application plist, and use `vtool -show-build` to verify each freshly
built arm64 and x86_64 executable's minimum. Verify both packaged slices too.
Cross-compilation does not replace native Intel UI coverage; record that limit
if an Intel host is unavailable.

Use focused static and documentation checks for this inventory. Any later
behavioral cleanup must include the native or visual checks named above.

## Unresolved questions

None block the approved minimum-version change. Vendor cleanup remains optional
and should be reconsidered against the GPUI revision in use at that time.
