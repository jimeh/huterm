# Native and non-native fullscreen

Status: proposed.

Keep published GPUI 0.2.2 for this delivery. Implement a small local AppKit
adapter modeled on upstream GPUI's simple-fullscreen API and implementation.
Replace its native mechanics with the upstream API when a published GPUI
release passes Huterm's compatibility and behavior checks.

Deliver [#40: native and non-native fullscreen][issue-40] as one
window-management PR. Preserve the existing fullscreen shortcut while adding
an explicit, serialized controller that [#41: quake terminals][issue-41] can
reuse without bringing global hotkeys or quake profiles into this delivery.

## Outcome and user contract

Huterm supports three window presentation states: windowed, native fullscreen,
and macOS non-native fullscreen. Native fullscreen uses the platform window
manager. On macOS it creates a separate Space. Non-native fullscreen removes
window chrome and fills the current display without creating a Space.

Users can invoke three window commands:

| Command | Behavior from a windowed state |
| --- | --- |
| `toggle_fullscreen` | Enter the configured default mode. |
| `toggle_native_fullscreen` | Enter native fullscreen. |
| `toggle_non_native_fullscreen` | Enter macOS non-native fullscreen. |

Every command exits the current fullscreen mode when the window is already
fullscreen. It does not switch directly between fullscreen modes. For example,
`toggle_native_fullscreen` exits a window that is currently in non-native
fullscreen.

The `cmd-enter` binding on macOS and `f11` on macOS and Linux invoke
`toggle_fullscreen`. The View menu keeps its single standard
Toggle Fullscreen item. The two explicit commands remain available to custom
keybindings, programmatic callers, and the future command palette.

On macOS, this setting selects the mode used by `toggle_fullscreen`:

```toml
[window]
macos_fullscreen_mode = "non_native" # or "native"
```

The macOS default is `non_native`. The green window button retains AppKit's
native fullscreen behavior. The setting is accepted on every supported platform
so one config file remains portable. Linux ignores
it and always uses native fullscreen for `toggle_fullscreen`, through GPUI's X11
backend. Huterm currently enables X11 only; enabling Wayland is outside #40.
An explicit `toggle_non_native_fullscreen` invocation
outside macOS returns an unavailable command error. If the Linux window manager
or compositor does not honor its native fullscreen protocol, the accepted
command times out with a window status error rather than claiming success.

A successful reload changes later default-toggle invocations. It does not
change a window's current mode or interrupt a transition already in progress.
An invalid value follows the existing atomic reload behavior and leaves the
working config and bindings intact. At startup it follows other non-engine
configuration errors: preserve a valid engine choice, start with default
settings, and surface the diagnostic. Spell the enum's Serde value explicitly
as `non_native`; the surrounding config enums' lowercase convention would
otherwise produce `nonnative`.

## Scope

This delivery includes:

- the three command catalog entries and existing menu and binding integration;
- config parsing, defaults, reload behavior, and user documentation;
- a window-owned state machine for native and non-native fullscreen;
- a confined AppKit adapter for macOS non-native fullscreen;
- correct window chrome, layout, mouse, focus, PTY sizing, and restore bounds;
- native macOS and Linux smoke coverage; and
- deterministic handling of repeated commands, failures, window close, and
  display changes.

Quake profiles, OS-global hotkeys, show and hide animation, opacity, blur,
command palette UI, durable restoration, and GPUI changes are out of scope.
The controller must make quake reuse cheap, but it does not grow profile or
hotkey concepts for that future caller.

## Current implementation and constraints

`WorkspaceView::run_command` currently handles `toggle_fullscreen` by calling
`gpui::Window::toggle_fullscreen()` and immediately returning `Completed`.
GPUI 0.2.2 exposes only `toggle_fullscreen()` and `is_fullscreen()`. Its macOS
implementation calls `NSWindow.toggleFullScreen:`, while its X11 implementation
requests `_NET_WM_STATE_FULLSCREEN`. GPUI has no public operation for changing a
window's origin or macOS style mask, so this published version cannot implement
borderless macOS fullscreen on its own.

Upstream added `toggle_simple_fullscreen()` and `is_simple_fullscreen()` in
[PR #60020][gpui-simple-pr], followed by restore-bounds and cross-platform fixes
in [PR #62819][gpui-simple-fix]. Use the [GPUI API][gpui-window] and
[macOS backend][gpui-macos-window] at the fixed revision linked below as the
primary implementation reference. Zed application code is only a behavioral
reference; do not copy GPL-covered application code into Huterm.

Adopting current upstream GPUI would also migrate platform construction and
other framework APIs. The selected path avoids coupling #40 to that migration
and preserves the repository's dependency policy. This is a local adapter,
not a vendored GPUI fork or a Cargo patch.

The desktop already has the right containment pattern for the missing native
operation. `native_quit.rs` obtains the exact `NSView` through
`raw-window-handle`, confines Objective-C calls to a module that permits unsafe
code, and exposes safe Rust methods to the rest of the client. Fullscreen should
follow that pattern in a separate `native_fullscreen.rs` module rather than add
more unrelated responsibilities to `native_quit.rs`.

Several existing call sites treat GPUI's native flag as the complete
fullscreen state:

- `WorkspaceView::key_context` publishes `fullscreen` from
  `window.is_fullscreen()`;
- `terminal_top` removes the transparent macOS titlebar inset only when
  `window.is_fullscreen()` is true;
- the window refresh pump copies `window.window_bounds()` into the view every
  16 ms; and
- `finish_close` reads `window.window_bounds()` again immediately before Quit
  captures the in-memory `WindowRestore` record.

GPUI reports false during Huterm-managed non-native fullscreen and reports the
full-display frame as ordinary windowed bounds. The controller must become the
single owner of the logical mode and the last restorable windowed bounds. The
renderer and lifecycle code should ask it for those facts instead of inferring
them from geometry.

Native fullscreen is asynchronous on macOS. The public GPUI API provides no
completion callback. Its `is_fullscreen()` result is only the AppKit style-mask
bit, which changes before a Space animation has finished and therefore cannot
release the next queued operation. The existing GPUI window delegate already
owns the fullscreen and screen-change selectors, so do not replace it. Observe
window-specific AppKit notifications instead. Keep terminal input and
structural commands usable while a fullscreen transition is pending.

## Design

### Controller and transition rules

Add a private `fullscreen.rs` module in `huterm-gpui`. `WorkspaceView` owns one
`FullscreenController` for its native window. Core, protocol snapshots, and
terminal engines do not gain presentation state.

The controller stores:

- the stable observed mode, `Windowed`, `Native`, or `NonNative`;
- the current chrome/layout fact, which may change before a transition is stable;
- the desired mode after all accepted commands;
- at most one platform transition with its expected result and deadline;
- a monotonically increasing operation generation and a closing gate;
- the last restorable windowed `WindowBounds`; and
- macOS non-native saved state while that mode is active or exiting.

The pure state machine accepts a `ToggleIntent` of `Default`, `Native`, or
`NonNative`, plus observations from the platform. It emits a small effect such
as `ToggleNative`, `EnterNonNative`, or `ExitNonNative`. `WorkspaceView`
starts effect execution, but no AppKit mutation runs inside a GPUI `App`,
`Window`, or entity update borrow. Follow the existing composition-cleanup
pattern: capture a retained, main-thread-only native operation while the window
is borrowed, leave the update, execute it from a detached GPUI task, and enqueue
its result for the refresh pump to feed back into the controller. AppKit frame
changes can synchronously invoke GPUI's resize callback, so this separation is a
correctness requirement rather than scheduling polish.

On macOS, a window-scoped native observer writes
[will-enter, did-enter, will-exit, did-exit][apple-window-notifications], and
did-change-screen events into a small queue without re-entering GPUI. The
refresh pump drains those events before handling further effects. This adopts
native fullscreen entered or exited through the green button or another OS
action and marks the controller's external transition as pending.
On Linux, where the enabled GPUI backend delegates to the X11 window manager,
the pump observes `window.is_fullscreen()`. A stable non-native state comes from
the owned AppKit saved-state handle, never from frame equality.

Each command toggles the desired state rather than the last observed state:

```text
desired windowed + toggle(mode)  -> desired mode
desired fullscreen + any toggle -> desired windowed
```

This gives rapid commands deterministic parity. If two toggles arrive while a
native entry animation runs, the controller finishes that entry and then exits.
If a third toggle requests non-native mode, it exits native mode before entering
non-native mode. Only one platform operation runs at a time.

Every native and non-native entry or exit receives a five-second deadline from
an injectable clock. On macOS, the matching did-enter notification completes
native entry. Native exit completes after did-exit and a foreground frame
reconciliation through AppKit's `constrainFrameRect:toScreen:`. This prevents
a queued non-native entry from saving an offscreen native-exit frame that
AppKit would later constrain. The reconciliation is generation checked and
leaves existing non-native saved recovery state to its own exit sequence. The
style-mask bit is only a consistency check. On Linux, reaching the expected
GPUI state completes it. An immediate platform error or deadline expiry resets
the desired mode to the latest recoverable mode and reports one window status
error. A later OS state change after a timeout is adopted as an external
change rather than replaying the failed request. If AppKit has not delivered a
completion notification, reject native dispatch and non-native entry before
acquiring presentation state. Timeout alone does not prove the native animation
stopped. Recovery must not require a style mutation during that unresolved
transition.

If a native will-enter event arrives while non-native saved state exists, that
saved state and its presentation leases take precedence and must never be
discarded. Mark recovery desired, wait for the external native entry to finish,
exit native fullscreen, then roll back non-native state to the saved windowed
geometry. Do not overlap AppKit operations in an attempt to cancel the entry.

Commands that start or retarget a transition return `Accepted`. Unsupported
requests and immediate adapter failures return a synchronous command error.
Later failures use the existing window status path. Fullscreen transitions do
not set `WorkspaceView::busy`, since that would block unrelated input and tab
operations.

The `fullscreen` key context reflects the observed presentation state, not the
desired state. The transition itself cannot make conditions claim fullscreen
before the OS has changed the window.

Layout has a narrower question than the key context: whether native chrome is
currently consuming the titlebar inset. On macOS, update that fact from the
will/did lifecycle plus GPUI's style-mask observation, and update it at the
style-change phase of a non-native operation. This keeps layout aligned during
animation without pretending the transition is complete. The stable mode still
changes only on the completion signal.

### Native fullscreen

Continue to use `gpui::Window::toggle_fullscreen()` for native entry and exit
on macOS and Linux. Do not reproduce GPUI's platform behavior in Huterm.

The refresh pump samples `window.is_fullscreen()` and advances the controller.
On macOS the window-scoped AppKit did notifications are authoritative for
completion; the pump uses GPUI's flag and bounds only for consistency and
windowed restore state. On Linux the window manager or compositor remains
authoritative through GPUI's observed state. Rendering continues to re-derive
viewport size and scale through existing Huterm paths; GPUI exposes no public
bounds or scale observer for this controller.

After a macOS did-enter or did-exit event, schedule any next native effect no
earlier than the following main-loop turn. AppKit can still reject a second
`toggleFullScreen:` in the notification's own turn. Missing did notifications
and native failures fall back to the deadline and status path; do not replace or
swizzle GPUI's existing window delegate to receive failure callbacks.

When a native transition starts from windowed state, retain the last windowed
bounds before calling GPUI. When native fullscreen is stable, GPUI's
`WindowBounds::Fullscreen` restore value may refresh that record if it is
valid and no non-native saved state exists. In particular, external native entry
over non-native mode captures the screen-sized borderless frame as GPUI's native
restore value; it must never replace Huterm's earlier windowed record. Quit
captures the controller's restorable windowed bounds, not the animated or
fullscreen frame.

### macOS non-native fullscreen

Keep the adapter's basic operations recognizable as upstream
`toggle_simple_fullscreen()` and `is_simple_fullscreen()`. Huterm calls this
mode `NonNative`; upstream calls it `simple`. Keep that terminology mapping at
the adapter boundary. The controller chooses explicit enter/exit effects; the
adapter checks its owned state before performing a toggle so stale effects
cannot reverse an already satisfied request. Its state query distinguishes
completed entry from saved recovery state retained during entry or rollback.

The similarity is behavioral, not a promise of identical signatures. Huterm
needs queued completion/error events and explicit close cleanup around the
basic operations. Keep generation checks, native notification observation,
and command serialization separate from the replaceable simple-fullscreen
mechanics. Follow upstream for saved style/frame state, application presentation
reference counting, focus restoration, close cleanup, and preserved windowed
bounds. GPUI 0.2.2 cannot report Huterm's saved bounds itself, so the controller
continues to supply them to refresh and Quit capture.

Add `native_fullscreen.rs` under the existing macOS target. It uses the exact
GPUI `NSView` from `raw-window-handle`, resolves its owning `NSWindow`, checks
the AppKit main thread, and contains every Objective-C operation and retained
native object. Non-macOS builds expose a small adapter that reports the mode as
unsupported.

The adapter registers `NSNotificationCenter` observers scoped to that exact
`NSWindow` for native will/did fullscreen and screen-change notifications.
It also observes app-scoped
`NSApplicationDidChangeScreenParametersNotification` for display topology,
resolution, scale, and coordinate changes that do not move this window between
screens. Callbacks only enqueue plain events; they never call back into GPUI.
Retain the observer tokens on the main thread and remove them during accepted
close or unexpected view cleanup. This avoids both polling Objective-C every
16 ms and competing with GPUI's existing delegate methods.

Entering non-native fullscreen performs these operations:

1. Reject entry when the native view, window, or screen is unavailable.
2. Save the current content frame in AppKit screen coordinates, the screen's
   full and visible frames, style mask, screen identity, and first responder.
   Keep content/frame conversion explicit for windows without full-size content.
   Huterm's transparent titlebar uses full-size content; its PTY geometry change
   primarily follows the controller's chrome inset, not that conversion.
3. Acquire app-wide auto-hide leases for the Dock and menu bar. Compute and
   validate the complete presentation-option set before assigning it atomically.
   Auto-hiding the menu bar requires hiding or auto-hiding the Dock; never assign
   an intermediate invalid combination, including during restoration.
4. Remove the `titled` and `resizable` style-mask bits while preserving every
   unrelated bit. GPUI's window subclass allows key/main status even when fully
   borderless; preserving its other style choices is the reason for this mask.
5. Defer the frame change by one main-loop turn so AppKit has applied the style
   and presentation changes. Fill the current `NSScreen.frame`, bring the window
   forward, and restore the retained first responder.
6. Report completion only after the controller observes the saved-state handle
   and the expected screen-sized frame. Deliver that result through the event
   queue after all synchronous AppKit callbacks have returned.

Treat entry as a generation-tagged transaction. If close, display change, a
newer operation, an AppKit error, or the deadline intervenes before completion,
invalidate deferred callbacks and enter the shared restoration sequence for
every step that was applied. Keep ownership of saved state until style, frame,
responder, and leases are either restored or the native window is confirmed
gone. A rollback failure reports status and remains retryable instead of
clearing the only recovery record.

Here, adapter errors mean detectable failures such as a missing native object,
failed focus restoration, or unmet completion conditions. The pinned `objc`
dependency does not enable exception catching. Do not promise Rust rollback
from Objective-C exceptions raised by invalid setters; validate their inputs
before calling AppKit. Test presentation-option validity for acquisition,
release, and rollback through the same pure computation used by production.

The AppKit presentation options belong to the application, not one window. A
main-thread lease manager records whether Huterm added each option and reference
counts active non-native windows. Exiting or closing one window must not reveal
the Dock or menu bar while another non-native Huterm window still needs them.
When the final lease ends, restore the preexisting option bits without clearing
options that Huterm did not add.

Exiting reverses the operation:

1. Restore the exact style mask.
2. Release this window's menu and Dock leases. If it was the final owner, this
   starts restoring the application's ordinary presentation options; another
   non-native owner keeps those options in force.
3. Defer one main-loop turn, then generation-check again. AppKit does not update
   `NSScreen.visibleFrame` synchronously after presentation options change.
4. Resolve the restore content frame against the now-current display geometry,
   convert it back through
   `frameRectForContentRect:` and restore that frame.
5. Restore the retained first responder and make the window key.
6. Clear saved state only after all required restoration succeeds.

Run exit through the same outside-update executor and generation check as
entry. Entry rollback uses this same style, lease, deferred-geometry, and focus
sequence rather than maintaining a second ordering. A stale deferred callback
may release only resources owned by its own generation; it must not mutate a
reused or closing window.

The initial mode has no window animation. #41 owns quake show and hide animation
and can animate around the stable fullscreen controller later.

[Apple documents][apple-borderless] the general key/main-window caveat for fully
borderless windows, although GPUI's window subclass explicitly allows both.
Preserving GPUI's unrelated style bits and restoring the exact original mask is
the binding reason to remove only title and resize. That narrower operation also
matches [Ghostty's non-native fullscreen controller][ghostty-fullscreen]. Ghostty
is a behavior and failure-mode reference, not a source dependency.

This deliberately differs from upstream's whole-mask borderless replacement.
The deferred restoration sequence, display recovery, and transactional error
handling below also remain Huterm requirements. Use upstream as the primary
reference without dropping these reviewed safeguards or duplicating its entire
platform backend.

### Replacing the adapter with published GPUI

When a published GPUI release includes simple fullscreen and the follow-up
restore-bounds fix, evaluate it in a separate dependency upgrade. Compile on
macOS and Linux, audit dependencies, and run the existing input, menu, Quit,
renderer/scroll checks plus the fullscreen smokes below. A matching method name
alone is insufficient evidence of compatibility.

Once the upgrade preserves the user contract and passes those checks, route
the adapter's basic operations through GPUI and delete the local simple-mode
style/frame, focus, and presentation-lease machinery that GPUI now owns.
Keep the controller and any notification or recovery support still required
by demonstrated behavior gaps. Do not retain a second presentation lease
manager alongside GPUI's. Preserve the restore-bounds tests when replacing the
controller's local saved-frame source with GPUI's reported restore value.

Specifically verify preservation of unrelated style bits while fullscreen and
exact mask restoration afterward, display refitting and fallback geometry,
and interrupted-entry recovery. Upstream's saved-mask restoration does not by
itself prove preservation of those bits during fullscreen. If the published
implementation cannot meet a requirement, retain the corresponding local
mechanics; do not combine two owners of style/frame or presentation options.

### Display changes and unavailable displays

Record `NSScreenNumber` from the screen's device description on entry and treat
it as the display identity for this process. Keep the original restoration
anchor frozen, with a separate last-settled fullscreen display frame.

Coalesce exact-window screen-change and application screen-parameters
notifications into a deferred foreground turn, outside GPUI's update borrow.
Before completing entry, validate the display identity and frame; fail and roll
back on a real mismatch. Once entry completes, refit to the same display's
current full frame when its size or origin changes. A rearrangement can put the
old window frame temporarily on another screen, so check the target display's
geometry before interpreting the current screen as an intentional transfer.

If the target geometry is unchanged but the window moved to another display,
exit non-native mode. Also exit when the target disappears. Ignore unrelated
notifications. Scale and safe-area changes continue through the window's normal
layout refresh. Refitting preserves the original restoration bounds, shadow
state and presentation lease, and does not change focus or window ordering.
Guard queued work against exit, native transitions and close; resolve the latest
screen geometry when it runs. If AppKit cannot apply that geometry, recover to
windowed mode.

If the original display still exists and its full frame is unchanged, restore
the saved content frame exactly. If the display's origin or size changed,
translate the saved frame by the old-to-new display-origin delta, cap its size,
and clamp it to the current visible frame. If the display disappeared, choose
the window's current display or the primary display, cap the saved size to that
display's visible frame, and center it. Put this geometry choice in a pure
helper so display-removal and rearrangement tests do not require physical
monitor changes.

Native display changes remain AppKit or window-manager behavior. The controller
observes the resulting native state and refreshes the last valid windowed bounds
after exit.

### Layout, focus, and lifecycle integration

Replace direct checks of `window.is_fullscreen()` with the controller's current
chrome fact for layout and its stable observed mode for key context. Both modes
remove
the macOS titlebar inset. `ChromeLayout`, terminal painting, mouse mapping,
selection, scrollbars, and PTY resize continue to share the resulting terminal
bounds.

`TerminalView` cannot reach its owning `WorkspaceView`, so give it the copied
chrome/layout fact used by `content_bounds` and `terminal_top`. Whenever that
fact changes, push it to every retained tab, including hidden tabs, and
synchronize it again before tab activation. This is the same ownership shape as
the existing shared sidebar-width input: the window owns the policy, while each
retained terminal owns the render input it needs. Request a fresh layout and
PTY resize after the push.

While non-native fullscreen is active, the 16 ms refresh pump must not overwrite
the controller's windowed restore bounds with the screen-sized GPUI bounds. It
may still update current viewport geometry and scale. Once windowed, stable GPUI
bounds replace the cached restore value.

Config reload updates the controller's default mode without changing observed
or desired mode. The controller survives tab changes because it belongs to the
window, not a terminal view.

Close assessment and confirmation leave the current presentation untouched.
Immediately before an accepted window removal, close the operation gate,
increment its generation, unregister native observers, discard the non-native
saved state, and release its application presentation leases. Frame restoration
is not required for a window that is about to disappear, but app-wide
presentation state must be repaired for surviving windows. An unexpected view
release has a main-thread cleanup fallback with the same lease guarantee.
Application process termination needs no frame restoration, though test and
normal multi-window close paths must not rely on process exit for cleanup.

`finish_close` must stop assigning live `window.window_bounds()` directly to
the restore record. It asks the controller for its last restorable windowed
bounds, just like the refresh pump. Otherwise Quit replaces the correct saved
geometry with fullscreen geometry immediately before capture.

While the invoking window's own non-native mode or transition holds
presentation state,
`minimize` and `zoom` return unavailable rather than operating on an untitled,
non-resizable window or hiding the only visible lease holder. A newly opened
ordinary window does not acquire a lease: app-wide Dock and menu-bar auto-hide
remains in force while any non-native window owns one, and opening or closing
that ordinary window must not change the count.

The in-memory `WindowRestore` record continues to carry windowed bounds only.
It does not persist the active fullscreen mode. Durable mode and window
restoration remain part of #42.

## Implementation sequence

1. **Add commands, config, and the pure controller.** Define the two new window
   command IDs beside `toggle_fullscreen`, add the explicitly renamed macOS
   default-mode enum, and preserve existing bindings and the single View menu
   item. Add reducer tests for every stable-mode and toggle-intent combination,
   rapid command parity, external native changes, native/non-native conflicts,
   timeouts, late observations, operation generations, and rollback. Confirm
   that an explicit non-native command returns unavailable outside macOS.

2. **Implement and test the AppKit adapter.** Add safe Rust entry points around
   the raw `NSWindow`, saved content-frame and responder state, style changes,
   frame application, display identity, notification observation, and
   presentation leases. Execute mutations outside GPUI update borrows and test
   pure display fallback, lease accounting, generation cancellation, and
   transactional rollback independently. Keep all Objective-C calls and unsafe
   declarations in the adapter. Use the pinned `objc` and `raw-window-handle`
   dependencies unless a verified API gap requires a dependency change.
   Model the basic operations on the pinned upstream simple-fullscreen
   reference, document the intentional differences above, and keep the native
   mechanics separable for replacement by a published GPUI API.

3. **Integrate the controller with `WorkspaceView`.** Route all three commands,
   drain adapter events from the refresh pump, and report asynchronous failures
   through window status. Replace native-only fullscreen checks in key context
   and titlebar layout, and synchronize the presentation fact into every
   retained `TerminalView`. Protect windowed restore bounds during non-native
   mode and replace `finish_close`'s live-bounds overwrite before Quit capture.
   Add focused pure layout, restore-selection, and command-availability tests;
   reserve entity and window assertions for the production smokes.

4. **Close lifecycle and reload gaps.** Release non-native presentation leases
   before accepted close, invalidate queued effects, unregister observers, cover
   unexpected view release, and update existing windows' configured default on
   reload without changing active modes. Define minimize and zoom availability.
   Verify simultaneous non-native windows, an ordinary window opened while a
   lease exists, canceled and accepted close, and Quit capture while fullscreen.

5. **Add native fullscreen smokes.** Extend the production desktop test entry
   points rather than creating a second fullscreen implementation. Add
   `smoke:macos-fullscreen` and `smoke:linux-fullscreen` Mise tasks plus focused
   checker scripts. Install and launch Openbox as the Linux smoke's
   EWMH-capable window manager and provision `xprop` through `x11-utils`; keep
   existing bare-Xvfb and `twm` jobs unchanged.
   Route native builds through `build:exec`, run script checks, document the
   Linux capability requirement, and place the new smokes in their platform CI
   jobs.

6. **Document, verify, and review the candidate.** Update the starter config,
   README command and key-context tables, development guide, and any new native
   smoke prerequisites. Run the checks below, inspect the final diff, and record
   real native and manual limitations in the PR. Do not start #41 in the same
   branch.

## Verification and acceptance

Use focused tests during implementation. Confirm new tests are collected and
fail at their intended assertion before the corresponding behavior lands when
practical.

### Pure and platform-independent tests

- Cover all nine observed-mode and intent combinations. Verify every fullscreen
  toggle exits an active mode, including native intent from non-native mode.
- Cover two and three rapid toggles, different-mode toggles during a native
  animation, immediate adapter failure, timeout, late native completion, and an
  external green-button state change. Model macOS will/did events separately so
  a style-mask change cannot complete the transition.
- Cover an external native entry while non-native saved state exists, close and
  display change before a deferred entry runs, failure after each applied entry
  step, stale-generation callbacks, and retryable rollback failure.
- Verify config omission, both accepted values, invalid reload retention, Linux
  default behavior, nonfatal startup fallback, exact `non_native` spelling, and
  explicit non-native unavailability.
- Verify menu and default binding behavior remains unchanged while all three
  commands validate and route through the catalog.
- Extract the fullscreen key-context decision as a pure function of stable
  observed mode and test all modes plus pending entry/exit. In the macOS smoke,
  exercise a `when = "fullscreen"` binding in non-native mode and verify it
  stops matching after exit, proving the real context uses that decision.
- Feed the copied chrome fact into the existing pure layout helpers and verify
  titlebar inset, tab placement, mouse coordinates, selection, scrollbar, and
  PTY grid bounds. Drive will and did separately and assert chrome changes at
  the platform style phase while stable mode waits for completion.
- Keep restorable-bounds selection on the pure controller seam used by
  `finish_close`. Verify native and non-native states, a saved display that
  disappears, and the external-native-over-non-native case that must reject
  GPUI's screen-sized restore value.
- Test screen-parameter revalidation with the same display and frame, a changed
  origin and reduced size on the same identity, a changed current identity, and
  a saved identity no longer present. Assert translation, clamping, and centering
  in AppKit's global coordinate space.
- Keep presentation lease accounting platform-free. Verify two owners plus
  close, drop, adapter failure, rollback retry, ordinary-window churn, and final
  exit release each owned lease exactly once.
- Put minimize and zoom gating in a pure command-availability helper and verify
  it refuses both while that window owns non-native presentation state. Verify
  an ordinary second window allows both while the first window holds leases.

These tests stop at pure inputs and emitted effects. `huterm-gpui` has no GPUI
test-support dependency or live `App`/`Window` unit harness, so retained entity
synchronization, real command routing, close capture, and AppKit effects belong
in the production desktop smokes below rather than simulated unit tests.

### macOS native smoke

Drive a real production `WorkspaceView` and PTY through programmatic commands:

- Native mode acquires AppKit's fullscreen style and later returns to the
  original content size and focused terminal. Hosted macOS runners may choose a
  different on-screen origin after leaving the native Space, so CI verifies the
  settled frame and physical macOS validation verifies exact position.
- Non-native mode never acquires AppKit's fullscreen style, removes visible
  chrome, fills the current screen, keeps terminal input working, and restores
  style, content bounds, first responder, and PTY size.
- Enter with two tabs while one is hidden, activate the retained tab after the
  mode change, and verify its layout, mouse mapping, and PTY bounds use the same
  chrome fact.
- Rapid toggles converge to the requested final state without overlapping
  AppKit operations; the second operation starts only after the matching did
  notification and a later main-loop turn.
- Two windows retain menu and Dock auto-hide until the final non-native window
  exits or closes. Opening and closing an ordinary third window does not change
  the lease count.
- Minimize and zoom commands report unavailable during non-native presentation
  without changing style, frame, or leases.
- Quit while either mode is active captures the original windowed bounds through
  the real `finish_close` path rather than the live fullscreen frame.
- Switching the configured default by reload changes the next invocation only.

Run the smoke with both terminal engines only where it asserts PTY input and
resize behavior. Fullscreen state itself needs one engine because the engine
does not own the window.

CI has one virtual display. Record manual macOS evidence for two physical
displays, entering on each display, disconnecting the active display, native
Space entry and exit, and repeated toggles. Record whether the run used Stage
Manager and whether Displays Have Separate Spaces is enabled. Check Dock and
menu-bar behavior both on the display that owns the menu bar and on the
secondary display. Synthetic CI evidence does not replace these checks.

### Linux native smoke

Use Xvfb with Openbox. The repository's existing `twm` and bare-Xvfb jobs do not
provide the EWMH behavior this smoke needs, so leave them focused on their
current coverage and add Openbox to the Linux CI packages and development
prerequisites for this task. Add `x11-utils` for `xprop`, which the checker uses
to read `_NET_WM_STATE_FULLSCREEN`; geometry alone is insufficient evidence.

Invoke `toggle_fullscreen` through the production binding, observe the actual
fullscreen window property and screen-sized geometry, verify terminal bounds
and input, then exit and compare restored geometry. Linux has no non-native mode
in this delivery. Add a focused no-EWMH checker case proving the deadline emits
one status error and clears the pending request. The automated native smoke is
X11-only, matching Huterm's enabled GPUI backend. Wayland support and its
verification belong to a separate platform change.

### Repository checks

- Run focused `huterm-protocol` and `huterm-gpui` tests while iterating.
- Run `mise run check:scripts` for checker or task changes.
- Run `mise run smoke:macos-fullscreen` and the existing macOS menu and input
  smokes when on macOS.
- Run `mise run smoke:linux-fullscreen` and `mise run smoke:linux-input` on
  Linux.
- Run `mise run verify` before handoff.
- Run `mise run license` if dependencies change.

Success means all three commands obey the same mode-independent exit rule,
native transitions never overlap, non-native fullscreen remains in the current
macOS Space, both modes restore usable focus and PTY geometry, retained tabs use
the same presentation fact, and unsupported or ignored platform requests fail
clearly.

## Implementation evidence

Local Linux validation on 2026-09-08 passed:

- Clippy for all GPUI targets with locked dependencies and warnings denied,
  through `mise run build:exec`;
- `mise run build:exec -- cargo test -p huterm-protocol -p huterm-gpui --lib`,
  with 189 GPUI tests and 7 protocol tests;
- `mise run check:scripts`, including 48 script tests and TypeScript checking;
- `mise run smoke:linux-fullscreen`, with both engines, actual EWMH fullscreen
  and restored root geometry, PTY input and resize, unavailable non-native
  commands, ignored-EWMH timeout, and assessed Quit capture; and
- `mise run smoke:linux-input`, with exact input bytes for both engines.

`mise run verify` also passed, covering workspace tests, formatting, Clippy,
documentation, architecture, dependency policy, and workflow checks.

The host lacked Openbox and X11 inspection/input tools. Sudo required a password,
so these checks used Ubuntu packages extracted into a temporary tools directory,
without installing system packages. CI installs the same prerequisites through
apt.

GPUI 0.2.2 stores raw X11 ConfigureNotify origins. Under Openbox, its cached
windowed origin changed from root-relative to parent-relative after exiting
fullscreen, while `xdotool` proved exact physical restoration. The Linux smoke
therefore compares root geometry through X11 and keeps GPUI size, grid, and
fullscreen-cache checks strict. Windowed and Quit metadata may retain that
parent-relative origin. Correcting GPUI's coordinate reporting is outside this
delivery; durable restoration remains deferred.

The Linux host could not compile or run AppKit. macOS CI must validate the
adapter and native smoke; local reducer, operation-gate, display-recovery, and
lease tests do not substitute for native execution. Physical display
disconnects, Stage Manager, Separate Spaces, and Dock/menu-bar behavior on
multiple displays remain manual checks. Objective-C exceptions are outside the
pinned binding's error handling, as described in the contract above.

### macOS restoration diagnosis

Diagnostic revision `9caab27` reproduced the macOS 14 CI failure after rapid
native entry/exit followed by non-native entry. The adapter saved AppKit frame
`556,832,808,584` on a `1920x1080` display. Exit requested that exact frame, but
AppKit constrained it to `556,471,808,584`; it remained there in a later sample.
The first non-native exit in that run restored its valid saved frame exactly.
This evidence ruled out an immediate comparison alone as the failure's cause.

Native exit now reconciles AppKit's constrained window frame before releasing
queued operations. Non-native restoration remains exact and retains recovery
ownership on failure. The smoke's final windowed-size check permits the same
native Space origin changes as its earlier native restoration check; its
non-native origin checks and exact Quit capture comparison remain unchanged.
The smoke exercises the shared AppKit constraint/setter with an offscreen frame
on newer hosts too. Actual native notification ordering remains covered by the
full native transition sequence and macOS 14 CI.

Local macOS 27 arm64 validation of the correction passed 29 focused fullscreen
Rust tests, 54 script tests, `check`, `test`, and `verify`. The final fullscreen
runner passed three consecutive times with both engines and isolated frame
probes. Menu and Quit smokes passed. The input smoke passed on an unchanged retry
after one event-7 timeout. The native-frame probe failed at its reconciliation
check when frame adjustment was temporarily disabled, then passed after restore.
The macOS 14 CI result remains the validation of the original native sequence;
physical display and window-manager coverage limits above still apply.

Non-native fullscreen disables the saved window's shadow to remove AppKit's
thin perimeter outline, then restores the original shadow setting with the
window style on exit or rollback. It still uses the exact display frame.
The current display's `safeAreaInsets` reserve an unobscured content area on
notched displays. The root paints the reserved area with the theme background;
tabs and terminals share the inset layout for rendering, input, and PTY sizing.
Native fullscreen receives no additional safe-area inset because AppKit already
positions its content. Automated smokes compare the reported display insets,
retained layout, shadow restoration, and PTY dimensions.

## Alternatives considered

**Upgrade to upstream GPUI now.** Upstream already has simple fullscreen, but
published 0.2.2 lacks it. A source revision also brings platform and application
API migration and requires revisiting the no-Git-dependency policy. Treat that
as a separate framework upgrade rather than a prerequisite for #40.

**Backport into a GPUI fork.** Patching 0.2.2 could expose upstream's method
names directly, but would make Huterm maintain a framework patch and its
packaging. The existing raw-window adapter pattern provides the required
access without changing GPUI. Keep the local implementation small and remove
its native mechanics under the published-release replacement criteria above.

**Branch directly in `WorkspaceView`.** Calling GPUI or AppKit directly from
each command would be shorter initially. It would spread desired state,
transition serialization, restore bounds, native cleanup, and quake-facing
behavior across command, layout, refresh, and close paths. A controller keeps
those invariants in one place and gives #41 one reusable interface.

**Combine #40 and #41.** Quake adds profile identity, global shortcut
registration, hidden-window lifecycle, focus return to another application,
placement, animation, and repeated window reuse. Those failure modes are large
enough for their own PR. Issue #40 should finish with a quake-ready controller,
then issue #41 can remain a caller-focused delivery.

**Use a fully borderless AppKit style mask.** Apple documents that such windows
may not become key or main, though GPUI's subclass opts into both. More
importantly, replacing the mask would discard unrelated GPUI style choices.
Removing only `titled` and `resizable` follows a proven terminal implementation
and permits exact restoration of the saved mask on exit.

## Decisions

- Keep published GPUI 0.2.2 and model the local adapter on upstream simple
  fullscreen; do not fork, patch, or upgrade GPUI for #40.
- Replace local simple-mode mechanics when a published GPUI upgrade preserves
  the contract and passes the specified compatibility and runtime checks.
- Non-native fullscreen is the macOS default; Linux uses native fullscreen.
- The explicit non-native command is macOS-only; the config remains portable.
- Any fullscreen toggle exits whichever mode is active.
- Rapid toggles update desired state and never overlap platform operations.
- macOS native completion follows exact-window did notifications and native-exit
  frame reconciliation, not the early style-mask change; later operations wait
  one more main-loop turn.
- Non-native fullscreen auto-hides the menu bar and Dock and fills
  `NSScreen.frame`.
- A non-native window refits when its display changes size or origin. Transfer
  to another display or removal of its target exits fullscreen.
- Native state can never erase live non-native recovery state or its leases.
- AppKit mutations run outside GPUI update borrows and every deferred operation
  is generation checked.
- Minimize and zoom are unavailable while non-native presentation state exists.
- Linux reports a timed-out native request instead of treating an ignored
  window-manager or compositor request as success; automated coverage is X11.
- The View menu keeps one configured-default Toggle Fullscreen item.
- Fullscreen mode is window presentation state and stays out of core and
  protocol snapshots.
- This PR does not persist active fullscreen mode or animate quake windows.

No product decisions remain open. Implementation may adjust private type names
or the smoke's EWMH window manager when current platform evidence justifies it,
without changing the user contract above.

[apple-borderless]: https://developer.apple.com/documentation/appkit/nswindow/stylemask-swift.struct/borderless
[apple-window-notifications]: https://developer.apple.com/documentation/appkit/nswindow#Notifications
[ghostty-fullscreen]: https://github.com/ghostty-org/ghostty/blob/82232ecde55405559dec29c5466cb9e39938cb41/macos/Sources/Helpers/Fullscreen.swift
[gpui-simple-pr]: https://github.com/zed-industries/zed/pull/60020
[gpui-simple-fix]: https://github.com/zed-industries/zed/pull/62819
[gpui-window]: https://github.com/zed-industries/zed/blob/242fe31a399f695103dd0cfe7dcef33a59fbf596/crates/gpui/src/window.rs
[gpui-macos-window]: https://github.com/zed-industries/zed/blob/242fe31a399f695103dd0cfe7dcef33a59fbf596/crates/gpui_macos/src/window.rs
[issue-40]: https://github.com/jimeh/huterm/issues/40
[issue-41]: https://github.com/jimeh/huterm/issues/41
