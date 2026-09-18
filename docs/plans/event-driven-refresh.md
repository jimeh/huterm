# Event-driven window refresh

Status: proposed. The groundwork in "Where the branch stands" is implemented on
branch `t3code/audit-gpui-renderer-performance`; the conversion in "Steps" has
not started.

This plan is written for an agent continuing the work on macOS, which is the
primary Huterm platform and the only one here with a real display. Read
[AGENTS.md](../../AGENTS.md) first. The measurements it relies on are described
in [the renderer performance notes](../performance/gpui-terminal-renderer.md).

## Outcome

Each window stops waking every 16 ms. Work happens when something changes: a
terminal publishes events, an animation is running, a native notification
arrives, or the pointer moves. A configuration option decides whether terminal
snapshots are clamped to the display's refresh rate.

This removes three costs that remain after the groundwork:

- A permanent 60 Hz timer per window, which also refreshes every hidden tab and
  clones each tab's title twice per tick.
- One redundant snapshot and repaint after every isolated update, described
  under "Where the branch stands".
- Up to 16 ms before a title change or shell exit reaches the tab bar.

Non-goals: changing the renderer's paint path, changing the protocol crate, and
the deferred renderer items listed under "Deferred renderer work".

## Where the branch stands

Six commits sit on top of `main` at release 0.11.0:

| Commit | Change |
| --- | --- |
| `850d2b5` | `bench:renderer-scenarios`, the output latency probe, Docker support |
| `e4cc474` | Glyph layout cache rotates only when full; ASCII cells skip hashing |
| `fbe3aff` | Scenario benchmark prepares one step per frame |
| `6284221` | One grid background quad, mask only built-in paths, slot-indexed built-in geometry |
| `6b006c9` | Scenario benchmark finishes sampling early in the process |
| `4072d88` | Snapshot on runtime activity; snapshot requests wake the runtime |

The macOS checks under "First tasks on macOS" ran on 2026-09-17.

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

### The redundant snapshot

After an activity snapshot, the pump's next tick drains the queued `Invalidated`
event, calls `scroll.invalidate()`, and starts a second snapshot. It reuses every
row `Arc`, so preparation rebuilds nothing, but the view still repaints.

Do not skip that invalidation by comparing the event's generation with the
applied snapshot's. Presentation updates, such as theme colors, invalidate
without advancing the content generation, because selections are tied to it.
The skip would leave stale colors on screen. The fix is a single consumer of
events, which is step 2.

## First tasks on macOS

These ran on 2026-09-17 on an M3 Max. `check`, `test`, and the renderer,
input, fullscreen, palette, and quit smokes pass. The quake smoke fails at its
external-grab step whenever another Huterm build on the host holds a global
shortcut, such as a parallel smoke session; every earlier phase passes. The
visual pass and the feel comparison in items 3 and 6 remain to be done.

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
| Terminal events | `TerminalView::refresh` per tab | `try_recv_event` is the only way events are read | `wait_for_activity` per terminal |
| Host effects | start of `TerminalView::refresh` | Clipboard writes queue in `HostEffectSink`, which does not go through `EventPublisher` | Make the sink set the activity signal when it queues an effect |
| Terminal scrollbar and resize indicator | `scrollbars.advance`, `resize_visibility.update` in `refresh` | Hold and fade timing | Animation clock while visible |
| Retry queued input, resize, presentation | `retry_client_messages` | `RuntimeError::Busy` when the message queue is full | Retry on the next activity wake and on a short timer while anything is queued |
| Scroll benchmark | `benchmark.drive` in `refresh` | Injects scroll input on a cadence | Its own timer, only when `HUTERM_SCROLL_BENCH` is set |
| Title and exit propagation | The `previous != current` comparison around `refresh` | Detects changes made by the drain | Direct result of the single event consumer |
| Queued closes | `resume_close` | Runs the next close once `busy` clears | Call where `busy` clears and where an exit is observed |
| Palette availability | `refresh_palette` | Compares a state tuple each tick | Call from the mutations that change that state |
| Palette scrollbar | `palette.advance` | Fade timing | Animation clock while visible |

