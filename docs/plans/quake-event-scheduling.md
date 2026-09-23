# Quake event scheduling

Status: reviewed follow-up to merged PR #157 (`d96b832`). Codex and Claude
Opus 5.5 at high effort agree on this plan. Implementation has not started.

## Outcome and scope

Remove the global 16 ms Quake pump while preserving summon, hide, focus return,
fullscreen, display recovery, profile conversion, and retained terminal
behaviour. Settled visible and hidden Quake windows should schedule no
recurring Quake work when reliable native notifications are available.
Animations should advance on actual display opportunities, without a hard-coded
60 Hz or 120 Hz cadence.

Keep the central Quake transition state machine. Native callbacks publish facts
and wake its owner; they do not perform transitions. Reduce duplicated
scheduling mechanics where their contracts match, without merging terminal,
fullscreen, and Quake state into a general application scheduler.

The work targets idle CPU/wakeups and animation cadence. It does not promise
lower RSS or terminal echo latency. Runtime/protocol ownership, glyph
rendering, snapshot representation, GPUI idle display-link policy, and Quake
product behaviour are outside scope. Use native macOS and Docker Linux for
validation. Use the Linux development VM if Docker cannot cover a required native
scenario. Do not create or use a macOS development VM.

## Current behaviour

`desktop/quake_windows.rs::start_pump` starts one global task while the
registry contains windows, including hidden windows. Every 16 ms, `tick` visits
each window and calls `step`. Native effects run afterward, outside GPUI window
borrows. `step` does more than animation:

- Samples native fullscreen, activation, visibility, and frame state.
- Distinguishes application/window blur from non-activating panels on macOS.
- Advances Prepare, Windowed, Animate, Activate, SettleVisible, SettleHidden,
  and Idle stages; handles reversal, recovery, and detachment.
- Retries activation, applies blur suppression, and enforces operation
  deadlines.
- Rechecks displays once per second and during selected transition stages.
- Synchronizes Quake chrome and records opt-in smoke observations.

`desktop/refresh.rs::FrameClock` already shares presentation frames and
animation hold deadlines within each window. `fullscreen_work.rs` owns a
coalesced wake, cancellable task, and earliest-deadline timer for fullscreen
reconciliation. Terminal pending work and runtime activity have independent
lifetime and progress requirements; leave those owners unchanged.

## Proposed ownership and scheduling

Keep `Registry` responsible for profile associations, global shortcuts, and
focus return. Give each attached Quake presentation owned, cancellable
scheduled work. A global notification may target affected Quake windows, but
ordinary window changes must not scan every registered window. Dropping or
detaching a presentation cancels its timers, registrations, and pending
effects.

The owner reconciles current state and returns three separate scheduling needs:
immediate continuation, a display frame, and an optional earliest deadline.
Presentation changes are separate from scheduling needs. A state transition can
need a later turn without requiring a redraw; a static suppression interval can
need a deadline without requesting frames.

Reuse the existing frame clock for animation opportunities. `WorkspaceView` is
already registered by entity ID for tab/reveal animation: merge Quake's frame
need into that registration rather than replacing it or adding a duplicate. Arm
it directly when scheduling changes; do not call `cx.notify` merely to request
the next frame. Its animation callback signals Quake work without native
mutations. Frame callbacks only admit animation work; execute native
frame/style/opacity/focus effects on a later foreground turn, outside every
GPUI window borrow. Coalesce queued frame signals so slow native work cannot
accumulate old animation samples. Sample elapsed time when the effect is
prepared, retain reversal continuity, and preserve operation generations
through deferred execution. Non-frame notifications may advance lifecycle
stages but must not produce extra animation samples. Consume at most one
pending animation opportunity per pass, and invalidate old opportunities on
reversal, display changes, detach, and close.

Make Quake work selection testable with explicit `now` and sampled native
facts. Separate native sampling/effect execution from a deterministic decision
function that produces operations and scheduling needs. Keep the existing
transition and activation policies; avoid a wholesale state-machine rewrite.
Tests must exercise the actual decision function, not a second model of its
intended behaviour.

