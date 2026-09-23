# Event-driven window refresh

Status: steps 1 to 3 and the Unix runtime wait conversion shipped in PR #147.
Step 4, including animation scheduling and scrollbar cleanup, shipped in
PR #153 as `42117b6`. Steps 5 and 6 shipped in PR #155. Steps 7 and 8 are
implemented in PR #157, with fullscreen notification scheduling and removal of
the per-window pump. Paired measurements and local validation are complete.

The 2026-09-19 comparison rebuilt the baseline at `94f8028` under the current
single-display, scale-1 setup. At 120 Hz, default flood snapshots rose from 60
to 120 per second. With 50 idle tabs, visible CPU fell from 124.6% to 2.2% of
one logical core, and interrupt wakeups fell from about 59,700/s to 180/s.
See the [post-refresh measurements](../performance/gpui-terminal-renderer.md#post-refresh-measurements-2026-09-19)
for methods, latency distributions, tradeoffs, and remaining coverage limits.

The 2026-09-23 fullscreen comparison accepted 72 samples from baseline `f3ec40e`
and feature `1d589d1`, including an old-pump-enabled control. Visible CPU fell
44–55% and hidden CPU 96–98%; RSS and thread counts were essentially unchanged.
The legacy loop is now removed. The display link still accounts for most visible
wakeups. See the [fullscreen comparison](../performance/gpui-terminal-renderer.md#fullscreen-three-arm-comparison-2026-09-23)
for absolute values and coverage limits.

This plan is written for an agent continuing the work on macOS, which is the
primary Huterm platform and the only one here with a real display. Read
[AGENTS.md](../../AGENTS.md) first. The measurements it relies on are described
in [the renderer performance notes](../performance/gpui-terminal-renderer.md).

The [2026-09-20 hardening follow-up](../performance/gpui-terminal-renderer.md#hardening-follow-up-2026-09-20)
verified native 60 Hz pacing and found no concrete correctness defect in an
independent source review. Longer echo runs retain a small latency increase;
memory probing traced most of the increase to per-thread TLS allocation,
dominated by Zig's 256 KiB signal-stack buffer. Retain the blocking waiter;
changing native signal-stack policy needs separate validation.

## Outcome

The goal is lower visual latency, CPU work, allocation pressure, and resident
memory, with consistent presentation as the display cadence changes. Delivered
frames govern presentation opportunities; 60 Hz and 120 Hz are validation
cases, not scheduling constants. Runtime I/O and lifecycle work must continue
without display frames. Completing the numbered steps is a means to those
outcomes, not a performance result by itself.

The fresh `42117b6` checkpoint is recorded in the
[2026-09-23 performance notes](../performance/gpui-terminal-renderer.md#pointer-and-pending-work-checkpoint-2026-09-23).
Steps 5 and 6 target remaining coordination work. Reassess active-frame encoding
and memory costs before choosing between renderer changes and steps 7 and 8.
Keep this implementation separate from changes to glyph encoding, snapshot
representation, fullscreen ownership, or GPUI's display-link policy.

First remove redundant snapshots and centralize refresh scheduling. Complete
removal of the window's 16 ms pump is a later, measurement-dependent step.
Work happens when something changes: a
terminal publishes events, an animation is running, a native notification
arrives, or the pointer moves. A configuration option decides whether terminal
output-driven snapshots are clamped to the display's refresh rate, with a
bounded exception for pending viewport changes.

The planned stages target four costs that remain after the groundwork:

- A permanent 60 Hz timer per window, which also refreshes every hidden tab and
  clones each tab's title twice per tick.
- One redundant snapshot and repaint after every isolated update, and a
  needless snapshot after every title, metadata, or bell event on the visible
  tab. Both are described under "Where main stands".
- Up to 16 ms before a title change, bell, or shell exit reaches the tab bar.
- Recurring 2 ms waits in the runtime, writer, and PTY reader, including hidden
  and idle terminals. Measure actual wakeups rather than treating the runtime
  thread's nominal 500 polls per second as the terminal's total.

Non-goals: changing the renderer's paint path, changing the protocol crate, and
the deferred renderer items listed under "Deferred renderer work".

This work improves scheduling overhead, output latency, and idle cost. It does
not directly reduce the cost of painting a genuinely changed frame. Shared
attachments and split panes remain future work; the scheduling boundary must
allow them without putting desktop frame policy into core.

## Baseline architecture before this implementation

The groundwork merged in #141 on 2026-09-18 as squash commit `776e567`. The
hashes below are its pre-squash branch commits and are not on `main`:

| Commit | Change |
| --- | --- |
| `850d2b5` | `bench:renderer-scenarios`, the output latency probe, Docker support |
| `e4cc474` | Glyph layout cache rotates only when full; ASCII cells skip hashing |
| `fbe3aff` | Scenario benchmark prepares one step per frame |
| `6284221` | One grid background quad, mask only built-in paths, slot-indexed built-in geometry |
| `6b006c9` | Scenario benchmark finishes sampling early in the process |
| `4072d88` | Snapshot on runtime activity; snapshot requests wake the runtime |

`ci:benchmarks` runs `bench:output-latency` in `echo` mode with a 5 ms
applied-median budget. It is the only automated check of the activity path.

Then #139 added terminal metadata and the visual bell, which widened the pump's
per-tab work. See "What #139 added to the pump".

### What `4072d88` does

- `huterm-core` publishes events through `EventPublisher`, which also sets a
  one-slot `async_channel` signal. `RuntimeClient::wait_for_activity()` awaits
  it. The signal carries no data and coalesces. **Clones of a client share one
  signal, so it supports one waiter.** A second waiter would take wakes from the
  first.
- A snapshot request sends `RuntimeMessage::Wake`, because the runtime thread
  blocks in `messages.recv_timeout(2 ms)` and reads the control channel only
  between those waits.
- Each `TerminalView` spawns a task in `TerminalView::new` that awaits the
  signal and calls `snapshot_on_activity`. That starts a snapshot when the view
  is visible and `starts_snapshot_on_activity` allows it: the last snapshot
  started at least `ACTIVITY_SNAPSHOT_INTERVAL` (8 ms) earlier.
- The window refresh pump is unchanged and remains the only consumer of
  terminal events.

Measured on Linux x86_64 with `mise run bench:output-latency`, output to applied
snapshot, median with maximum in parentheses:

| Configuration | `echo` | `flood` | `flood` snapshots per second |
| --- | --- | --- | --- |
| Pump only | 10.4 ms (17.1) | 16.2 ms (32.4) | 60 |
| Activity snapshot and runtime wake | 0.4 ms (6.3) | 12.3 ms (46.6) | 80 |

On macOS with the runtime wake, `echo` applies in 126 µs (1.3 ms) and paints
in 5.1 ms (8.9), and `flood` holds 61 snapshots per second: the pump paces
sustained output at 60 Hz regardless of the display's refresh rate.

### The redundant snapshots

After an activity snapshot, the pump's next tick drains the queued `Invalidated`
event, calls `scroll.invalidate()`, and starts a second snapshot. It reuses every
row `Arc`, so preparation rebuilds nothing, but the view still repaints.

`snapshot_on_activity` also calls `scroll.invalidate()` unconditionally. The
activity signal fires for every published event, including `TitleChanged`,
`MetadataChanged`, and `Bell`, so each of those starts a snapshot and repaint
of the visible tab even though its content did not change.

Do not skip that invalidation by comparing the event's generation with the
applied snapshot's. Presentation updates, such as theme colors, invalidate
without advancing the content generation, because selections are tied to it.
The skip would leave stale colors on screen. The fix for both is a single
consumer of events that invalidates only on `Invalidated`, `Ready`, `Exited`,
and `Failed`, as `refresh` does today. That is step 2.

### What #139 added to the pump

- `refresh` now handles `Bell`: it flashes a visible tab in an active window
  and marks any other tab's bell unseen. It also applies `MetadataChanged`
  events by revision.
- The bell flash is a timed animation advanced by `bell.advance` in `refresh`.
- The pump's per-tab comparison covers title, exit, failure,
  `metadata_revision`, and `bell.unseen`. It still clones the title twice per
  tab per tick.
- A separate 1 Hz task, `start_process_metadata_sampler`, runs `/bin/ps` on a
  background worker when `tabs.label` is `process` or `process_and_directory`.
  The default, `title`, leaves it idle. It is outside this plan, but it is an
  idle cost to include in `powermetrics` runs that use those labels.

## First tasks on macOS

Performance baselines were refreshed on 2026-09-19 at `ee91a54`, before product
code changes: five runs per renderer scenario, three echo and three flood runs,
and the scroll benchmark. All passed. See the
[dated baseline](../performance/gpui-terminal-renderer.md#pre-refresh-change-baseline-2026-09-19)
for results, host/display differences, and local comparison artifacts. This
refresh did not repeat the older test, smoke, or manual visual checks below.

The same benchmark set was then repeated with explicit built-in-display
selection and frame diagnostics. Echo and flood delivered approximately 120
callbacks per second while flood still applied 60 snapshots per second. Use
the [built-in baseline](../performance/gpui-terminal-renderer.md#built-in-display-baseline-2026-09-19)
for the original scale-2 measurements. The final comparison below uses a fresh
baseline at scale 1 because the display topology changed. The targeting and
diagnostic harness was committed before the refresh implementation.

This is step 1. These ran on 2026-09-17 on an M3 Max. `check`, `test`, and the renderer,
input, fullscreen, palette, and quit smokes pass. The quake smoke fails at its
external-grab step whenever another Huterm build on the host holds a global
shortcut, such as a parallel smoke session; every earlier phase passes. #141
reports the visual pass in item 3. The feel comparison in item 6 has not been
recorded; do it against the step 3 build instead, where the frame clock changes
the feel.

1. Run `mise run check` and `mise run test`. On Linux both pass at `4072d88`.
2. Run the macOS smokes: `smoke:renderer`, `smoke:macos-input`,
   `smoke:macos-fullscreen`, `smoke:macos-quake`, `smoke:macos-palette`,
   `smoke:macos-quit`. On Linux, `smoke:renderer` and `smoke:linux-input`
   pass.
3. Look at built-in graphics in a real session. `6284221` changed how
   backgrounds and built-in glyphs paint: check box drawing, block elements,
   powerline separators, a program with colored backgrounds such as `htop`, and
   a theme reload. `HUTERM_RENDERER_HOLD=1 target/debug/examples/renderer_smoke`
   holds the fixture open.
4. Record `mise run bench:output-latency` and
   `mise run bench:output-latency -- flood`. This is the first measurement of
   `painted_us` on a real display. Under Xvfb it only reflects GPUI's 60 Hz
   timer.
5. Record `mise run bench:renderer-scenarios -- --output
   target/bench/macos-baseline.json`. Jim's run at `6b006c9` is the reference:
   prepare 133 to 200 µs for full rebuilds, paint about 500 µs for text, 257 µs
   for `blocks`, 965 µs for `boxes`, zero cache misses.
6. Type in a shell and in an editor, and run
   [DOOM-fire-zig](https://github.com/const-void/DOOM-fire-zig), to compare the
   feel with a build of `main`.

### Measurement hazards found so far

- **macOS slows a lightly loaded process after about two seconds.** The same
  paint cost 2.2 to 2.7 times more once the benchmark process was about two
  seconds old. `bench:renderer-scenarios` finishes sampling within about 1.5
  seconds for this reason, and prints `elapsed_ms`. Treat changes above about 5%
  as real on macOS and above about 10% on the Linux host. A terminal in
  ordinary use runs in the slower state, so real paint cost is closer to 1.2 ms
  than 0.5 ms.
- **Glyph layout cache misses cost very different amounts per host.** The same
  426 misses cost 34 ms on a Linux desktop and 0.7 ms in the Docker image, which
  installs only DejaVu.
- **GPUI keeps line layouts for its current and previous frame.** Work repeated
  inside one frame hits that cache and hides shaping cost.
- **Reports are comparable only within one version of a benchmark.**
- **Continuous frames hold the main thread.** With `HUTERM_RENDER_STATS=1`,
  which requests a frame per display tick, the main thread sits in Metal present
  until vsync, so the activity wake waits for the frame and `echo` reported
  12.7 ms instead of 126 µs. `bench:output-latency` uses
  `HUTERM_RENDER_STATS=events` for this reason. Any window that draws
  continuously, such as `flood` or DOOM-fire, has the same main-thread
  behaviour in ordinary use; unclamped mode in step 3 must account for it.
- **An occluded macOS window gets no frames.** GPUI starts its display link
  only while `occlusionState` reports the window visible, and stops it on the
  occlusion callback. A locked screen or a covering window stops every
  frame-driven benchmark and every `on_next_frame` callback.

## What the pump does today

The pump is the task spawned in `open_window_with_profile` in
`crates/huterm-gpui/src/desktop/windows.rs`, a loop around
`timer(Duration::from_millis(16))`. Each tick does the following.

| Duty | Code | Why it polls | Event-driven source |
| --- | --- | --- | --- |
| Reconcile fullscreen | `refresh_fullscreen`, `observe_fullscreen`, `advance_fullscreen` | Native observers queue events; deadlines and settling need time | The native notification callback, plus a timer only while an operation or deadline is pending |
| Tab overlay reveal | `refresh_tab_visibility` | Reads `window.mouse_position()` and hover each tick, advances `Reveal` | Mouse move, hover, activation, and modifier events; animation clock while `Reveal` is between 0 and 1 |
| Tab strip scrolling | `advance_tab_scroll` | Eases toward `scroll_target`; edge autoscroll during tab reorder | Animation clock while a target or drag exists |
| Tab scrollbar fade | `tab_scrollbars.advance` | Hold and fade timing | Animation clock while visible |
| Terminal events | `TerminalView::refresh` per tab | `try_recv_event` is the only way events are read, including `Bell` and `MetadataChanged` | `wait_for_activity` per terminal |
| Host effects | start of `TerminalView::refresh` | Clipboard writes queue in `HostEffectSink`, which does not go through `EventPublisher` | Make the sink set the activity signal when it queues an effect |
| Terminal scrollbar and resize indicator | `scrollbars.advance`, `resize_visibility.update` in `refresh` | Hold and fade timing | Animation clock while visible |
| Bell flash | `bell.advance` in `refresh` | Flash deadline | Animation clock while flashing |
| Retry queued input, resize, presentation | `retry_client_messages` | `RuntimeError::Busy` when the message queue is full | Retry on the next activity wake and on a short timer while anything is queued |
| Scroll benchmark | `benchmark.drive` in `refresh` | Injects scroll input on a cadence | Its own timer, only when `HUTERM_SCROLL_BENCH` is set |
| Tab metadata propagation | The comparison of title, exit, failure, `metadata_revision`, and `bell.unseen` around `refresh` | Detects changes made by the drain | Direct result of the single event consumer |
| Queued closes | `resume_close` | Runs the next close once `busy` clears | Call where `busy` clears and where an exit is observed |
| Palette availability | `refresh_palette` | Compares a state tuple each tick | Call from the mutations that change that state |
| Palette scrollbar | `palette.advance` | Fade timing | Animation clock while visible |

Quake windows already use a loop that stops when idle (`keep_running` in
`crates/huterm-gpui/src/desktop/quake_windows.rs`). It is a working model for
"timer only while something is in motion".

## Design

### Scheduling ownership

Introduce a small internal scheduling component in `huterm-gpui`, owned by the
window. Keep scheduling decisions testable with injected time, frame ticks,
visibility, and completion events; GPUI callbacks execute those decisions.
Avoid a general task framework or a new protocol abstraction.

| Owner | Responsibility |
| --- | --- |
| Core runtime | Canonical terminal state, ordered controls, event publication, and wake signalling |
| Window scheduler | Bounded drain continuations, frame admission, pending-work deadlines, and cancellation |
| Terminal view | Apply terminal events and snapshots; retain scroll, selection, link, and presentation state |
| Workspace view | Apply tab metadata and exit transitions; coordinate close and palette state |
| Renderer | Prepare and paint immutable snapshots with client presentation state |

Callers report work rather than independently spawning timers or bypassing
snapshot admission. Keep event consumption, snapshot requests, and presentation
updates distinct: event consumption must progress even when drawing cannot.
Return a compact refresh result describing metadata changes, exit transitions,
and remaining work instead of comparing growing tuples or cloning titles.

The scheduler identifies terminal views within the current attachment. Today
there is one terminal per tab; do not encode that as a scheduler invariant.
Future split panes can have several visible terminal views sharing one window
frame clock, with separate in-flight snapshot state.

Cloned `RuntimeClient`s share both the destructive event receiver and activity
signal. They are not independent subscriptions. This change retains one consumer
per runtime stream. Before shared attachments are implemented, add a subscription
or fan-out boundary so multiple windows cannot steal each other's events. Do not
build that facility in this change or claim the new scheduler supplies it.

### One consumer of terminal events, at the window

Move the per-tab body of the pump into `WorkspaceView::refresh_tab(tab_id)`:
drain terminal events and host effects, apply the refresh result, feed
`exited_tabs`, notify on a change, then call `resume_close`. `WorkspaceView`
starts one activity task per terminal stream when its tab is created. The task
awaits `wait_for_activity` and schedules a bounded drain.

Remove the task in `TerminalView::new` in the same change. The signal supports
one waiter.

Make successful `HostEffectSink` enqueue signal activity after publishing the
effect. Preserve recipient selection, generation checks, and admission budgets.
Once the
pump is no longer a fallback, clipboard writes would otherwise wait for the
next terminal event.

Only events that invalidate terminal state mark a snapshot dirty; a wake itself
does not. The scheduler then considers snapshot admission independently.

Treat the activity signal as a hint to inspect pending work, not as a count of
events. Keep the current per-turn budgets of 64 events and 8 host effects. If a
drain exhausts either budget, retain a continuation and yield before another
bounded turn, even if no producer sends again. A conservative extra empty drain
is acceptable. Return to waiting only after queued work is exhausted, with the
enqueue-before-signal ordering covering arrivals during that transition. On
channel closure, finish any queued work before retiring the consumer.

Pacing must not turn the event queue into a backlog. It is currently unbounded;
only invalidations are coalesced. A 100 ms timer with a 64-event batch is not an
exit-latency bound when titles or bells flood. Before shipping step 2, choose and
test an event admission/coalescing policy that bounds replaceable state and bell
backlogs while retaining lifecycle transitions. Preserve relevant ordering and
the documented visual-bell behavior. Never block the parser waiting for a UI
drain. This can remain inside core without changing protocol event types.

Under ordinary load, consume the first activity promptly. Under sustained load,
use bounded, yielding continuations and coalescing rather than one UI callback
per PTY chunk. Snapshot pacing must not be the only permission to consume exit,
failure, metadata, or host-effect work. A hidden-tab presentation cadence of up
to 100 ms is acceptable for labels and unseen-bell markers, but must not delay
draining lifecycle events behind that presentation work.

Tab visibility is not window visibility. An occluded or minimized window still
needs event consumption, close-on-exit, and retries without a frame clock. Keep
those paths independent of `on_next_frame`. Bound pending frame registration to
one per window and reschedule dirty visible views when frames resume. Resolve
how frame availability is observed in step 3; inactivity alone must not create
a permanent polling loop.

### Task and attachment lifetime

Store activity tasks in their owning tab/attachment state and cancel them on
removal or retargeting. Waiting for a failed view update is insufficient: an idle
detached runtime may survive forever without another wake. Cancellation must
release the client clone without closing the terminal.

Frame callbacks and timers use weak owners and validate attachment/request
identity before acting. Retargeting invalidates old callbacks and snapshot
results. Repeated wakes share a pending continuation rather than accumulating
tasks. Config reload and tab activation reevaluate pending work immediately.

### Frame pacing and the refresh clamp

GPUI calls its frame callback on every display-link tick while the window is
visible, and runs `Window::on_next_frame` callbacks at the start of that tick,
before drawing. That is a frame clock that needs no knowledge of the refresh
rate, so it follows the actual delivered display ticks, including ProMotion
changes. The nominal display specification is not a measured frame rate.

Centralize admission immediately before starting any snapshot, including calls
from refresh, input/scroll, link lookup, initial display, and snapshot completion.
Keep the existing one-in-flight request and coalesced pending scroll behavior.
Do not clear dirtiness that arrives after a request starts when that request
completes. Presentation invalidation remains independent of content generation.

Clamped mode, the default:

- Dirty visible views may start immediately when no request is in flight and
  their frame allowance is available. Restore allowance on an observed frame
  tick, not elapsed wall time. Preserve an immediate first request after idle
  without continuously registering callbacks solely to count idle ticks.
- Otherwise retain pending work and use the window's single `on_next_frame`
  registration to reconsider admission. This replaces
  `ACTIVITY_SNAPSHOT_INTERVAL` and applies to completion-triggered requests too.
- Output and link requests share one allowance. Pending viewport changes may use
  one additional allowance per delivered frame. Consume normal allowance first;
  the extra allowance cannot admit output or link work alone. Both allowances
  replenish only on an observed frame, including after reload. Keep one request
  in flight and coalesce pending scroll, so stopped frames permit only a finite
  remaining burst.
- This viewport exception avoids holding scroll behind an output snapshot while
  GPUI presents a frame. Same-runner Linux comparisons exposed an additional
  frame of latency under the original shared allowance. Returning to live output
  also counts as a viewport change. The extra snapshot includes current terminal
  output; this is a bound of two total requests per frame during viewport work,
  not two independent output and scroll streams.

A callback runs before drawing, but the snapshot completes asynchronously.
Starting it there does not guarantee its result appears in that draw. Measure
both applied and painted latency, including isolated output after idle.

Unclamped mode:

- A dirty visible view starts as soon as its previous request completes and
  pending events have been handled. It still has at most one request in flight.
  This may reduce the age of presented output; it does not promise more visible
  frames or bypass main-thread waits in Metal presentation.
- Snapshots are built on the runtime thread, which also parses PTY output.
  Measure `flood` throughput before choosing whether unclamped mode needs a
  floor between snapshots.

Configuration: add `terminal.refresh = "display" | "unlimited"` to
`TerminalConfig` in `huterm-config`, defaulting to `display`. An enum rather
than a boolean leaves room for a fixed cap later. Regenerate schemas
with `mise run schema:generate`, extend `schemas/fixtures.json`, and apply the
value on config reload like `links`.

`on_next_frame` needs a `Window`. Use it from a window update, not from a bare
timer. The vendored GPUI implementation only queues the callback and does not
access `current_view`; `request_animation_frame` does. Cover the intended
window-update call path in integration verification and preserve GPUI's window
borrow rules.

The 16 ms pump caps sustained output at 60 snapshots per second on macOS even
when the display refreshes faster. The frame clock removes that cap.

### Animation clock

Each component must report two independent facts: whether presentation changed,
and when it next needs evaluation. Use an internal result equivalent to a redraw
flag plus `Idle`, `NextFrame`, or `At(deadline)`. Existing `advance` booleans
report only change: a bell returns false before expiry, and a scrollbar returns
false during its hold period. Neither means scheduling can stop.

Aggregate requests into the window's single frame callback and one timer for the
earliest deadline. Keep both when different components require them. Holds and
the bell's static flash need deadline wakes, not redraws on every frame; fades,
easing, and drag autoscroll use frame ticks. Notify only on presentation changes.
Starting or extending an animation must update its scheduling request.

When frames stop, retain animation state and evaluate elapsed-time deadlines;
do not accumulate callbacks or keep retrying visual-only frame work. On resume,
reconcile against current time so expired flashes and holds cannot remain stuck.

Headless Xvfb delivers frames only with a window manager. Every Linux smoke and
benchmark that depends on animation progress must keep working; check how each
starts its X session before removing its timer-driven progress. The fullscreen
smoke uses unlimited snapshots only for its intentional no-WM ignored-EWMH
fixture; the Openbox fixture retains default display pacing.

### Pointer-driven reveal

`refresh_tab_visibility` decides whether the pointer is at the tab edge.
Compute that from mouse-move and hover events instead. Register movement
through `Window::on_mouse_event` during paint, as the terminal canvas does,
because element `on_mouse_move` filters by hover. Two details need care:

- GPUI's macOS hover flag reports activation and can keep the last position
  after the pointer leaves. The existing code corrects this with
  `native_fullscreen::Adapter::pointer_on_display`. Leaving for another display
  produces no move event in this window, so keep a slow check while the overlay
  is revealed.
- A terminal gesture keeps move and release ownership beneath the overlay until
  the next refresh hides it.

## Runtime thread polling

At the baseline, each terminal's runtime thread in `huterm-core` loops on
`messages.recv_timeout(RUNTIME_POLL_INTERVAL)`, which is 2 ms. Control requests
arrive on a separate `std::sync::mpsc` channel that the thread cannot block on
together with `messages`, so it polls. `RuntimeMessage::Wake` exists only to
cut that wait short for snapshot requests. The writer thread has its own 2 ms
`recv_timeout` loop. The PTY reader also calls
`reader_waiter.wait(RUNTIME_POLL_INTERVAL)` after `WouldBlock`, so its readiness
wait times out every 2 ms while idle.

These are three separate recurring wait sources per live terminal, including
hidden tabs. Their nominal rates do not establish the OS's measured process
wakeup rate. The window pump also wakes every 16 ms, and GPUI's display link
wakes on every vsync while a window is visible. Runtime I/O is therefore a likely
major source to measure first. It is contained in `huterm-core` and does
not touch the fullscreen, quake, or overlay invariants that steps 4 to 8 carry.
Keep its changes in separate commits from steps 2 and 3.

The loop turnover currently does more than read controls. Each of these needs a
replacement wake source before the timeout can go:

| Duty | Code | Replacement to evaluate |
| --- | --- | --- |
| Read control requests | `controls.try_recv` after each message or timeout | One channel for messages and controls, or a select over both |
| Detect child exit | `observe_child_exit` calls `child.try_wait()` | A waiter that sends a message on exit; keep the post-close bounded reap described in AGENTS.md |
| Track foreground process groups | `pty::record_foreground_group` | Sample on output, input, and control requests; close consent needs fresh groups |
| Observe `closing` | `while !closing.load(..)` | A message sent wherever `closing` is set outside the loop |
| Retry a full writer queue | `WriterQueueState::Full` sleeps 2 ms | A writer acknowledgement when its queue drains |
| Writer thread | `recv_timeout` in the writer loop | Block on `recv`; wake it explicitly on close and input closure |
| PTY reader readiness | `ReadinessWaiter::wait(2 ms)` after `WouldBlock` | Interruptible readiness wait covering both PTY readability and explicit shutdown; preserve the macOS-safe `filedescriptor` path |
| Reader queue pressure and writer readiness | Reader retries a full message queue; writer sleeps on `WouldBlock` | Capacity/readiness notification with a shutdown escape; distinguish idle waits from active backpressure |

The runtime checkpoint keeps bounded data and separate priority controls, with a
coalesced condition-variable wake shared by their senders. Controls drain in
bounded batches before data gets a turn. Writer dequeue notifies queue capacity;
reader admission blocks until capacity or receiver closure. Unix reader and
writer readiness include a separate cancellation descriptor, signaled before
joining their workers. Idle writer input blocks until data or channel closure.
Non-Unix paths retain their existing polling fallback.

A dedicated thread owns the physical child and blocks in `wait`. The runtime
reads cached status through a child proxy and wakes when reaping finishes. No
shared lock spans the physical wait. Thread creation failure returns the original
child for cleanup; interrupted and failed waits retain ownership until reaped.
The existing bounded shutdown and deferred reaper operate through that proxy.
This adds one sleeping thread per terminal, which must be included in memory and
thread-count measurements.

Tests cover cancellation before and after worker startup, idle writer closure,
notification handoff, waiter creation failure, and repeated wait errors. Existing
PTY and close-assessment tests cover silent exits, saturated queues, retained
history, foreground changes, shutdown escalation, and descriptor cleanup.

Foreground sampling now runs on runtime work and again at live teardown. A
close-assessment control wakes the runtime even when the foreground job is
silent. Native Quit and close smokes remain required before release.

Settle child ownership and shutdown ordering before changing the remaining
loops. A
blocking child waiter must not hold a lock needed to signal, close descriptors,
or finish bounded teardown. Preserve immediate root-exit observation, final
output parsing, retained history, and deferred reaping after cleanup timeout.
Do not join a waiter before performing the actions that can let it finish.

Preserve bounded PTY/input admission and explicit fairness between controls and
output. A single unbounded queue would trade idle polling for memory growth;
unlimited control draining can starve output parsing. Every sender of controls,
worker failures, exit, input closure, and shutdown must reach the new wait.
Shutdown signalling must still work when data queues are full. Test notification
arrival both before and during wait registration to expose lost-wake races.

Foreground-group sampling on I/O is a candidate, not an established replacement
for polling: a silent job can change foreground ownership. Prove fresh close
assessment and cleanup evidence without depending on unrelated terminal output.

Evidence: wakeups per second for an idle window with several tabs, measured
with `powermetrics` before and after; `bench:output-latency` in both modes;
`bench:engine`; the shutdown and PTY integration tests, which must still prove
that live children and blocked I/O workers terminate; and the quit and close
smokes, which exercise close consent.

## Steps

The original sequence follows. Steps 1 to 3 and the runtime prerequisite shipped
in PR #147. Steps 4 to 6 shipped in PRs #153 and #155; steps 7 and 8 are
implemented in PR #157 after the three-arm comparison. Native 60 Hz pacing was
verified in the
2026-09-20 hardening follow-up. The subjective
editor/DOOM feel comparison remains unverified. Convert one duty at a time
with the pump still running so changes remain bisectable. Remove the timer last.

1. **Record macOS baselines.** Done; see "First tasks on macOS" above.
2. **Single event consumer and scheduler.** Implemented in `dae1ccb`.
   Add `refresh_tab`, compact refresh results, owned activity tasks, and bounded
   drain continuations; remove the
   task in `TerminalView::new`. Implement and test event backlog policy before
   slowing consumers. The pump routes through the same scheduler as a temporary
   fallback, without bypassing budgets. Success: `echo` stays near its current median,
   the redundant snapshot is gone, title, metadata, and bell events no longer
   start snapshots, and title, bell, exit, and close-on-exit still work for
   active and hidden tabs. More than one batch of events or host effects drains
   without a new producer wake. Idle task cancellation releases its client.
3. **Frame pacing and the clamp option.** Implemented in `ef6e859`.
   Replace the 8 ms rule with the frame clock at the common snapshot admission
   point and add the configuration option.
   Resolve frame availability and resume handling. Success: clamped requests do
   not exceed one per terminal view per observed frame interval under flood;
   verify on 60 Hz and 120 Hz displays and record delivered ticks. Preserve
   leading-edge latency after idle, scroll/link responsiveness, and progress
   while occluded. Check reload and completion cannot bypass admission.
4. **Animation clock.** Implemented in the follow-up. Terminal scrollbars,
   the resize indicator, visual bell, tab scrolling, tab scrollbars, palette
   scrollbar, and `Reveal` share the window frame clock and one earliest-deadline
   timer. Notifications and render-time layout changes arm work; weak targets and
   release cleanup keep scheduling separate from view ownership. Each component
   reports pending frame work and deadlines independently of redraw changes.
   The pump no longer advances these animations. Steps 5 and 6 below remove
   pointer reveal, retries, palette availability, and close work from it.
   Controlled-time tests cover holds, extension, fade completion, settled hover,
   and reveal's hold-to-fade boundary. The native refresh smoke covers animation
   cadence, idle completion, bell expiry while frames stop, and stale callbacks.
   Physical display runs passed at 60 Hz and 120 Hz on 2026-09-22.
   Review fixes renew expansion holds after settled drags and reuse earlier
   deadline timers when holds extend. Repeat scroll comparisons on 2026-09-23
   passed all budgets without reproducing a consistent branch-specific penalty.
   See the renderer performance report for the earlier slower pairs, repeated
   measurements, and attribution limits.
5. **Pointer-driven reveal.** Implemented. Capture listeners coalesce mouse move,
   press, release, exit, and modifier changes into a deferred reconciliation after
   gesture ownership settles. Activation, bounds, notifications, and rendering
   cover layout changes. A 100 ms macOS fallback remains only for a revealed
   overlay or the native fullscreen top edge outside the content view. It shares
   the animation deadline timer and does not request idle redraws.
6. **Call-site triggers.** Implemented. Workspace notifications reconcile
   `resume_close` and `refresh_palette`; async completion paths notify explicitly.
   Each terminal owns a cancellable task and bounded wake channel for queued
   input, resize, and presentation updates. Admission signals
   work without requiring a redraw. Activity delivery also retries; a 16 ms
   backstop runs only while work remains. The opt-in scroll benchmark uses this
   task for its workload cadence. Production idle terminals arm no retry timer.
   Mouse motion now flushes on the next executor turn instead of waiting for the
   old 16 ms tick. Queued motion still coalesces and preserves bounded ordering,
   but this may send more motion reports to the PTY; that traffic is not measured.
   Config and window-bounds changes explicitly reconcile retained terminal
   geometry without waiting for a frame. The presentation-query smoke checks
   inactive-tab font and padding-only reloads against actual PTY replies.
   Native coverage queues input and controls while display frames are paused,
   observes a shell acknowledgement, and checks cancellation after view release.
7. **Fullscreen.** Implemented through a coalesced window-owned task, native
   notifications, the X11 property callback, and explicit operation/refit
   deadlines. See the [fullscreen plan](fullscreen-event-scheduling.md).
8. **Remove the pump timer after measurement.** Implemented after the accepted
   three-arm comparison and pump-off correctness checks. Opt-in counters remain,
   with no repairing diagnostic tick. Only macOS adapter construction failure
   retains the explicit compatibility sampler; its cost is measured separately.

Steps 2 and 3 deliver the latency and correctness benefits and ship together.
The runtime loop change ships separately, before steps 4 to 8. Steps 4 to 8
carry most of the risk. The earlier scope decision used per-process CPU and
interrupt-wakeup counters from `proc_pid_rusage` with a stubbed pump body.
`powermetrics` required unavailable sudo access. The later three-arm comparison
measured the timer itself and established the benefit of removing it, while
leaving display-link work outside this migration.

## Invariants to preserve

These come from AGENTS.md and from failures found while building the groundwork.

- Only visible terminal views start snapshots. Today `begin_visible_snapshot`
  enforces this for the active tab; future panes may have several visible views.
- Drain terminal events in bounded batches. `refresh` reads at most 64 events
  and 8 host effects per call. Budget exhaustion retains a scheduled continuation.
- Every snapshot path shares admission; at most one request is in flight per
  terminal view. New dirtiness survives an older request's completion.
- Shell-exit transitions queue once for assessed tab close, separately from the
  merged manual close intent.
- Never lock the Mux mutex from rendering, input, or these wake paths.
- `TerminalView` destruction only detaches. A task holding a `RuntimeClient`
  clone is explicitly cancelled when its owning tab/attachment goes away or is
  retargeted, even when the runtime survives silently. Stale callbacks and replies
  cannot act on the new attachment.
- Keep terminal focus and gesture ownership rules around overlays and tab drags.
- Exited tabs keep history, selection, and scrolling, so their views must still
  repaint on scroll and selection.

## Verification

| Risk | Evidence |
| --- | --- |
| Latency regresses | `bench:output-latency` in `echo` mode, before and after each step, on macOS; compare applied and painted distributions, including output after idle; `ci:benchmarks` enforces a 5 ms applied-median budget on Linux |
| Sustained output floods the runtime | `bench:output-latency -- flood` snapshot rate, and `bench:engine` throughput |
| Scroll coordination breaks | `bench:scroll`, which gates snapshot timing, wakeups, offsets, and queue bounds |
| A missed wake leaves the UI stale | Deterministic scheduler and integration tests with the fallback disabled; native input, clipboard, integration, fullscreen, quake, palette, and quit smokes |
| Hidden tabs miss exit or title changes | Hidden/occluded view tests including a title/bell flood followed by exit; assert queue bounds and lifecycle progress, then check native `close_on_exit` |
| Idle CPU does not improve | Instruments or `powermetrics` for 1, 10, and 50 idle tabs, with visible and occluded windows, before/after runtime changes and pump removal; record CPU and wakeups with process labels disabled, then measure their separate cost |
| Renderer regresses | `bench:renderer-scenarios -- --compare` against the macOS baseline |

Unit-test scheduler decisions with injected time and frame ticks. Add focused
integration tests for the real queues and task lifecycle. Required cases:

- Coalesced terminal events and all eight admissible host effects drain after
  one wake. A concurrent host-effect producer refills released capacity during
  a drain, then becomes silent. All admitted work finishes without another wake.
- An event arrives during drain-to-wait handoff, and the channel closes with
  queued work. Neither loses pending work or spins after completion.
- Invalidation arrives during an in-flight request; completion preserves it and
  schedules exactly the permitted follow-up. Theme changes with unchanged
  content generation still refresh colors.
- Activity, scroll, link lookup, config reload, and completion compete for frame
  admission. Verify one in-flight request and the clamped per-frame bound.
- Frames stop with a callback pending, lifecycle work continues, and frames
  resume. Callback count stays bounded and dirty visible views catch up.
- A tab is removed or retargeted while its task waits silently. The client clone
  is released; an old timer, callback, or reply cannot affect its replacement.
- Animation hold, fade, expiry, and deadline extension work across occlusion.
  A false redraw result does not erase a pending deadline.

Do not sleep to assume a wake occurred. Synchronize on observable progress or
poll a condition under a deadline. Prove migrated duties with their fallback
disabled throughout steps 2 to 8, not only when deleting the final timer.

Run `mise run verify` before handing each branch back.

## Deferred renderer work

Measured and set aside, in case a later profile changes the ranking:

- **Path tessellation for built-in glyphs.** About 1 µs of CPU per path per
  frame on Linux and 0.45 µs on macOS. Realistic screens have tens of paths.
  Caching the tessellation needs a vendored GPUI patch, because a built `Path`
  cannot be translated through the public API. The GPU cost of paths is not
  measured.
- **`paint_glyph` lookups.** About 70 ns per glyph on macOS, two locked hash
  lookups plus the scene push. It is the largest steady renderer cost and also
  needs a vendor patch.
- **Inline small string for `Cell.text`.** Every cell allocates a `String`,
  including blanks. `bench:engine` with the `full` fixture measures it. This is
  a protocol change.
- **Skip the repaint when a snapshot equals the displayed one.** `Arc<TerminalRow>`
  equality is cheap when rows are shared.
- **Smaller prepare items:** flatten single-glyph layouts, memoize `Hsla`
  conversion across runs of one color, reuse row vectors.

## Decisions and remaining coverage

1. Unlimited mode keeps one request in flight without an added timer floor.
   Measured flood throughput was 216 to 223 snapshots/s versus 120 in display
   mode. It is opt-in; the default follows observed frames.
2. Step 2 coalesces title, metadata, bell, and invalidation events by variant,
   retaining the first pending lifecycle event of each kind. Bounded drains
   and owned activity tasks preserve progress without frame delivery.
3. Step 3 uses one pending GPUI frame callback per window and weak terminal
   registrations. Event draining is independent of frames. Deterministic tests
   cover admission; native 120 Hz flood matches delivered callbacks. The 2026-09-20
   retry held the display-mode helper open and verified 60 callbacks and 60
   snapshots/s at 60 Hz, then restored 120 Hz.
   The focused native refresh smoke now covers occlusion and resume, coalesced
   callbacks across tab replacement, and idle activity-task cancellation while
   the core terminal remains alive. Run `mise run smoke:macos-refresh` in an
   unlocked macOS GUI session. The same test runs as the `macos-refresh` CI
   smoke step; physical 60/120 Hz cadence budgets remain explicit native checks.
   The 2026-09-21 scroll follow-up adds one bounded viewport allowance per
   delivered frame, preserving output-only pacing and one in-flight request.
   Controlled Linux and idle 120 Hz macOS comparisons recovered baseline scroll
   latency. Native coverage also verifies the extra allowance while frames are
   paused. See the [performance results](../performance/gpui-terminal-renderer.md#scroll-admission-follow-up-2026-09-21).
4. Unix runtime waits use explicit work notifications, blocking child wait,
   cancellable reader/writer readiness, and bounded queue admission. Non-Unix
   fallbacks remain. The child waiter adds one sleeping thread per terminal;
   measured resident memory increased about 18 MiB at 50 idle tabs, without
   isolating how much of that increase comes from the waiter in the first run.
   The follow-up traced 13.28 MiB for 50 added threads to the linked image's
   TLS blocks, dominated by Zig's signal-stack buffer.
5. Steps 4 to 6 follow delivered frames for animation, deadlines for holds,
   pointer events for reveal, and targeted wakes for pending terminal work. The
   fullscreen migration in steps 7 and 8 now removes the remaining timer after
   the accepted three-arm comparison. Local validation is complete; physical
   display migration and a fresh 60 Hz run remain untested.
6. Automated native checks pass. Physical IMEs, subjective typing/scroll feel,
   and long-running interactive workloads still need human acceptance; the
   benchmark's paint marker does not measure physical presentation latency.