Quake windows already use a loop that stops when idle (`keep_running` in
`crates/huterm-gpui/src/desktop/quake_windows.rs`). It is a working model for
"timer only while something is in motion".

## Design

### One consumer of terminal events, at the window

Move the per-tab body of the pump into `WorkspaceView::refresh_tab(tab_id)`:
call `terminal.refresh`, compare title and exit state, feed `exited_tabs`, notify
on a metadata change, then call `resume_close`. `WorkspaceView` spawns one task
per tab when the tab is created, and the task awaits `wait_for_activity` and
calls `refresh_tab`.

Remove the task in `TerminalView::new` in the same change. The signal supports
one waiter.

Make `HostEffectSink` set the activity signal when it queues an effect. Once the
pump is no longer a fallback, clipboard writes would otherwise wait for the
next terminal event.

`refresh` then starts the snapshot itself, as it does today, so each event is
handled once and the redundant snapshot disappears.

Pace the drain. After a drain clears `invalidation_pending`, a flooding program
publishes another event at once, so an unpaced loop would run at the PTY chunk
rate. A visible tab drains at most once per frame, as described next. A hidden
tab needs only title and exit, so it can drain at most every 100 ms. The view
ignores `Bell` events today.

Tab visibility is not window visibility. The active tab of an occluded or
minimized window has no frame clock, yet still needs title, exit, and
close-on-exit handling. Pace its drain with the hidden-tab timer whenever the
window delivers no frames, and hold at most one registered `on_next_frame`
callback per view so an occluded window does not accumulate them.

### Frame pacing and the refresh clamp

GPUI calls its frame callback on every display-link tick while the window is
visible, and runs `Window::on_next_frame` callbacks at the start of that tick,
before drawing. That is a frame clock that needs no knowledge of the refresh
rate, so it follows a 120 Hz ProMotion display.

Clamped mode, the default:

- Activity may start a snapshot immediately when none has started since the last
  frame tick. An isolated update therefore still reaches the next frame.
- Otherwise the view registers one `on_next_frame` callback, which starts the
  snapshot at the next tick. This replaces `ACTIVITY_SNAPSHOT_INTERVAL`.

Unclamped mode:

- A snapshot starts as soon as the previous one is applied and the terminal is
  invalid. The display still presents at most one frame per refresh, so this
  buys lower latency to the most recent output, not more visible frames.
- Snapshots are built on the runtime thread, which also parses PTY output.
  Measure `flood` throughput before choosing whether unclamped mode needs a
  floor between snapshots.

Configuration: add the option to `TerminalConfig` in `huterm-config`. A
candidate is `terminal.refresh = "display" | "unlimited"`. Regenerate schemas
with `mise run schema:generate`, extend `schemas/fixtures.json`, and apply the
value on config reload like `links`.

`on_next_frame` needs a `Window`. Use it from a window update, not from a bare
timer. AGENTS.md records that `request_animation_frame` panics outside a view
render in GPUI 0.2.2; `on_next_frame` does not depend on the current view, but
verify that before relying on it.

The 16 ms pump caps sustained output at 60 snapshots per second on macOS even
when the display refreshes faster. The frame clock removes that cap.

### Animation clock

Replace the unconditional timer with one that runs only while an animation is
active. Each animated component already reports progress through a `bool` from
`advance`. Keep a per-window "animating" flag: any code that starts an animation
sets it and ensures the clock task is running; the clock advances every
component and stops when all of them report no change and nothing is pending.

Prefer `on_next_frame` plus `cx.notify()` over a 16 ms timer for the clock, so
animations follow the display's refresh rate.

Headless Xvfb delivers frames only with a window manager. Every Linux smoke and
benchmark that depends on animation progress must keep working; check how each
starts its X session before removing its timer-driven progress.

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

## Steps

Convert one duty at a time with the pump still running, so every step is
shippable and bisectable. Remove the timer last.

1. **Record macOS baselines.** "First tasks on macOS" above.
2. **Single event consumer.** Add `refresh_tab`, the per-tab activity tasks, and
   drain pacing; remove the task in `TerminalView::new`. The pump keeps calling
   `refresh_tab` as a fallback. Success: `echo` stays near its current median,
   the redundant snapshot is gone, and title, exit, and close-on-exit still work
   for active and hidden tabs.