Extract a small shared wake/deadline primitive from fullscreen work only where
Quake needs the same semantics: non-reentrant signalling, a one-slot wake,
earlier-deadline reuse, cancellation, weak lifetime, and bounded continuations.
Keep fullscreen fallback eligibility and diagnostic counters with fullscreen.
Do not make Quake an ordinary `Animated` painter if that would run native
effects inside the frame clock's GPUI borrow. Preserve existing fullscreen
tests and add shared primitive tests before moving either consumer.

Retain separate state-machine owners. A universal window dispatcher would
couple independent lifecycle rules and risk visiting unrelated work on every
wake. Leaving every consumer with a copied scheduler would spread cancellation
and lost-wake rules. Share mechanics only where the second consumer
demonstrates the same contract; do not refactor terminal scheduling as part of
this PR.

### Trigger replacement map

| Existing polling duty | Replacement and required ordering |
| --- | --- |
| Summon, toggle, reversal, reload, attach/detach | Explicit targeted wake after accepting intent; preserve generations and profile identity. |
| Native effect completion or failure | Publish result and schedule continuation/recovery after the native effect returns. Recheck generation and owner before applying it. |
| Animation progress | One coalesced opportunity from the window frame clock, or the target-display clock/active-only fallback while window frames are unavailable. Frames do not gate native completion or cleanup. |
| Fullscreen transition | Native lifecycle notifications and the X11 fullscreen-property path, routed to Quake while it owns presentation. Keep adapter mutation gates and ordinary fullscreen event-discard semantics. |
| Window activation and true app blur | Window and application activation/key-window notifications; recompute the existing native `active`/`blurred` predicates after callbacks settle. |
| Display identity, topology, scale, work area | Native display/work-area notifications and window geometry changes. Query affected geometry after notifications; coalesce without extending an existing transition deadline. |
| Native visibility and geometry settling | Window map/unmap and bounds notifications plus deadline-paced pending-transition retries where the platform lacks a reliable acknowledgement. |
| Presentation lease acquire/release, including ordinary fullscreen windows | Explicit work-area invalidation to affected Quake owners after the lease mutation. Refresh selected display geometry before entry and on every settling retry. |
| Chrome, notch, safe area, and terminal insets | Reconcile after native state, display/work-area, profile, and geometry changes through the existing visibility/layout path; avoid notifications when values are unchanged. |
| Activation retry, blur suppression, transition timeout | Explicit earliest deadlines, armed only while relevant. Expiry reconciles even when no new event or display frame arrives. |
| Close confirmation, busy state, profile removal, Quit | Explicit wake or cancellation from the existing state mutations. A deferred hide condition must be reevaluated when its blocker clears. |
| Regular-window restore bounds | Bounds notifications while detached-in-progress or regular, without reinstating polling. |

Native notifications are hints to re-read authoritative state. Preserve ordered
fullscreen lifecycle events; coalesce wakeups, not transitions. Consume the
wake before reconciliation so signals produced during a pass remain observable.
Bound each pass and cross a foreground executor turn before continuing backlog.
Register observers before the initial sample, and reconcile after registration
to close the subscribe/sample race.

Separate observation from mutation admission during settling. Bounds events
from our own frame writes can refresh facts but cannot authorize another
immediate write. Maintain a next-reapply deadline (initially the existing 16 ms
retry floor, only while unsettled), plus the unchanged operation timeout.
Coalesce incoming notifications without moving that deadline forward. Reapply
only when due and still necessary; stop on settlement, cancellation, or
timeout. This is native acknowledgement retry pacing, not animation cadence.
Test a constraining window manager and synchronous callbacks to prove writes
stay bounded.

### Platform edges to prove before removing the pump

On macOS, key-window loss alone is insufficient for hide-on-blur. A launcher or
password-manager panel can take key status while Huterm stays active. Observe
application activation and key-window changes and retain the native predicate
in `quake/macos.rs::Window::blurred`. A real app switch after panel opening
must still be detected even if this window receives no second key-loss
notification.

On X11, audit GPUI events against the private Quake connection's authoritative
`_NET_ACTIVE_WINDOW`, fullscreen, map state, work area, and display
observations. If GPUI cannot expose a needed event, use a narrow backend
notification patch or a cancellable native event subscription. If a separate
observer is needed, use a dedicated event connection owned by its worker,
distinct from the existing `Rc<RustConnection>` used for synchronous commands
and smoke queries. Drain library-buffered events before waiting on readiness
plus shutdown signals; request/reply reads can buffer events while leaving the
socket non-readable. Include `_NET_CURRENT_DESKTOP`, `_NET_WORKAREA`,
active-window changes, relevant window properties, and RandR subscriptions in
the audit. Shut down and join the observer when its owner ends. Do not consume
GPUI's connection events from Quake or treat Wayland support as part of this
migration.

