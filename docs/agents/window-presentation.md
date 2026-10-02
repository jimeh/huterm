# Window presentation

Rules for fullscreen, Quake windows, the title row, window decorations, and
native window handles on macOS and X11. AppKit and X11 effects in this area
often reenter GPUI or arrive late, so most rules here are about ordering.

## Fullscreen

GPUI's X11 ConfigureNotify handler stores raw event origins, including
parent-relative coordinates after a reparenting window manager restores a
window. Fullscreen smokes must compare actual root geometry through xdotool;
GPUI's windowed and Quit metadata can retain the parent-relative origin even
when physical restoration is exact. Keep size and fullscreen-cache checks
strict. Do not interpret Linux WindowBounds::Windowed as absence of fullscreen;
the X11 backend reports it even while its fullscreen flag is true.

Fullscreen native observers own a separate event queue and operation gate.
Invalidate deferred non-native work in the notification callback itself, then
signal the window-owned coalesced wake to reconcile mode on a later turn.
Fullscreen scheduling uses events and one earliest-deadline timer; only macOS
windows created without a native adapter retain fallback polling. Keep the
Quake and ordinary fullscreen state owners separate, sharing only coalesced wake
and earliest-deadline mechanics. Each Quake presentation owns its observers and
pending work; animations use the shared workspace frame registration and an
active-only target-display clock or refresh-derived fallback. Hidden healthy
Quake owners have no periodic work; macOS visible non-fullscreen profiles retain
a one-second work-area safety sample for external Dock changes. AppKit setters
and presentation cleanup,
including unexpected view release, run outside GPUI update borrows.

Observe queued native notifications and the current fullscreen flag before
accepting a toggle; dispatch effects only after that intent is accepted. Native
fullscreen must remain available when the non-native adapter is unavailable.
For display recovery, expand saved content to its restored titled frame before
clamping to the visible display, or AppKit constrains the titlebar a second time.

Fullscreen completion can precede the final frame and grid publication on macOS
and X11. Smokes must wait for settled geometry and compare PTY dimensions with
the grid published in the same snapshot. Hosted macOS native Spaces may choose
a new on-screen origin; keep non-native and X11 geometry exact, and verify exact
native placement on physical displays. A late native screen-change notification
can arrive during the next non-native entry. Record it without canceling from
the callback; main-thread display identity and frame validation distinguishes
notification noise from a real display change.

AppKit 14 can deliver native did-exit with an offscreen window frame. A later
`titled` frame setter constrains that frame, so saving it for non-native entry
makes exact restoration impossible. Reconcile native exit through AppKit's
`constrainFrameRect:toScreen:` on a generation-checked foreground turn before
publishing DidExit. Skip this adjustment when non-native recovery already owns
saved state. Never clamp a non-native saved target to disguise a failed restore.
Native notification freshness must survive operation timeout while still
rejecting newer native events and close; keep its epoch separate from the
mutation generation.

AppKit's window shadow includes a thin outline in non-native fullscreen. Save
and disable `hasShadow` on entry, and restore it with the saved style before
validating exit. Keep the frame equal to the display rather than oversizing it.
For custom fullscreen, read the current NSScreen's `safeAreaInsets`; native
Spaces already handle this. Carry those logical-point insets through ChromeLayout
and every retained TerminalView. Keep safe-area padding separate from the titlebar
inset, which also controls titlebar rendering. AppKit NSEdgeInsets field order is
top/left/bottom/right, unlike GPUI's top/right/bottom/left.

Keep the original non-native fullscreen restoration display frozen and track the
last fitted display separately. Screen rearrangement can temporarily put the
old window frame on another screen; compare the target display geometry before
treating that as a transfer. Refit outside GPUI updates without changing focus,
window ordering, shadow state or presentation leases.

A native transition timeout cancels requested work without proving AppKit stopped
animating. Reject native dispatch and non-native entry while the adapter's native
transition remains unresolved, before saving state or acquiring a lease. Keep
this guard at effect dispatch so normal pending toggles can still change intent.
A late native completion still reconciles normally; an unavailable adapter must
not block explicit native fullscreen.

GPUI's macOS window-hover flag reports activation and can retain the last mouse
position after exit. Gate fullscreen tab reveal with the current AppKit pointer's
display membership so leaving for another display dismisses the overlay. Keep a
slow pointer deadline only while revealed or while the native fullscreen top
edge can receive pointer entry outside the content view. Capture pointer events
before terminal handlers and reconcile after gesture ownership changes.

