# Fullscreen event scheduling and refresh-pump removal

Status: scheduler migration and legacy-pump removal implemented after the
accepted three-arm comparison. Final validation remains pending. This implements
steps 7 and 8 of the
[event-driven refresh plan](event-driven-refresh.md), following PR #155. The
inspected production source is `99aef30`, equivalent to merged `84e6fca`; `main`
subsequently added the 0.12.3 release bump (`a3b35f0`). Refresh the base and
recheck these seams before implementation.

## Outcome and scope

Stop waking every window every 16 ms just to reconcile fullscreen state.
Fullscreen must still respond to commands, external native transitions, display
changes, asynchronous completions, failures, and timeouts when frames stop. Keep
rendering paced by delivered display frames, with no fixed refresh-rate
assumption. Preserve the existing command, restoration, focus, PTY geometry,
Quake, and close behavior.

The performance target is lower idle CPU and wakeups, especially for hidden
windows. This is not a promise of lower input-to-display latency or memory use.
The [last paired
measurements](../performance/gpui-terminal-renderer.md#pointer-and-pending-work-implementation-2026-09-23)
showed about 26% lower visible idle CPU after steps 5 and 6, but approximately
180 visible and 60 hidden interrupt wakeups per second remained. Those are
process counters, not attribution to individual timers. GPUI's display link
remains a separate wake source.

This PR does not change runtime/protocol ownership, snapshot admission,
animation pacing, the GPUI display link, rendering caches, or terminal storage.
Keep the conditional macOS pointer probe and busy-terminal retries from PR #155.
Their limited scheduling is independent of the fullscreen pump. Do not turn this
work into a GPUI upgrade or a new general-purpose scheduler. A narrow vendored
X11 notification fix is in scope, as described below. The separate global Quake
pump is out of scope and must appear in the remaining-polling inventory.

## Current boundary and proposed design

`desktop/windows.rs` creates a detached 16 ms loop per window. Its only duty is
`refresh_fullscreen`, which observes state before advancing the controller.
`fullscreen.rs` owns logical mode, serialized operations, generations, restore
bounds, and five-second transition deadlines. `native_fullscreen.rs` owns AppKit
mechanics, event queues, display refits, cleanup, and presentation leases.

AppKit callbacks currently queue events or set `screen_changed`; `Adapter::emit`
also queues asynchronous completions and failures. `Adapter::drain` can itself
schedule deferred display reconciliation and native-exit settling. Every one of
these producers needs a wake, including work emitted after a drain returns.
Commands currently observe queued changes before selecting their target.
Preserve that ordering: do not execute effects for an obsolete desired state
first.

Use the existing `TerminalView` pending-work task as the local model: a bounded
one-slot channel with nonblocking `try_send`, an owned task, and weak entity
access. Use one window-owned, cancellable fullscreen task with a coalesced wake
and an optional earliest deadline. The task holds weak view/window ownership.
Native callbacks enqueue plain state and signal the wake; they must not
synchronously read or update GPUI windows or run fullscreen effects. The wake
operation must be non-reentrant: sending must never execute its receiver inline.
This also protects adapter `saved` borrows held across AppKit `setFrame:` calls,
which can synchronously trigger GPUI bounds observers.

The task reconciles on the UI executor through the correct window update
context, then rearms from current controller state. A wake received during
reconciliation must remain observable. Bound each pass and schedule a
continuation if work remains, without waiting for another producer or a display
frame. Coalesce wake signals, not ordered Will/Did/Complete/Failed lifecycle
events. Suppress redraws when presentation did not change, and avoid
self-notification loops.

`FullscreenController::next` currently consumes `defer_next` after a native Did
notification and returns `None` once. Preserve this as an explicit scheduling
result (operation, deferred continuation, or idle), not as an indistinguishable
idle return. Schedule the deferred continuation on a genuinely later foreground
executor turn. Rechecking in the same drain loop, or receiving an already-ready
channel item in the same future poll, does not satisfy this boundary. The queued
opposite toggle must run without another producer or frame. Verify AppKit
accepts that later turn; if a settle deadline is necessary, justify it with
native evidence and keep it scoped to that transition.

Expose the controller's next transition deadline through a small private query.
Use one cancellable timer for that window's fullscreen work. Cancel it when no
operation or compatibility probe needs it; replace it when an earlier deadline
is required. On firing, read the current generation and state rather than act on
a captured operation. Expiry, failure, and completion must re-evaluate the next
operation and its deadline. Neither an idle window nor an idle terminal should
acquire a new retry loop. This deadline task is independent of visual frames; it
does not need to be folded into the animation clock.

Bounds, scale/display changes, relevant entity notifications, and adapter events
only signal the task. They must never call fullscreen reconciliation inline:
native geometry effects hold adapter borrows while invoking synchronous GPUI
callbacks. Commands retain their existing inline observe-before-intent ordering,
then signal/rearm the task after advancement, including early failures. Do not
add activation as a generic trigger unless a concrete missing duty requires it.
Windowed bounds must remain current for restoration even when no fullscreen
transition is pending. Retain terminal geometry synchronization from PR #155 and
avoid scanning tabs on unrelated pointer events. Reconcile safe-area/notch
changes and refresh presentation only when their values change.

Every deferred adapter task completion must signal reconciliation, including
successful or unchanged display refits with no emitted event and native-exit
settling. This publishes safe-area/notch changes even when geometry is
unchanged. Distinguish completion from a request to retry the refit. The
mid-refit `screen_changed` self-retry must use a coalesced deadline no earlier
than 16 ms later, matching its former maximum retry cadence, rather than
spinning on an immediate wake. A completion wake can publish state but must not
consume that retry early. A fresh screen notification received while a retry
deadline is pending coalesces into that deadline rather than starting an
immediate refit. Include this deadline in the task's earliest-deadline
query; cancel or invalidate it on close, superseding generation, or ownership
change. Test repeated display churn and eventual settling without another
notification.

Quake continues to own its frame/style effects. Preserve the adapter gate
updates and event-discard behavior while Quake owns the window; a fullscreen
wake must not start an ordinary fullscreen operation there. Attach and detach
are explicit wake sites: detach replaces the controller and must reconcile
observed state without a later command. Window close cancels the task, closes
the native mutation gate, and prevents queued callbacks or deferred refits from
reviving state. Preserve lease cleanup and generation guards.

### Platform coverage and fallback decision

On macOS, wire both native notification enqueue paths and `Adapter::emit`, plus
screen-change-only events. Cover external green-button transitions as well as
Huterm commands. Observer installation failure must preserve the existing
degraded native-flag behavior; it must not leave an accepted operation stuck.

On Linux, the vendored X11 backend's `_NET_WM_STATE` PropertyNotify path updates
`state.fullscreen` without invoking a GPUI callback. ConfigureNotify is
therefore insufficient: it can precede the property change or be absent when
geometry does not change. Close this known gap with a small vendored GPUI patch,
using the repository's `vendor:start` / `vendor:finish` / `vendor:check`
workflow. When the fullscreen bit changes, invoke the existing resize/bounds
callback with current geometry after releasing the X11 state borrow. GPUI's
`bounds_changed` notifies observers even for unchanged dimensions; Huterm's
observer only signals its task. Do not notify on unchanged fullscreen bits or
unrelated property changes. Verify the full path with controlled property/bounds
ordering and an external WM toggle in the Linux smoke. No new GPUI public API or
default Linux sampler is planned. If the existing callback proves unsuitable,
revisit this narrow patch decision before inventing another observer API.

On macOS without a working adapter, retain a 16 ms compatibility sampler for the
lifetime of that ordinary window, preserving current external-transition latency
and timeout behavior. Suppress it while Quake owns the window, resume on detach,
and cancel on close. Its cost must be measured separately. This explicit
degraded path avoids making an unproved assumption about style-mask/bounds
notification ordering. Native-adapter windows and patched X11 windows have no
settled-idle fullscreen sampler. Do not claim universal zero polling.

Record fallback eligibility from adapter creation failure, not from the adapter
being absent later. Approved Quit takes adapters while windows remain alive.
Stop and permanently disarm fallback scheduling when `fullscreen.close()`
begins, even before window removal or task cancellation. Add a construction-skip
seam available only to the fullscreen smoke, not end-user configuration. With
the old pump disabled, force this path and verify an external native toggle is
observed, then verify the sampler stops on close and approved Quit. Normal idle
benchmark arms must assert successful adapter installation; measure the forced
degraded path separately.

Rejected alternatives: slowing the universal pump retains polling and delays
reactions; driving fullscreen from animation frames loses progress under
occlusion; a new cross-platform scheduler adds abstractions beyond this duty.
The per-window task keeps platform mechanics in the adapter and policy in the
existing controller.

## Checkpoint event-source inventory

The checkpoint tooling leaves the production fullscreen pump unchanged. The
following producer map is the implementation checklist for its replacement:

| Current source or duty | Replacement trigger or boundary |
| --- | --- |
| `desktop/windows.rs` per-window 16 ms `refresh_fullscreen` | Window-owned coalesced task, canceled on close. |
| `observe_fullscreen` samples windowed restore bounds and native flags | Bounds callback signals only; explicit compatibility sampler only after macOS adapter construction failure. |
| Fullscreen command router observes before selecting intent | Preserve inline observation, then signal/rearm after advancement and failure. |
| Native Will/Did observer event queue | Ordered queue plus non-reentrant wake for every enqueue. |
| `native_fullscreen::Adapter::emit` completion/failure | Signal after enqueue, including deferred completions. |
| Native `screen_changed` and `Adapter::drain` deferred refit | Signal initial work; publish successful/unchanged completion; coalesce mid-refit retries behind a 16 ms deadline. |
| Deferred native-exit settling | Completion wake even without another event or geometry change. |
| Controller `defer_next` following Did | Explicit continuation on a later foreground executor turn. |
| Controller five-second pending-operation deadline | Earliest cancellable deadline independent of frames; re-read current generation. |
| Safe-area and notch presentation after deferred work | Completion reconciliation compares presentation and notifies only on change. |
| X11 `_NET_WM_STATE` PropertyNotify mutates fullscreen bit | Vendored bounds callback after state borrow release, only when the bit changes. |
| Quake attachment/detachment replaces ownership/controller | Explicit wake and mutation gate; suppress ordinary fallback while Quake owns the window. |
| Close, approved Quit, adapter removal | Permanently disarm on controller close; taking an adapter must not enable fallback. |

The separate Quake global pump, conditional macOS pointer probe, busy-runtime
pending-work retries, and GPUI display link remain outside this migration.
The [idle checkpoint tooling](../performance/gpui-terminal-renderer.md#reproducible-idle-checkpoint-tooling)
excludes Quake and process labels and does not add a measurement-period state loop.

## Implementation and verification sequence

1. **Capture a fresh checkpoint and event-source inventory.** Start from current
   `main`, keep separate target directories for baseline and feature builds, and
   record exact revisions, host load, display refresh/scale, visibility, and
   process-label configuration. Build a reproducible opt-in `bench:idle` Mise
   task, initially macOS-only: the earlier multi-tab startup hook was temporary
   and is not committed. Require an unlocked GUI session and verify lock state
   before and after each sample; discard samples crossing a lock boundary.
   Create tabs through production commands, acknowledge readiness, then stop
   control traffic for the measurement interval. Do not reuse a smoke's 10 ms
   state loop. Use isolated configuration with no Quake windows; its separate 16
   ms pump would confound attribution. Measure one and fifty idle tabs in
   visible and hidden states using alternating baseline/feature order and
   repeated samples. Include a two-window check because the timer is per window.
   Record CPU, interrupt wakeups, resident memory, and thread count. Compare
   three arms: baseline, feature with the old pump enabled, and feature with it
   disabled. The latter two isolate the timer's marginal cost from the new
   scheduler. Map every current pump duty and producer to its replacement
   trigger, including bounds sampling, `defer_next`, Quake handoff, and adapter
   deferred work.
2. **Add the wake and deadline path while retaining an explicit rollback.** Wire
   producers and the owned task, controller deadline query, and platform
   observers and the narrow X11 vendor patch. Keep a development/test switch
   that disables the old pump entirely. All correctness evidence for migration
   must run with that pump disabled. Do not allow a repairing fallback to
   conceal missing production wakes. Add focused controller/task tests alongside
   this change.
3. **Prove platform behavior without the pump.** Run the existing fullscreen,
   refresh, Quake, and Quit smokes plus targeted extensions below. Verify the
   X11 property callback and the explicit macOS no-adapter fallback before
   removal. Xvfb/Openbox with controlled event ordering is the automated gate.
   Docker runs the supported Linux integration checks. Also use
   a no-WM Xvfb property-only fixture for unchanged bounds, without counting it
   as proof of real window-manager behavior. Use native physical-display checks
   for display migration/notch behavior that headless fixtures cannot establish.
   Mark
   unavailable physical cases explicitly.
4. **Measure and remove the redundant loop.** Repeat the checkpoint and the
   output/scroll benchmarks with matched conditions. Remove the global
   per-window pump only after event coverage and deadlines pass independently.
   Retain only justified platform-specific fallback work. Remove temporary dual
   scheduling and the old-pump switch once evidence is captured. Retain opt-in
   reconciliation/timer/fallback counters for one release, read on demand or
   through test acknowledgements. Do not introduce a recurring diagnostic
   watchdog that repairs missing wakes or contaminates measurements. Document
   final results and remaining polling in the existing performance report and
   update steps 7 and 8 in the parent plan.

Keep checkpoint, scheduling, and final removal/result changes independently
reviewable. If the replacement does not reduce recurring work, or measurement
shows a regression beyond run-to-run variability, investigate before claiming an
improvement. If native coverage cannot be established, retain the existing path
for that platform and explicitly leave its removal incomplete. Do not increase
architectural scope just to finish the checklist.

## Migration checkpoint

The fresh baseline at `f3ec40e` is recorded in the
[performance report](../performance/gpui-terminal-renderer.md#fullscreen-scheduling-checkpoint-2026-09-23).
The three-arm comparison accepted 72 samples at 120 Hz on the unlocked native
host. The window-owned task replaces the legacy loop, which is now removed.
A separately spawned foreground task provides the continuation boundary. Native
wake callbacks never reconcile inline. Scheduled native event batches are capped
at 32; effects wait until ordered backlog drains. Commands retain the original
observation-before-intent behavior by consuming the finite snapshot of events
already in the inbox, without a loop that chases new producers.

Refit retries carry operation and native-generation identity, and ownership
changes invalidate them. Completion wakes publish presentation without consuming
a future retry early. Fullscreen counters are opt-in through the smoke or
`HUTERM_FULLSCREEN_STATS`. The forced adapter failure is available only when the
fullscreen smoke seam is enabled, including the explicit idle fallback arm.
Visible CPU fell 44–55% and hidden CPU 96–98% against the fresh baseline; absolute
CPU and wakeup values are in the performance report. Forced-fallback cost, latency
comparison, and the broad handoff gates remain pending.

## Acceptance evidence

| Failure or requirement | Evidence required |
| --- | --- |
| Lost wake during drain-to-wait handoff | Controlled task test sends work before waiting and during reconciliation, including a completion emitted by deferred native-exit/display work; all events progress without a later frame or producer. |
| Queued toggle stalls after a Did notification | Controlled task test proves `defer_next` schedules a later executor turn, with no other producer or frame; native rapid-toggle smoke proves the second effect is accepted. |
| Observer re-enters native state during geometry effects | Native non-native entry/exit and display refit exercise synchronous bounds callbacks; assert callbacks signal only, with no inline adapter reads or nested reconciliation. |
| Display refit stalls or busy-loops | Inject repeated mid-refit changes and a fresh screen notification during a pending retry; require bounded retry deadlines and successful completion without a fresh notification; verify successful unchanged-geometry completion refreshes safe-area state. |
| Deadline lost or stale timer mutates a newer operation | Inject time for completion before expiry, earlier/later rearm, expiry without frames, rapid retoggle, and close before firing; verify controller state and absence of stale effects. |
| Hidden polling or redraw loop | After observed task completion and quiescence, prove no fullscreen timer remains on the fully event-driven path and no unsolicited reconciliation/redraw occurs; classify any compatibility sampler separately. |
| External transitions missed | Native green-button entry/exit and Linux WM-driven state changes, including unchanged bounds and property/bounds ordering, with the legacy pump disabled; repeated unrelated X11 properties must not wake fullscreen work. Check prompt chrome changes after macOS Will events without frames. |
| Transitions stuck or restore geometry corrupted | Existing fullscreen smoke covers rapid toggles, native/non-native modes, timeout/ignored EWMH, display refit, focus/input, retained tabs, and restore bounds. Preserve status errors and operation generation guards. |
| Quake or lifecycle work regresses | Quake show/hide, attach/detach conversion and profile ownership, close during native/non-native transition, multiple-window leases, Quit capture, and adapter event discard with frames stopped where applicable. |
| Degraded fallback accidentally starts at shutdown or cannot be tested | Smoke-only adapter construction skip; external native toggle is observed without inline command reconciliation, close/approved Quit permanently disarms sampling, and taking an installed adapter never enables fallback. |
| Layout depends on a paint | Inactive-tab geometry and presentation-query smoke; config reload, scale/bounds changes, and safe-area updates reach PTYs without a frame. |
| Frame pacing or latency regresses | `bench:output-latency` echo/flood, `bench:scroll`, and native refresh pacing on available 60/120 Hz displays; retain one in-flight snapshot and bounded queue invariants. |
| Idle cost shifts elsewhere | Paired visible/hidden one/fifty-tab and two-window CPU/wakeup samples, memory and thread counts; record the conditional pointer probe and any compatibility sampler separately. |

Use `mise tasks` to confirm exact invocations. Relevant tasks currently include
`smoke:macos-fullscreen`, `smoke:linux-fullscreen`, `smoke:macos-refresh`,
`smoke:macos-quake`, `smoke:linux-quake`, `smoke:macos-quit`, and
`smoke:presentation-queries`. Use the native macOS host and Linux Docker/Xvfb
with a WM where frames are
required. Do not create or use development VMs for this implementation; retain the
existing no-WM ignored-EWMH fixture. Run `mise run verify` before delivery.

Tests synchronize on observed events, acknowledgements, state changes, and task
completion. After each migrated producer, wait for settled state through the
read-only smoke state probe before issuing another fullscreen command: commands
observe native state inline and can conceal a missing wake. Use opt-in task-pass
and armed-timer counters for scheduling absence, not process wakeup measurements
from a smoke that polls state every 10 ms. Inject clocks for controller
deadlines. A fixed sleep is not evidence of readiness. For absence assertions,
establish that the task ran before checking quiescence. Prefer an intended red
assertion for a new lost-wake regression; existing green suites alone do not
prove the pump is dispensable.

Report CPU durations accurately: benchmark `Instant` timings include scheduling
and are elapsed time. CPU paint markers are not physical presentation latency.
Expect any wakeup benefit to be clearer while hidden; do not assert that
removing a 16 ms timer necessarily subtracts exactly 60 OS wakeups per second.
No minimum percentage improvement is promised before the checkpoint.

## Unresolved questions

- Is one later foreground turn sufficient for AppKit after each native Did event
  on supported macOS versions? Resolve with the pump-disabled rapid-toggle
  smoke; add a scoped settle deadline only if native evidence requires it.
- Which physical display migration/notch cases are available during validation?
  Report unavailable cases without counting VM coverage as physical evidence.
- How much idle CPU/wakeup reduction remains after removing this timer, versus
  GPUI display-link cost? Resolve through paired measurements; renderer/GPU and
  memory optimization remain separate follow-up work.