Display observation must cover hidden windows and work-area changes, not just
movement between monitors. Preserve display selection/fallback and presentation
lease behaviour. Shared presentation leases can change the menu bar/Dock work
area without a guaranteed screen notification: signal after every acquire or
release that may affect it, including ordinary fullscreen ownership. Verify
when `visibleFrame` settles; sample it again on each deadline-paced
SettleVisible retry without renewing the original failure deadline. Native
external work-area changes still need observation or a narrowly justified
fallback. Never depend on the ordinary fullscreen adapter being present for all
Quake notifications. If a notification cannot be obtained reliably, document
the exact gap and retain only a conditional, measured fallback for that gap. A
permanent universal 16 ms fallback is not acceptance of this plan.

A fully offscreen macOS slide cannot deliver its first window frame: the
vendored display-link code stops when occluded or when NSScreen is nil. X11
also stops its window refresh loop when unmapped or fully obscured. Bootstrap
show with the correct initial geometry/opacity outside GPUI borrows, but do not
rely on that alone to obtain frames.

Provide active-animation progress while window frames are unavailable. Prefer a
clock bound to the target display, feeding the same coalesced animation
admission path. If backend constraints require a deadline-driven
bootstrap/stall fallback, derive its interval from the target display's queried
refresh period, replace it with window frames as soon as they arrive, and
prevent double sampling during handoff. Do not impose a fixed 60 Hz cap.
Establish the available clock/refresh source in step 2. When neither delivered
frames nor a valid display refresh period is available, use an explicit
unsynchronized 16 ms compatibility interval only for the remaining active
animation, matching the existing X11 unknown-mode fallback. Report that
condition in diagnostics and benchmark results; it is not a 60 Hz cap on
displays with usable clocks. Cancel it on settlement and switch to real frame
delivery when available. Any backend extension must remain scoped to active
Quake transitions and preserve normal idle display-link policy.

The fallback runs only during an animation and stops at its endpoint, reversal,
owner cancellation, or operation timeout. A finite end deadline guarantees
completion even if all frame sources stall. Recompute the animation end from
current progress/target and pauses; an earlier reused deadline must not finish
a newly reversed animation prematurely. Keep hide animation visible until the
endpoint is committed. Zero-duration and regular-window conversions progress
without frames. Preserve native transition pauses and the original bounded
failure deadline. Require intermediate native samples for offscreen slide-in
and fade-from-zero, including in the existing Linux compositor harness.

## Implementation and verification sequence

1. **Record a fresh Quake checkpoint.** Extend the idle fixture to create
   genuine Quake presentations and verify their native state. Sample settled
   visible and hidden windows, one and multiple profiles, and an
   ordinary-window control. Cover one and many retained tabs without treating
   tab count as window count. Use isolated configuration, preserved
   baseline/feature binaries, alternating run order, the same display, and an
   unlocked relatively idle Mac. Record CPU, wakeups, RSS, threads, hashes, and
   clean Quit. Set `hide_on_focus_loss = false` in visible idle arms and verify
   actual native visibility before and after every sample. Record display mode
   and measure delivered cadence separately in instrumented animation runs.
   Disable periodic smoke-state publishing during idle sampling; readiness must
   not keep it alive.
2. **Prove the notification map and scheduling contract.** Inventory every
   caller that mutates Quake stage, desired visibility, activation,
   suppression, config, busy/close state, or ownership. Establish platform
   event sources and lifetime before replacing the driver. Add controlled-time
   tests for work selection and shared wake/deadline mechanics. Preserve
   fullscreen behaviour if extracting helpers. Establish offscreen animation
   clock delivery before migrating the driver; a window-frame-only
   implementation cannot pass acceptance.
