# Quake profiles and global hotkeys

Status: implementation for [issue #41][issue-41]; native macOS CI validation pending.

Deliver one PR for the embedded quake workflow. A configured global shortcut
summons a named terminal window from another application. Hiding it preserves
its session and jobs, returns focus, and leaves Huterm running to service the
next shortcut. Repeated activation reuses the same window.

Frameless quake presentation, auto-hide on focus loss, temporary regular-window
presentation, retained display choice, and animation variants are agreed behavior.
Configuration spelling and numeric defaults below are implemented.
Keep #24's palette in a separate PR.

## Scope and current foundations

[Epic #8][epic-8] puts this delivery after fullscreen and before server work.
The command catalog, configurable in-app bindings, fullscreen controller, and
embedded close/Quit lifecycle have landed. Issue #21's remaining server portion
does not block this work.

Current implementation points:

- [Desktop coordination][windows] owns configuration, weak window references,
  pending spawns, command routing, and assessed Quit. Ordinary New Window
  creates a private session and workspace. There is no profile registry.
- `run_app_command` implements `hide` through application-wide `cx.hide()`.
  Quake needs individual window visibility and must leave ordinary windows alone.
- [Configuration][config] already validates reload before publishing new
  settings and bindings. Native registration adds fallible external effects.
- [Command definitions][commands] support application scope and text arguments.
  Profile commands need no new runtime identity or GPUI type in protocol.
- [FullscreenController][fullscreen] owns mode, pending operations, generations,
  recovery, and restorable bounds. Quake must coordinate with that owner.
- The workspace uses patched, vendored GPUI 0.2.2 with X11 enabled. Its public
  window API exposes activation and minimization, but lacks the individual
  hide/show and arbitrary placement operations required here. Use the vendored
  source as the API reference, not the older assumptions in the fullscreen plan.

Include profile config, commands, native global registration, placement,
animation, focus, lifecycle integration, diagnostics, docs, and native tests.
Exclude persistent appearance opacity/blur, palette UI, shared viewers, arbitrary
session adoption,
daemon/IPC, login-item installation, and durable restore. Preserve an integration
point for #42 to save profile associations later without defining a disk format.
Transient window opacity for fade animations is included.

## Proposed user contract

### Configuration and commands

Provide the built-in `default` profile even when no quake config exists. Create
its window lazily. Keep ordinary application startup and New Window unchanged.
Do not register an unsolicited default global shortcut; document a ready-to-copy
example. Users must launch Huterm before global shortcuts work.

Proposed configuration:

```toml
[quake.profiles.default]
edge = "top"
display = "active"
width = 1.0
height = 0.5
fullscreen = false
hide_on_focus_loss = true
animation = "auto"
animation_ms = 150

[quake.profiles.logs]
edge = "right"
display = "pointer"
width = 0.4
height = 1.0
animation = "fade_slide_right"
animation_ms = 150

[[global_keybinding]]
key = "ctrl-alt-t"
command = "toggle_quake"
args = { profile = "default" }

[[global_keybinding]]
key = "ctrl-alt-l"
command = "toggle_quake"
args = { profile = "logs" }
```

Register `show_quake`, `hide_quake`, and `toggle_quake` as application commands.
Their optional text argument `profile` defaults to `default`. Unknown profiles
return a useful error without creating a window. Explicit profile arguments work
from normal keybindings and future palette/programmatic callers.

Global entries initially support these three commands, one key stroke, and no
`when` predicate. Use the existing modifier spelling where native matching can
honor it. Reject unsupported keys/modifiers and multi-stroke sequences rather
than silently approximating them. Do not route global events through GPUI's
ordinary shortcut matcher or terminal-input reservation. Detect exact conflicts
between global and in-app bindings so one press cannot execute both paths.
Suppress key-repeat until release, including when the target window gains focus.

Validate nonblank profile names, known edges and display selectors, finite
width/height fractions in `(0, 1]`, and animation duration from 0 to 1000 ms.
Unspecified fields inherit built-in defaults. `fullscreen = true` uses the
display frame and overrides width/height. With `animation = "auto"`, fullscreen
profiles fade and partial profiles fade while sliding from their anchored edge.
Explicit animation directions are independent of placement edge.

### Association and lifecycle

Each profile owns at most one dedicated window and its private embedded session.
Reserve the profile entry before asynchronous shell creation. A second request
updates the desired state of that entry rather than spawning another window.
Failure clears the reservation and reports the cause so a later invocation can
retry. Never expose core session names as profile identity.

`show_quake` is idempotent and focuses the target. `hide_quake` is idempotent and
does not create a missing window. Toggle flips desired visibility during a
transition. For a visible but unfocused window, toggle first raises and focuses
it; the next toggle hides it. Quake presentation hides on focus loss by default;
`hide_on_focus_loss = false` disables that behavior. Temporarily regular windows
suspend auto-hide. Focus changes inside the same window do not count as blur.

Hiding retains the window, attachment, tabs, and processes. Stop hidden-window
snapshot preparation and painting while continuing bounded lifecycle event
processing. Resume and invalidate on show. Cancel terminal composition and
accepted mouse gestures through existing focus/visibility cleanup paths.

Explicit window close and final-tab exit keep the existing lifecycle semantics.
Closing the last tab closes the window fully. Successful close removes the
profile association and its remembered display choice; its next show creates a
fresh session and resolves the display from profile configuration again.
Registered global shortcuts keep the application available even if no
windows or sessions remain. Explicit Quit always performs assessed cleanup and
unregisters shortcuts. A canceled Quit leaves profiles and registrations usable.
Expose and focus a hidden window when its close or Quit requires confirmation.
Do not allow a hide request to make an active confirmation inaccessible.

### Geometry, animation, and fullscreen

Quake presentation has no native window frame, titlebar, or window controls,
for both partial and fullscreen profiles. Huterm's tab bar follows its configured
single-tab and fullscreen visibility policies.
Partial profiles use the display's usable work area, honor minimum window size,
and center on the axis perpendicular to their selected edge. Resolve `active`
from the previously focused application's display and `pointer` from the pointer
when creating the profile's window. Also support `primary` and a platform display
identifier, with a documented fallback to primary when the target is gone. Never
persist transient display enumeration indices as stable identifiers.

For a profile-associated window, `toggle_fullscreen` switches between quake
presentation and a regular framed, movable/resizable window. The profile remains
associated with the same window, session, tabs, and jobs in either presentation.
This applies even to partial profiles. Ordinary windows retain their existing
fullscreen command behavior. Explicit native/non-native fullscreen commands must
be reconciled with this presentation state during implementation; they must not
bypass transition serialization or lose the profile association.

In regular presentation, suspend auto-hide and restore usable regular bounds.
Toggling back applies the profile's quake geometry on the display containing the
regular window. Keep that display choice through later hide/show cycles without
rewriting the profile config. Showing an existing regular window preserves its
regular presentation; only the presentation toggle returns it to quake geometry.
Closing the last tab or explicitly closing the window clears the display choice.
A subsequent invocation starts from the configured display selector again.

Support these animation choices:

| Value | Show behavior |
| --- | --- |
| `auto` | Fade for fullscreen; fade plus slide from the anchored edge for partial profiles. |
| `none` | Show immediately. |
| `fade` | Fade in at the target position. |
| `slide_top`, `slide_bottom`, `slide_left`, `slide_right` | Slide in from the named edge. |
| `fade_slide_top`, `fade_slide_bottom`, `fade_slide_left`, `fade_slide_right` | Fade in while sliding from the named edge. |

Hide reverses the selected effect, fading out and/or sliding toward that same
edge. Combined effects share one duration and progress value. Explicit choices
work for both fullscreen and partial profiles. Duration zero is immediate.
Keep animation opacity separate from persistent appearance settings, and restore
normal opacity after completion, cancellation, failure, or conversion to regular
presentation. Rapid reversal starts from the current position and opacity,
without jumping to an endpoint. Regular presentation uses ordinary show/hide
without quake animation.

Keep the visible target geometry separate from offscreen animation frames so
animation never becomes restore metadata or changes the remembered display.
Animate window position and opacity, not terminal grid size. Resize the PTY for
the final target bounds rather than on each animation frame.

Fullscreen profiles use the existing non-native mode on macOS and native EWMH
fullscreen on X11. Serialize visibility, presentation, placement, and fullscreen
transitions; never let two controllers mutate frame, opacity, or style concurrently.
Verify directional animation of fullscreen windows on each native backend before
claiming support. Fully hidden macOS quake windows must release Dock/menu-bar
presentation leases. Showing reacquires presentation through the existing
fullscreen owner. Converting to regular presentation restores frame and controls
and releases quake fullscreen presentation state.

Display changes invalidate placement. Recompute or recover to the fallback
display without spawning another window. Respect safe areas, scale changes, and
the fullscreen controller's existing recovery state. Intermediate animations
must be cancelable and generation checked. An ignored native request must time
out with an error, not leave the registry permanently transitioning.

### Focus and configuration reload

Capture the previous external application/window before activating a profile.
Preserve that return target across switches between quake profiles. Hide returns
focus only if Huterm still owns focus; it must not steal focus from an application
the user selected during the transition. Auto-hide after focus loss must preserve
the newly focused application/window instead of restoring an older target. Suppress
blur-driven hides while presenting close/Quit confirmation or while native style
changes temporarily disturb focus. If the saved target disappeared or the
platform refuses activation, leave focus with the OS and report the limitation.

Prepare and validate reload before changing live config. Retain unchanged
registrations, acquire new registrations, and retire removed registrations only
after the replacement set succeeds. Queue native callbacks with a registration
generation so stale events cannot invoke a replaced profile. Where native APIs
require releasing a conflicting old registration first, track rollback and
report any failure to restore the old registration truthfully.

An invalid reload preserves the last working settings. Startup registration
failure must leave an ordinary usable window and show a diagnostic identifying
the shortcut and platform error. Also log failures when no window is visible.

Changing profile geometry applies at the next quake show or return from regular
presentation, avoiding mid-gesture jumps. Preserve a manual display choice while
the window survives; display configuration changes apply to its next creation.
Removing a profile releases its shortcut and converts any surviving window into
an ordinary visible window without terminating its session. Treat a config-key
rename as remove plus add, with the same non-destructive behavior. Removing an
override for `default` resets that profile's settings and retains its association.
Document these rules so editing a name cannot silently kill or duplicate jobs.

## Ownership and implementation shape

Use a desktop-owned registry with a small per-profile state machine. It owns
profile-to-window association, creation reservation, desired visibility,
quake/regular presentation, remembered display choice, transition generations,
focus-return targets, and registration lifetime.
Core retains sole ownership of sessions, terminals, and PTYs.

| Location | Responsibility |
| --- | --- |
| `huterm-protocol/src/command.rs` | Three catalog entries and argument validation. |
| `huterm-gpui/src/config.rs` | Serialized profiles/global entries and diagnostics. |
| New private `quake.rs` | Profile resolution, registry transitions, placement policy. |
| New private native quake modules | Hotkeys, exact-window visibility/placement, focus observation and return. |
| `desktop/windows.rs` | Create/retain windows, execute effects, route commands, integrate reload and Quit. |
| Existing fullscreen modules | Own fullscreen transitions, geometry recovery, and presentation leases. |
| New desktop smoke and scripts | Native events, external focus witnesses, PTY and lifecycle evidence. |

Native callbacks enqueue bounded events and wake the desktop. They do not enter
Mux, dispatch commands recursively, or block the GUI. Capture exact native window
handles while borrowed; execute reentrant native mutations outside GPUI update
borrows. Ignore completions from old generations or closed windows.

The registry hides duplicate prevention and transition ordering from callers.
Callers supply a validated command and profile name and receive completion,
acceptance of asynchronous work, or an error. Runtime failures surface through
the existing status/log path. Native operations remain private; do not build a
general window-management framework for future clients.

An alternative is to put all quake state on each `WorkspaceView`. That handles
visibility locally but leaves no owner before a window exists or after it closes,
spreading creation deduplication and registration lifetime across call sites.
Prefer the registry while retaining fullscreen state on its current window owner.
A daemon is another possible owner, but conflicts with the embedded requirement
and introduces unnecessary IPC and application-lifetime changes.

## Delivery sequence

1. Prove native feasibility on the pinned framework. Verify global registration,
   individual show/hide, cross-application focus, work-area/display lookup, and
   fullscreen lease release on macOS and X11. Inspect primary API documentation
   and native source before selecting an implementation. Compare a maintained
   permissive hotkey crate with narrow native adapters. Apply the three-day
   release-age and license policies to any dependency. Keep Wayland unsupported
   explicitly; XWayland does not prove compositor-global shortcuts.
2. Add config and catalog contracts with focused validation tests. Stabilize the
   command names/arguments here so #24 can start separately if desired.
3. Implement the registry and lazy window association. Verify duplicate requests,
   failures, close/reopen, reload removal, and application keepalive first.
4. Add native visibility, placement, animation, focus return, and serialized
   fullscreen integration. Prove the complete user loop on each platform before
   expanding the failure matrix.
5. Complete registration reload/rollback, error reporting, docs, native smokes,
   and review. Keep these steps in one PR unless platform evidence exposes a
   materially larger requirement that warrants revisiting scope.

Use narrow local adapters where sufficient. If a GPUI patch is necessary, use
the repository's vendor workflow and preserve its reproducible patch recipe.
Do not upgrade GPUI as an incidental part of this feature.

## Verification and acceptance

Test state and observable behavior through the same command interface as callers.
Use a fake only for the native registration/window boundary and an injected clock.
Cover successful summon/hide, pending-toggle parity, duplicate creation, spawn
failure/retry, stale completion, close during animation, canceled Quit, hidden
final-tab exit, profile removal, and registration rollback failure. Exercise
placement with scale changes, removed displays, and minimum-size constraints.
Confirm each new test runs and fails at its intended assertion before its fix
where practical. Do not treat existing green suites as new-feature coverage.

Add discoverable `smoke:macos-quake` and `smoke:linux-quake` Mise tasks. Invoke
actual OS-global shortcuts while another application owns focus. Observe a
unique shell ACK, start a job, hide, verify external focus, and summon again with
the same window/session/job identity. Run PTY assertions with both engines.
Use one engine for pure native-window scenarios where duplication adds no proof.

Native coverage must include:

- two profiles and shortcuts, customized default, repeated activation, key-repeat
  suppression, and toggle/show/hide during animation;
- frameless partial/fullscreen geometry, regular-window toggle and frame recovery,
  retained display choice across hide/show, and reset after last-tab close;
- auto-hide on blur, its opt-out, suspension in regular presentation and during
  confirmation, and focus preservation when switching between profiles;
- fade, all directional slides, combined fade/slide, automatic defaults, zero
  duration, rapid reversals, and opacity restoration after cancellation/failure;
- hidden macOS presentation leases and fullscreen directional animation;
- registration conflicts, reload replacement/removal, stale callbacks, and
  errors with no visible window;
- previous-target disappearance and deliberate focus changes during transitions;
- hidden-window job survival, shell exit, explicit close, canceled/retried Quit,
  and hotkey availability with zero windows;
- terminal focus, pending shortcuts, composition, mouse cleanup, and final PTY
  geometry after animation.

Use the existing X11 smoke conventions: Xvfb with `-noreset`, an EWMH window
manager, XTest input, and root-relative geometry inspection. Keep raw PTY readers
in the foreground process group. A separate native application/window must
witness focus; an internal active-window flag alone is insufficient.

Record physical multi-monitor macOS and Linux evidence for target selection,
display disconnect, scale changes, and repeated toggles. Include macOS Spaces,
Stage Manager, and Dock/menu-bar behavior. Synthetic display tests and headless
CI do not establish these results. State unavailable coverage explicitly.

Run focused protocol/GPUI tests during implementation, `mise run check:scripts`
for smoke tooling, and relevant existing fullscreen/input/menu/Quit smokes.
Run `mise run license` for dependency changes and `mise run vendor:check` for
vendored changes. Run `mise run verify` before implementation handoff and bind
CI/review evidence to the final PR head.

Run `mise run verify` and the platform's native quake smoke before handoff.

## Implementation decisions and validation boundaries

- Defaults are top-edge, full work-area width, half work-area height, and 150 ms.
  Global shortcuts remain opt-in. Profile fractions are finite values in `(0, 1]`;
  animation duration is 0 through 1000 ms.
- Global registration uses pinned `global-hotkey` 0.8.0. Callback admission is
  bounded, releases update held-key state before admission, and a coalesced
  asynchronous wake dispatches work on GPUI. Registration replacement acquires
  new grabs before retiring old ones and tracks failed rollback ownership.
  Modifier-free supported keys are accepted. There is no default global grab.
- Display selectors are `active`, `pointer`, `primary`, and `id:<identifier>`.
  Native identifiers are CoreGraphics UUIDs and RandR monitor names. Profiles
  retain the initially selected display; missing displays fall back to primary.
- The profile-associated window owns a single quake presentation reducer. Its
  ordinary fullscreen controller is suspended while associated. The existing
  macOS observer still reports native Space transitions, and quake shares its
  application presentation lease pool. Explicit native/non-native commands are
  unavailable while associated; the ordinary toggle switches presentation.
- Reentrant AppKit effects run outside GPUI update borrows. Every effect checks
  the request generation. Recovery and profile removal wait for native Space
  exit before restoring frame/style. X11 visible endpoints reassert placement
  after mapping because the window manager may override unmapped geometry.
- Three narrow GPUI vendor patches preserve hidden creation, expose the exact
  live XCB window handle, and leave zero-window application lifetime to Huterm.
  Archive-plus-patch reproduction covers each patch.
- `smoke:linux-quake` and `smoke:macos-quake` run in the platform CI smoke job.
  They drive native shortcuts from a separate focus witness, verify retained
  PTYs, exercise 22 partial/fullscreen animation combinations and lifecycle
  boundaries, and check real OS grab conflicts. Linux observes composed pixels
  during fade under Xcompmgr. macOS observes native alpha and drives real AppKit
  Space entry/exit; its witness requires session event-posting permission.
- Physical multi-monitor layouts, Stage Manager, and user-specific Spaces
  arrangements remain manual QA limits. They do not excuse missing native CI
  coverage. Current local evidence is Linux X11; macOS execution remains a CI gate.

[issue-41]: https://github.com/jimeh/huterm/issues/41
[epic-8]: https://github.com/jimeh/huterm/issues/8
[windows]: ../../crates/huterm-gpui/src/desktop/windows.rs
[config]: ../../crates/huterm-gpui/src/config.rs
[commands]: ../../crates/huterm-protocol/src/command.rs
[fullscreen]: ../../crates/huterm-gpui/src/fullscreen.rs