3. **Frame pacing and the clamp option.** Replace the 8 ms rule with the frame
   clock and add the configuration option. Success: `flood` holds one snapshot
   per display frame in clamped mode on a 60 Hz and a 120 Hz display.
4. **Animation clock.** Move terminal scrollbars, the resize indicator, tab
   scrolling, tab scrollbars, the palette scrollbar, and `Reveal` onto it. The
   pump stops advancing them.
5. **Pointer-driven reveal.**
6. **Call-site triggers.** `resume_close`, `refresh_palette`, and
   `retry_client_messages`.
7. **Fullscreen.** Drive `refresh_fullscreen` from the native observer callback
   and a deadline timer. This has the most platform invariants; read the
   fullscreen sections of AGENTS.md before touching it.
8. **Remove the pump timer.** Keep a debug assertion or log for one release that
   reports work found by a slow fallback tick, to catch a missed wake source.

Steps 2 and 3 deliver the latency and correctness benefits. Steps 4 to 8
deliver the idle-CPU benefit and carry most of the risk; they can ship later.

## Invariants to preserve

These come from AGENTS.md and from failures found while building the groundwork.

- Only the visible tab starts a snapshot. `begin_visible_snapshot` enforces it.
- Drain terminal events in bounded batches. `refresh` reads at most 64 events
  and 8 host effects per call.
- Shell-exit transitions queue once for assessed tab close, separately from the
  merged manual close intent.
- Never lock the Mux mutex from rendering, input, or these wake paths.
- `TerminalView` destruction only detaches. A task holding a `RuntimeClient`
  clone must end when its view or tab goes away; `wait_for_activity` returns an
  error once the runtime stops, and a failed `update` means the view is gone.
- Keep terminal focus and gesture ownership rules around overlays and tab drags.
- Exited tabs keep history, selection, and scrolling, so their views must still
  repaint on scroll and selection.

## Verification

| Risk | Evidence |
| --- | --- |
| Latency regresses | `bench:output-latency` in `echo` mode, before and after each step, on macOS |
| Sustained output floods the runtime | `bench:output-latency -- flood` snapshot rate, and `bench:engine` throughput |
| Scroll coordination breaks | `bench:scroll`, which gates snapshot timing, wakeups, offsets, and queue bounds |
| A missed wake leaves the UI stale | Step 8's fallback report, plus the smokes for input, fullscreen, quake, palette, and quit |
| Hidden tabs miss exit or title changes | A GPUI test around `refresh_tab` for a hidden tab, and a manual check with `close_on_exit` |
| Idle CPU does not improve | Wakeups per second for an idle window, measured with Instruments or `powermetrics` before step 4 and after step 8 |
| Renderer regresses | `bench:renderer-scenarios -- --compare` against the macOS baseline |

Unit-test pacing decisions as pure functions with injected times, as
`starts_snapshot_on_activity` is tested now. Do not add tests that sleep to wait
for a wake; synchronize on the signal or poll a condition under a deadline.

Run `mise run verify` before handing the branch back. It has not been run on
this branch.

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

## Unresolved questions

1. Should unclamped mode have a floor between snapshots? Decide from the `flood`
   throughput measurement in step 3.
2. What should the option be called, and should it live under `[terminal]`?
3. Answered: `on_next_frame` does not fire for an occluded or minimized window
   on macOS. GPUI's `start_display_link` returns early unless the window's
   `occlusionState` is visible, and `window_did_change_occlusion_state` stops
   the link. The callback vector is drained only by the request-frame closure.
   See "Tab visibility is not window visibility" under the design.
4. Answered: the groundwork ships first in its own pull request. Steps 2 and 3
   follow in a second one; steps 4 to 8 later, after measuring idle wakeups
   with `powermetrics` at `main` and with the pump body stubbed out. GPUI's
   display link wakes the process on every vsync while a window is visible, so
   removing the pump lowers per-tick work but not the wakeup rate.