3. **Migrate Quake scheduling.** Add targeted event reconciliation and deferred
   native effects, then frame-paced animation and finite deadlines. Exercise
   all correctness tests without the old driver so it cannot repair missing
   notifications. Replace the `start_pump` call sites directly during cut-over;
   the old driver must be unreachable in this build. Each Quake window then
   owns its scheduled work. Preserve generations, recovery, and focus
   semantics.
4. **Validate native behaviour and efficiency.** Run focused Rust/script tests,
   `smoke:macos-quake` on the host and `smoke:linux-quake` through Docker, plus
   fullscreen, refresh, and Quit coverage affected by shared scheduling
   changes. Run `mise run verify` before delivery. Compare preserved baseline
   and feature binaries in alternating order. Do not run both transition
   drivers together: that risks double advancement and does not improve
   behavioural evidence.
5. **Remove the old driver.** Delete `start_pump`, global tick scans, and
   obsolete once-per-second display sampling after replacement coverage passes.
   Commit cut-over and dead-code removal together so every committed revision
   passes the normal Clippy gate. Update existing smokes that assume periodic
   focus observations to acknowledge a reconciliation that sampled the
   post-event native state. The smoke state publisher must remain read-only and
   never wake production work. Record results and remaining fallbacks in the
   renderer performance notes and link this follow-up from the event-driven
   plan. Independent review precedes PR readiness.

### Required behavioural evidence

- Work selection chooses the earliest activation retry, blur-suppression
  expiry, animation/fallback deadline, geometry retry, and operation timeout.
  Reversal under an earlier reused timer preserves progress. Constrained
  geometry and self-generated bounds events cannot bypass retry pacing or
  extend timeouts.
- Wakes during reconciliation are retained; burst events coalesce;
  continuations yield; close/detach cancels queued callbacks and effects,
  including timer races.
- Settled visible and hidden Quake owners perform no recurring reconciliation
  or animation-frame requests. Use opt-in counters separately from
  uninstrumented performance runs; observe quiescence after a known completed
  transition.
- Summon from hidden state, zero-duration effects, reversal, repeated
  shortcuts, all existing animation modes, and resummon preserve the same
  native window and PTY. At available native refresh rates, animation samples
  follow delivered frames rather than a 16 ms cap. Do not claim physical
  presentation latency from CPU-side effect timestamps.
- Paused-frame tests still complete show/hide, failure timeout, detach, and
  Quit. Keep all existing Linux intermediate-animation and compositor fade
  assertions mandatory. The active-animation clock must work when ordinary GPUI
  window frames stop; missing frames must not turn animations into endpoint
  jumps. Test frame/fallback handoff for duplicate samples and release at idle.
- True app blur hides when configured; a non-activating panel does not. Test
  panel then app switch, switching between Huterm windows, blur during
  suppression, and close/busy blockers clearing without another focus event.
  Preserve focus return and bounded activation retries without stealing focus
  after initial activation.
- External fullscreen entry/exit, ignored WM requests, rapid toggles, timeout
  and recovery, profile removal, and conversion to a regular window retain
  their existing semantics. Notifications arriving under Quake ownership cannot
  corrupt the ordinary fullscreen controller when ownership returns.
- Ordinary fullscreen lease acquisition/release and Quake's own lease changes
  update work area without needing a native screen notification. Repeated
  settling samples catch delayed native work-area updates while retaining the
  deadline.
- Display/work-area change during animation and while hidden updates the
  endpoint without restarting timeout indefinitely. Test deterministic injected
  events and available physical displays; report unavailable
  hotplug/multi-display coverage.
- Idle cost improves beyond run variability, and ordinary-window idle, terminal
  echo/flood, and scroll comparisons show no material regression. Record memory
  and thread changes without claiming improvements unsupported by measurements.

## Unresolved questions

- Which X11 events GPUI already exposes with sufficient ordering, and which
  need a small backend patch or private subscription, is resolved in step 2
  before pump removal. The same audit must prove macOS app/key-window and
  work-area coverage.
- Which target-display clock or refresh-derived active-animation fallback is
  available on each backend? Step 2 must settle the mechanism; offscreen
  windows are known not to guarantee ordinary window frames. Step 3 proves
  handoff, intermediate samples, and cancellation without weakening existing
  smokes.
- Which physical 60 Hz, 120 Hz, and multi-display cases are available during
  the fresh comparison? Record coverage limits rather than substitute VM
  timing.