## Native window handles

GPUI's inherent `Window::window_handle` returns its own handle; qualify
`HasWindowHandle::window_handle(window)` when obtaining the raw AppKit handle.
Map its `HandleError` explicitly into anyhow; it does not implement
`std::error::Error` with the current dependency features.

Guard nil retained handles before invoking `objc` message macros, including in
retained-handle destructors. Adapter drop moves native resources, such as an
AppKit owner, into deferred cleanup and leaves nil placeholders behind. The Rust
message dispatch path dereferences its receiver before Objective-C receives it,
even though Objective-C permits nil.

## Quake windows

Quake native effects run after returning from GPUI's App/window update borrow;
AppKit frame, style, and visibility changes can synchronously reenter GPUI.
Keep a generation check before every effect, and wait for native Space exit
before restoring style or applying quake geometry. Quake fullscreen shares the
existing application presentation lease pool. Hide only the associated window,
never the whole application. GPUI needs the tracked hidden-window vendor
patch: `show: false` otherwise maps on X11. The X11-handle patch reports a
destroyed window as unavailable. Run the quake native smoke to verify both.

Openbox may place a remapped window after its unmapped geometry was configured.
Quake must observe mapping and reassert the final EWMH frame until it settles,
including `animation = "none"` and zero-duration transitions.

GPUI 0.2.2's X11 `drop_window` stopped the platform loop on the last native
window, bypassing Huterm's zero-window keepalive. Upstream moved that policy
into `QuitMode`, whose default still quits after the last Linux window; the
application sets `QuitMode::Explicit` so Huterm's close/quit coordinator owns
that lifetime. Pair zero-window global
summon coverage with ordinary final-window exit coverage when upgrading GPUI.

Quake profile definitions and portable validation live in `huterm-config`;
keep desktop geometry and animation behavior in `huterm-gpui`. The global
keybinding schema restricts the shared command catalog to quake commands.
Extend shared schema fixtures and the desktop global compiler parity check
together; native key support and OS registration remain runtime checks.

Global shortcut keepalive requires a dispatchable binding whose native grab is
still owned. Grabs left by failed rollback remain tracked for cleanup but must
not keep a zero-window application alive by themselves.

Restore the retained GPUI NSView as AppKit firstResponder after quake style
changes and activation. setStyleMask can replace it while GPUI focus remains
true: raw Return still reaches the shell, but printable text is lost. Release
the retained view with the window in deferred native cleanup.

AppKit returns no NSScreen for a fully offscreen window during quake slides.
GPUI display-link and maximization getters must handle nil; screen and occlusion
callbacks restart the display link when the window returns.

Reassert macOS quake geometry during settling after changing presentation options.
AppKit can move the frame after its initial write. Keep X11 fullscreen geometry
under the window manager and retain the native-transition gate.

Refresh the selected display work area before and while settling quake geometry.
Fullscreen presentation hides the menu bar and dock, so cached visibleFrame can
become wrong when its lease is released. Update active targets without restarting
progress or renewing the transition deadline.

Carbon non-exclusive RegisterEventHotKey can succeed while delivering no events
because another application owns the shortcut. Keep the global-hotkey patch using
kEventHotKeyExclusive so startup/reload conflicts fail and cannot create a falsely
usable keepalive registration.

Keep quake titlebar visibility separate from fullscreen tab policy: partial quake
windows are frameless but retain the configured tab bar. Resolve tab presentation
from actual quake fullscreen state, share layout synchronization with ordinary
windows, and suppress overlay reveal while the quake window is hidden.

Quake work-area refits must preserve focus; only explicit summons request it.
While initial activation has never been observed, SettleVisible may retry Show on
a bounded cadence within the original deadline. Once observed, later app switches
must never re-arm retries. Activation samples taken while a native fullscreen
transition is unresolved, or while the observed fullscreen state differs from
the target, are ignored: a still-active window must not cancel its Show as a
Space exit begins, and AppKit can revert an activation granted before the
Space switch ends. Hosted runners hit both after a fullscreen-to-windowed
reload. A failed return-focus attempt must not undo a successful
native hide.

AppKit clamps intermediate top-edge frames even for borderless windows. Quake
opts only its own native window into unconstrained frames, retaining that flag
while hidden and restoring default constraints on regular conversion and cleanup.

Read backing scale from NSWindow while offscreen: NSScreen can be nil, and a
synthetic 2x fallback can resize Metal's drawable at a different scale from GPUI
after an on-screen move. Keep retained half/quarter/half resize coverage tied to
native view and drawable dimensions, GPUI viewport/scale, and PTY rows/columns.

RandR can return monitors with no primary flag. Missing retained monitors and
regular-to-quake conversion must fall back to the first remaining monitor.

For withdrawn X11 windows, merge ABOVE/STICKY into `_NET_WM_STATE` before mapping;
window managers ignore state client messages until they manage the window.

Quake auto-hide keys off application deactivation or key status moving to
another Huterm window, never key loss alone. Non-activating panels from other
processes, such as 1Password Quick Access, take key status while Huterm stays
active and `NSApp.keyWindow` is nil or still names the quake window. The macOS
witness `panel` command covers this in `smoke:macos-quake`; wait for
`focus_observations` to advance after the observed key change before closing
the panel, or the step can pass before the hide rule ever sampled the loss.

The `ordinary activate_window` smoke command moves key status to the sibling
Huterm window, which must still hide the quake. The external grab probe walks
`GRAB_CANDIDATES`, owns the first free chord as `chord=`, verifies and releases
a second as `free=` for Huterm's own registration case, or writes `failed` with
every candidate's error and quits. Nothing in that probe may panic inside
GPUI's launch callback, which cannot unwind and aborts with a crash dialog.

Unchanged visible quake summons only activate the window. Preserve fullscreen
leases and native state; re-enter the transition path for changed profile or
target geometry. Fullscreen geometry ignores work-area-only changes.

## Title row and window decorations

`tabs.position = "titlebar"` resolves through
`tab_position::resolve_tab_position`: `top` in fullscreen, Quake windows, and
Linux windows without granted client-side decorations. Only drawing code
matches `Titlebar`. GPUI's `WindowControlArea` hit testing does nothing on
macOS or X11. Huterm owns macOS title-bar drags: windows set
`app_owns_titlebar_drag`, and `title_row_gestures` moves them through
`start_window_move` on the first drag motion and runs `titlebar_double_click`.
Otherwise the window server takes a press on a tab in the title strip as a
window drag, and AppKit and Huterm could both act on a double-click. GPUI
supplies both pieces upstream (zed#41839, zed#60620). AppKit's traffic-light
size varies (12 points under GPUI 0.2.2, 14 points now), so a merged row reads
the native close button once the window exists and centres the buttons in the
strip at AppKit's left inset through `set_traffic_light_position`; a fixed
offset drifts when the size changes. GPUI keeps repositioning them for that
window, including after regular-to-Quake conversion; it skips a borderless
window, which has no buttons, and Quake windows never request a position.
`smoke:macos-palette` asserts the centring from the `traffic_lights` state.

X11 moves also need `start_window_move`. GPUI grants
X11 client-side decorations only when, at client start, the window manager
lists `_GTK_FRAME_EXTENTS` in the root
`_NET_SUPPORTED` (Mutter and KWin do, Openbox does not); its compositor probe
accepts any EWMH window manager. It silently falls back to server decorations
otherwise, so read `window.window_decorations()` instead of trusting the
request; decoration changes arrive through the window appearance callback.
The client-frame smoke appends `_GTK_FRAME_EXTENTS` to Openbox's
`_NET_SUPPORTED` under `xcompmgr` to stand in for such a window manager.

The drawn window buttons follow XSettings `Gtk/DecorationLayout`, read on a
dedicated x11rb thread. GPUI is built without Wayland, so XWayland's XSettings
also covers Wayland sessions; do not add a GSettings or portal reader unless
that changes. Smoke state publishes the new layout order before the next paint
moves `window_buttons` rects, so poll for settled geometry.

Start a title-row move on the first drag motion, not the press: the window
manager's move grab otherwise swallows the second click of a double-click.
Openbox also grabs the keyboard for a move and releases it only after it
handles the button release, which can come after the window reaches its new
position. Smokes probe with `XGrabKeyboard` until the grab is free before
typing; a key typed earlier is lost.

GPUI's X11 setters such as `set_title` and `set_client_inset` wait for a
checked reply, which reads pending events into x11rb's queue. calloop watches
only the socket, so an event queued this way, such as a new window's
MapNotify, can wait forever: GPUI never starts that window's refresh loop and
requests no further frame. GPUI drains those buffered events after each
foreground task (zed#62081); `bench:scroll` under twm and the composited
client-frame smoke both stall without it. Still seed cached platform values,
such as the window title, with what the window opened with, and call these
setters only when the value changes.
