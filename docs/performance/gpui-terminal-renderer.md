# GPUI terminal renderer measurements

Measured on 2026-09-03 using Linux 6.8.0 x86_64, an AMD Ryzen 5 5600GT with
six available cores, Rust 1.98.0, GPUI 0.2.2, and Mesa's Lavapipe Vulkan driver
under Xvfb and `twm`. All Huterm builds used Cargo's release profile.

## Result

The previous 100 by 32 renderer prepared one `ShapedLine` per nonblank cell.
An instrumented trace measured:

- 207,935 microseconds for full-frame preparation.
- 201,558 microseconds within the calls to `shape_line` and their cached-layout
  machinery.
- About 4.8 prepared frames per second.

The retained renderer's repository-owned animated-color workload produced
these three full-grid samples after build and system font caches were warm:

| Sample | Prepare | Paint encoding | Layout cache |
| --- | ---: | ---: | ---: |
| 1 | 735 us | 1,144 us | 3,199 hits, 1 miss |
| 2 | 645 us | 987 us | 3,199 hits, 1 miss |
| 3 | 688 us | 1,022 us | 3,199 hits, 1 miss |

Median preparation fell from 207,935 to 688 microseconds: about 302 times less
CPU time, or a 99.7% reduction. Median preparation plus paint encoding was
1,710 microseconds, well below a 16.7 ms 60 Hz CPU frame budget. The first run
after rebuilding measured 1,913 microseconds preparing and 1,884 microseconds
encoding paint primitives.

For a separate check with `const-void/DOOM-fire-zig` at commit
`eb0631b141b5778eefc6f5767bb45f8974c1be71`, patched only to skip its intro and
size-check screens, Huterm's first 100 by 32 frame measured 2,652 microseconds
preparing and 3,094 microseconds encoding paint primitives, with 3,123 layout
hits and 27 misses. The same workload emitted about 2,434 frames per second
directly into a PTY.

## Interpretation

The old timing was not pure font shaping. GPUI already retains `LineLayout`
values for two frames. The cost also included repeated string and font work,
construction and movement of large `ShapedLine` values, and one paint layer per
cell. The replacement caches compact `Arc<LineLayout>` handles, resolves the
configured font only on cache misses, retains unchanged rows, and uses one clip
layer per row.

These figures measure CPU preparation and scene encoding, not presentation or
GPU time. This headless host presents GPUI's initial frame but does not deliver
continuous animation frames under Xvfb, even with `twm`, so it cannot establish
sustained visible FPS.

## macOS whole-pipeline measurement

Measured on 2026-09-03 using a MacBook Pro `Mac15,8`, an Apple M3 Max with 16
CPU cores and 40 GPU cores, 64 GB RAM, macOS 27.0 build `26A5425a`, and Xcode
27.0 build `27A5228h`. Huterm and DOOM-fire-zig used release builds. The
terminal started at exactly 120 by 40 cells, and DOOM-fire-zig was at commit
`eb0631b141b5778eefc6f5767bb45f8974c1be71`.

At branch commit `2c5989e5b56fcb0312bdeefeecfd47daa9d45f5c`, DOOM-fire-zig reported
5.05 to 5.22 producer frames per second with renderer statistics disabled and
5.38 with `HUTERM_RENDER_STATS=1`. A Metal System Trace nevertheless measured
59.91 displayed frames per second. The on-screen DOOM counter therefore
measured how quickly its PTY writes completed, not how often Huterm reached the
display.

Time Profiler and reversible retry-delay experiments isolated the throttle to
the PTY reader's fixed two-millisecond sleep after every nonblocking
`WouldBlock`. Reducing only that delay to 50 microseconds raised the same
workload from about 5 to 148 producer frames per second; changing only the
full-channel retry delay reached 6.13. The retained renderer was not the active
bottleneck.

Replacing the `WouldBlock` sleep with a readiness wait raised the same release
workload, sampled after at least 10 seconds, to 707.91 producer frames per
second with renderer statistics enabled and 742.91 with them disabled. The
counters reduced throughput by about 4.7%, which is material when quoting an
exact result but does not change the diagnosis. With statistics enabled, stable
rolling windows measured 116 to 150 microseconds average preparation and 2,691
to 2,883 microseconds average GPUI paint encoding. With statistics disabled, a
separate 8.3-second Metal System Trace measured 60.12 presented callbacks per
second.

At 742.91 producer frames per second, a `ps` sample showed about 130.5% CPU for
Huterm and 80.8% for DOOM-fire-zig. A 10-second Time Profiler trace attributed
57.11% of sampled CPU to the runtime thread, 24.20% to the PTY reader, and
17.04% to the GPUI main thread. The largest leaf costs were `tcgetpgrp`'s
`ioctl` path at 28.20%, Alacritty processing at 14.39%, the readiness wait's
`select` at 13.81%, PTY `read` at 8.92%, and GPUI bounds-tree insertion/search
at 11.80% combined. `Window::paint_glyph` accounted for 0.36% and renderer
preparation for 0.57%.

For the exact-size comparison, `INITIAL_COLUMNS` and `INITIAL_ROWS` were
temporarily changed to 120 and 40 for the measurement binary, then restored.
The workload was launched as follows and started by pressing Return after its
capability screen:

```sh
SHELL=/Users/jimeh/src/DOOM-fire-zig/zig-out/bin/DOOM-fire \
  HUTERM_RENDER_STATS=1 target/release/huterm
```

## Reproduction

Install the Linux desktop dependencies from the development guide, including
`twm`, then run:

```sh
mise run bench:renderer
```

The task builds Huterm and its workload in release mode, launches a 100 by 32
animated grid under Xvfb, and prints `huterm-render` timing lines. Setting only
`HUTERM_RENDER_STATS=1` while running Huterm enables rolling renderer counters
on platforms that deliver continuous animation frames.

With `HUTERM_RENDER_STATS` set, each interval that applied a snapshot also
prints a `huterm-render output` line. `applied_us` is the delay from a
terminal's earliest unseen invalidation to its snapshot reaching the view, which
includes the wait for the window refresh pump. `painted_us` extends that delay
to the end of the paint that shows the snapshot.

`HUTERM_RENDER_STATS=1` requests a frame on every display tick. On a real
display that keeps the main thread in Metal present until vsync, so the
activity wake and every snapshot wait for the frame: on macOS it reported an
`echo` applied median of 12.7 ms where an ordinary session applies in 126 µs.
`HUTERM_RENDER_STATS=events` records the same counters while painting only when
the application invalidates. Use it for latency; the rolling per-frame counters
then print only while output keeps arriving.

### Output latency

`mise run bench:output-latency` runs Huterm against `render_workload` for eight
seconds and summarizes the `huterm-render output` lines, skipping the first
interval. The default `echo` mode writes a few bytes about every 100 ms, so each
write is an isolated update like an echoed keystroke. Setting
`HUTERM_OUTPUT_LATENCY_APPLIED_BUDGET_US` fails the run when the `echo` applied
median exceeds it; `ci:benchmarks` uses 5000 µs, which pump-paced medians of
8 to 12 ms exceed on every host measured. Pass `-- flood` for the
animated grid, which checks that sustained output stays paced. `applied_us` is
valid under Xvfb; `painted_us` there reflects GPUI's 60 Hz refresh timer rather
than a display.

A terminal view starts a snapshot as soon as its runtime signals activity, when
it is visible and its last snapshot started at least 8 ms earlier. Otherwise the
window refresh pump starts it on its next 16 ms tick. The pump remains the only
consumer of terminal events. A snapshot request also wakes the runtime thread,
which otherwise polls its control channel every 2 ms. Linux x86_64 medians, with
maximums in parentheses:

| Configuration | `echo` applied | `flood` applied | `flood` snapshots per second |
| --- | --- | --- | --- |
| Pump only | 10.4 ms (17.1) | 16.2 ms (32.4) | 60 |
| Activity snapshot | 2.2 ms (11.2) | 12.8 ms (31.7) | 89 |
| Activity snapshot and runtime wake | 0.4 ms (6.3) | 12.3 ms (46.6) | 80 |

macOS arm64 medians on 2026-09-18 with `HUTERM_RENDER_STATS=events` on a
MacBook Pro M3 Max built-in display, maximums in parentheses:

| Mode | Applied | Painted | Snapshots per second |
| --- | --- | --- | --- |
| `echo` | 126 µs (1.3 ms) | 5.1 ms (8.9) | 10 |
| `flood` | 14.4 ms (19.6) | 17.2 ms (26.3) | 61 |

`flood` holds 61 snapshots per second because the 16 ms pump paces sustained
output, even on a display that refreshes faster than 60 Hz.

After an activity snapshot, the pump still drains the queued invalidation and
starts one more snapshot, which reuses every row. It cannot be skipped by
comparing generations: presentation updates invalidate without advancing the
content generation.

### Pre-refresh-change baseline, 2026-09-19

Fresh native macOS runs used revision
`ee91a54efd20ff760f7295bcc5875965d8279c08`, before event-driven refresh
implementation. Product and benchmark sources matched that revision; the only
working-tree change when measurements started was the refresh plan. The renderer
report consequently labels the revision `ee91a54-dirty`.

Host: M3 Max with 40 GPU cores and 64 GiB RAM, macOS 27.0 build `26A428`,
Xcode 27.0 build `27A266a`, on AC power. The main display was an external LG
at 60 Hz, with the built-in display and an external BenQ also online. The
inventory does not establish each benchmark window's actual display or delivered
frame rate. Compare future runs under the same display setup; do not treat the
older built-in-display measurements as a controlled before/after comparison.

All tasks ran serially. `bench:renderer-scenarios` used its default five runs
per scenario. `bench:output-latency` ran three times each in `echo` and `flood`
mode, eight seconds per run, with the runner's isolated empty configuration and
`HUTERM_RENDER_STATS=events`. Each latency result summarizes six steady intervals
after excluding startup. All six runs and the scroll benchmark passed.

| Mode/run | Applied median µs | Applied max µs | Painted median µs | Painted max µs | Snapshots/s |
| --- | ---: | ---: | ---: | ---: | ---: |
| echo 1 | 100 | 263 | 9,777 | 17,038 | 10 |
| echo 2 | 127 | 243 | 12,970 | 17,364 | 10 |
| echo 3 | 111 | 259 | 10,469 | 16,978 | 10 |
| flood 1 | 14,844 | 19,717 | 24,956 | 33,261 | 60 |
| flood 2 | 14,808 | 25,868 | 25,836 | 32,473 | 60 |
| flood 3 | 14,837 | 17,048 | 25,604 | 27,932 | 60 |

Renderer timings below are medians of per-process medians, in microseconds.
Every scenario reported zero glyph layout cache misses. `scroll` rebuilt one
row; the other scenarios rebuilt 50.

| Scenario | Prepare µs | Paint encoding µs |
| --- | ---: | ---: |
| ascii | 180.0 | 525.0 |
| blocks | 218.8 | 273.6 |
| boxes | 148.3 | 1,070.2 |
| churn | 193.8 | 585.8 |
| scroll | 14.4 | 539.3 |
| selection | 182.3 | 576.8 |

`bench:scroll` passed snapshot and presentation gates with 70 snapshot samples
and 60 paint samples. Median snapshot elapsed time was 173 µs, p95 input to
snapshot was 655 µs, and median wakeup was 10 µs. Median paint elapsed time was
356 µs and p95 input to paint was 1,409 µs. Maximum concurrent requests and
maximum queued updates were both one. These are CPU-side completion/encoding
measurements, not proof of GPU presentation time.

Local artifacts are under `target/bench/2026-09-19-baseline/`: `host.json`,
`renderer.json`, `summary.json`, `latency-runs.json`, and the task logs. Use
`renderer.json` with the scenario runner's `--compare` option. These ignored
artifacts are local; the tables above retain the results in repository docs.
This refresh did not rerun the full test/smoke suite, measure idle power/wakeups,
or establish a 120 Hz baseline. Those remain separate verification work.

### Built-in display baseline, 2026-09-19

The baseline was repeated with `HUTERM_BENCH_DISPLAY_ID=1`, explicitly selecting
the built-in panel while leaving the external LG as the primary display. macOS
reported a 120 Hz maximum for display 1. Every renderer run reported display 1;
echo and flood probes verified the window stayed on it throughout sampling.
After the first callback interval, both modes delivered 119.9 to 120.1 callbacks
per second. This establishes actual 120 Hz delivery during these latency runs,
not just the panel's capability. Callback timing is not GPU presentation timing.

The base revision remains `ee91a54`; the working tree adds only display selection
and benchmark diagnostics to executable code. Refresh scheduling is unchanged.
The opt-in callback observer does not request redraws, but it adds diagnostic
work. Use the same instrumentation for subsequent comparisons. An initial
renderer run overlapped compilation and is retained as `warmup-*`, excluded
from the baseline. All reported runs were serial, after compilation/checks.

| Mode/run | Applied median µs | Applied max µs | Painted median µs | Painted max µs | Snapshots/s |
| --- | ---: | ---: | ---: | ---: | ---: |
| echo 1 | 116 | 1,492 | 6,048 | 8,506 | 10 |
| echo 2 | 132 | 1,206 | 4,172 | 9,413 | 10 |
| echo 3 | 120 | 440 | 5,668 | 9,024 | 10 |
| flood 1 | 14,205 | 17,123 | 18,907 | 25,814 | 60 |
| flood 2 | 14,491 | 20,924 | 19,083 | 29,563 | 60 |
| flood 3 | 14,673 | 20,489 | 20,773 | 29,716 | 60 |

Despite 120 Hz callbacks, flood still produced 60 snapshots per second. This is
the baseline for assessing removal of the pump's sustained-output cap.

| Scenario | Prepare median µs | Paint encoding median µs |
| --- | ---: | ---: |
| ascii | 175.4 | 527.7 |
| blocks | 212.0 | 264.8 |
| boxes | 146.6 | 994.3 |
| churn | 193.2 | 571.2 |
| scroll | 13.0 | 530.5 |
| selection | 172.7 | 567.3 |

The renderer used five runs per scenario, all with zero glyph layout cache
misses. Render intervals had medians near 8.33 ms, with longer gaps in some
processes; per-run interval counts and elapsed times are preserved in the log.
Do not infer uninterrupted 120 fps from those renderer medians alone.

The scroll benchmark passed with 70 snapshot and 67 paint samples, median
snapshot elapsed time 179 µs, p95 input to snapshot 10,615 µs, median wakeup
6,433 µs, median paint elapsed time 385 µs, and p95 input to paint 10,949 µs.
Maximum concurrent requests and queued updates remained one. These scroll
latencies are materially higher than the earlier external-display run; retain
both results and compare future changes on the same display and instrumentation.
The cause of that difference has not been isolated.

Artifacts are in `target/bench/2026-09-19-builtin/`, including `renderer.json`,
`summary.json`, `runs.json`, display inventory, task logs, and a copy of the
harness changes. Use this renderer report for the upcoming built-in-display
comparison. The earlier external-display report remains intact.

Display targeting applies to the existing renderer, output-latency, and scroll
tasks. For this host's current display ID:

```sh
HUTERM_BENCH_DISPLAY_ID=1 mise run bench:renderer-scenarios -- --output target/bench/builtin.json
HUTERM_BENCH_DISPLAY_ID=1 mise run bench:output-latency
HUTERM_BENCH_DISPLAY_ID=1 mise run bench:output-latency -- flood
HUTERM_BENCH_DISPLAY_ID=1 mise run bench:scroll
```

Display IDs identify currently connected displays; do not assume `1` is always
the built-in panel on another host or after a topology change. The override
rejects missing or invalid IDs instead of silently selecting the primary display.
Without it, window placement retains its normal behavior.

### Renderer scenarios

`bench:renderer` depends on PTY throughput and snapshot scheduling, and its
workload paints only built-in block rectangles. To measure the renderer alone,
run:

```sh
mise run bench:renderer-scenarios -- --output baseline.json
mise run bench:renderer-scenarios -- --compare baseline.json
```

Each scenario runs the production `prepare` and `paint` against synthetic 160 by
50 snapshots in a release build, in its own process, five times by default. The
report gives the median of the per-process medians, the overall minimum, and
counts that do not depend on timing. `--compare` adds the change against an
earlier report.

| Scenario | Measures |
| --- | --- |
| `ascii` | Full rebuild and paint of dense styled text |
| `blocks` | The `bench:renderer` half-block grid: built-in rectangles only |
| `boxes` | A bordered TUI whose rounded corners, diagonals, and powerline separators paint as paths |
| `churn` | A full redraw of about 480 distinct characters after two single-row updates; cache misses show what those updates evicted |
| `scroll` | Output scrolling by one row: retained rows shift and one is rebuilt |
| `selection` | `ascii` painted under a full-screen selection with a selection foreground |

The same task runs in the Linux test container, which suits macOS hosts and
keeps host load and fonts out of the comparison:

```sh
mise run linux:exec -- mise run bench:renderer-scenarios -- \
  --output target/bench/baseline.json
mise run linux:exec -- mise run bench:renderer-scenarios -- \
  --compare target/bench/baseline.json
```

Keep container reports under `target`: the workspace sync deletes other
untracked files but preserves that directory in the worktree's cache volume.
Container timings come from a virtual machine on macOS, so compare them only
with reports from the same container and host.

Each frame prepares one step and paints it, and a measured step records both
timings. Preparing every step inside one frame hid shaping cost: GPUI keeps line
layouts for its current and previous frame, so a layout the renderer had evicted
came back from that cache instead of the platform shaper. Repeated paints inside
one frame grow that frame's scene and inflated later samples threefold. Linux
therefore needs `twm`, because Xvfb without a window manager never reports the
window visible and GPUI stops after one frame.

Sampling finishes within about 1.5 seconds of process start, and `elapsed_ms` on
each process's `phase=window` line reports when it ended. On macOS arm64 the same
paint cost 2.2 to 2.7 times more once the process was about two seconds old: an
`ascii` paint measured 0.5 ms early and 1.1 to 1.4 ms later, presumably because
the scheduler stops favoring a lightly loaded process. The early state repeats
within about 2%, so it is the one to compare, but a terminal in ordinary use
spends its time in the slower state. Raising
`HUTERM_RENDERER_BENCH_ITERATIONS` moves samples into that state.

Reports are comparable only when produced by the same version of the benchmark.
Moving preparation to one step per frame raised prepare timings about 50% for
the same code, because each step now starts with colder CPU caches, and
recording paint alongside each step shifted paint timings by up to 10%.
Timings include scheduler preemption, so compare medians and minimums, and treat
paint differences under about 10% as noise. Two consecutive five-run baselines on
one Linux host differed by about 3% or less in every median; two-run reports in
the container differed by up to 6%.

Glyph layout cache misses cost far more on some hosts than others, because the
cost of shaping one cell depends on the installed fonts. The same 426 misses in
`churn` took 26 ms on a desktop Linux host and 0.7 ms in the container, which
installs only DejaVu.

`mise run bench:scroll` accounts for this limitation by gating snapshot elapsed
time, wakeup delay, returned offsets, and queue bounds from completion-time
records. It also checks renderer elapsed time, row reuse, and input-to-paint
latency when a host produces at least 25 paint samples. Linux Xvfb does not
satisfy that condition; run the same task from a frame-delivering macOS session
for those measurements.
The scroll timings use a monotonic wall clock and include scheduler preemption;
they do not measure per-thread CPU time.

### Post-refresh measurements, 2026-09-19

Compared `1d42558` with a fresh release build of the pre-change baseline
`94f8028`, using separate worktrees and Cargo target directories. The current
setup exposes only built-in display 1 at scale 1, 2704 by 1756, with roughly
120 GPUI callbacks/s. Earlier scale-2 reports are not directly comparable.
Builds finished before sampling; baseline and feature benchmarks ran serially
after the overloaded-host checks described below.

The output-latency harness accepts `HUTERM_BENCH_REFRESH=display` or `unlimited`
and writes that policy into its isolated config. With no override it writes an
empty config, preserving compatibility with the baseline executable. Early
baseline runs that passed the unsupported `refresh` field were excluded.

#### Output latency and frame pacing

Three runs per mode, each using the harness's six measured intervals. Values
are medians of per-run medians, not pooled sample medians. Painted latency ends
at the renderer's paint marker; it does not measure physical presentation.

| Build and policy | Echo applied µs | Echo painted µs | Flood snapshots/s | Flood applied µs | Flood painted µs |
| --- | ---: | ---: | ---: | ---: | ---: |
| Baseline | 134 | 5,682 | 60 | 14,895 | 18,847 |
| Feature, display | 146 | 6,768 | 120 | 13,184 | 15,414 |
| Feature, unlimited | 169 | 6,649 | 217 | 2,146 | 8,219 |

Default flood throughput now follows the observed 120 Hz callbacks. Unlimited
runs produced 216 to 223 snapshots/s while retaining one request in flight.
An additional timer floor is not introduced; display pacing remains the default.
Worst observed flood painted latency across the three runs was 45.2 ms for the
baseline, 21.9 ms for display pacing, and 18.6 ms for unlimited mode.

Echo application stays below 0.2 ms in every per-run median. The first set's
painted median increased by about 1.1 ms; baseline per-run medians ranged from
5.1 to 6.6 ms and display-mode medians from 5.7 to 7.0 ms. This overlaps, but is
not evidence that painted echo latency is unchanged. A second set alternating
baseline and feature runs measured applied medians of 180 versus 205 µs and
painted medians of 5,836 versus 6,592 µs. Individual painted medians ranged from
5.4 to 7.3 ms for the baseline and 6.3 to 7.3 ms for the feature. This leaves a
small possible painted-echo regression; its cause has not been isolated.

The macOS API accepted a temporary change to the same-size 60 Hz display mode,
but GPUI still delivered about 120 callbacks/s and flood applied about 118
snapshots/s. The original 120 Hz mode was restored. Those runs are retained as
an inconclusive 60 Hz attempt, not counted as 60 Hz acceptance. Admission tests
exercise explicit frame ticks, but a native host delivering 60 Hz is still
needed to complete that coverage.

#### Idle CPU and wakeups

Each row is the median of three five-second samples, following two seconds of
settling, with one window and the stated number of idle shell tabs. Process
labels were disabled (`tabs.label = "title"`). Hidden means the application was
hidden through AppKit; it does not cover every form of window occlusion.
`proc_pid_rusage` measured the Huterm process, not WindowServer or shell CPU.
CPU counters were converted with `mach_timebase_info` and checked against a
known CPU interval. 100% CPU denotes one logical core. Interrupt wakeups are
not all context switches and do not establish power consumption in watts.
`powermetrics` was unavailable without interactive sudo.

| Tabs | Window | Baseline CPU % | Feature CPU % | Baseline interrupt wakeups/s | Feature interrupt wakeups/s |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | Visible | 2.86 | 2.07 | 1,258 | 181 |
| 1 | Hidden | 2.76 | 0.89 | 1,256 | 60 |
| 10 | Visible | 22.81 | 1.84 | 12,198 | 180 |
| 10 | Hidden | 19.65 | 0.93 | 12,067 | 60 |
| 50 | Visible | 124.62 | 2.18 | 59,729 | 180 |
| 50 | Hidden | 77.91 | 0.89 | 60,450 | 60 |

A temporary startup hook created tabs through the normal `new_tab` path and
finished before sampling. The harness verified the exact child count and
readiness. Both builds used that hook; no continuous benchmark frame observer
was enabled. The hook and the following empty-pump probe were removed before
rebuilding production binaries. No instrumentation ships in the application.

A third build removed the refresh pump's body while retaining its timer. Its
visible CPU medians were 1.03%, 1.83%, and 1.47% for 1, 10, and 50 tabs; hidden
medians were 0.82%, 0.81%, and 0.77%. Interrupt wakeups remained 180/s visible
and 60/s hidden. The main implementation removes per-tab idle scaling; the
remaining pump body offers modest, variable savings. The visible/hidden delta
is consistent with the 120 Hz display link plus the retained 60 Hz timer.

Steps 4 to 8 of the event-driven plan are therefore deferred. The timer remains
responsible for animations, pointer reveal, retries, and native fullscreen
coordination. Removing it would save residual wakeups, especially while hidden,
but would not remove the visible display-link cost. Revisit that tradeoff if
profiles or battery measurements justify the additional lifecycle work.

Resident memory at 50 visible tabs rose from 177.3 MiB to 195.5 MiB. The Unix
implementation adds one blocked child-wait thread per terminal; this experiment
did not isolate its contribution to the roughly 18 MiB increase. Separate
process-label overhead and direct battery impact were not measured.

#### Renderer and scroll regression checks

Five fresh runs per renderer scenario, under the same scale-1 setup:

| Scenario | Baseline prepare µs | Feature prepare µs | Baseline paint µs | Feature paint µs |
| --- | ---: | ---: | ---: | ---: |
| ascii | 162.3 | 162.6 | 494.5 | 502.5 |
| blocks | 204.5 | 206.1 | 257.2 | 259.6 |
| boxes | 137.2 | 135.5 | 968.1 | 972.1 |
| churn | 179.6 | 186.9 | 547.0 | 557.5 |
| scroll | 11.2 | 10.8 | 518.4 | 510.4 |
| selection | 164.7 | 162.6 | 554.6 | 552.6 |

Prepare medians changed by -3.7% to +4.1%, and paint medians by -1.6% to +1.9%.
All scenarios retained zero glyph-layout misses and identical operation counts.
These results show no material change to renderer preparation or paint encoding.

The feature scroll gate passed with 70 snapshot and 69 paint samples. Median
snapshot elapsed time was 315 µs, p95 input-to-snapshot 8,043 µs, median wakeup
23 µs, median paint elapsed time 640 µs, and p95 input-to-paint 11,289 µs.
Maximum concurrent requests and queued updates both remained one. Timings
include scheduling and are not per-thread CPU measurements. A fresh baseline
scroll run also passed: 311 µs median snapshot, 10,021 µs p95 input-to-snapshot,
23 µs median wakeup, 667 µs median paint, and 11,082 µs p95 input-to-paint, with
70 snapshot and 68 paint samples and the same queue bounds.

#### Native acceptance and artifacts

Native input, clipboard, integration, fullscreen, palette, Quit, and Quake
smokes all passed on the implemented source. Earlier Quake attempts timed out
creating a hidden window or acquiring external witness focus. Some ran while
host load exceeded 330; one later witness state identified `loginwindow` as
frontmost despite the screen reporting unlocked. After a standalone witness
successfully activated, the complete Quake smoke passed without product changes
or relaxed assertions. The exact cause of those setup failures was not isolated.

`mise run verify` passed. Native physical IMEs, subjective shell/editor/DOOM
feel, and long-running interactive workloads remain outside these automated
checks. A genuine 60 Hz callback source remains an acceptance gap.

Local artifacts are in `target/bench/refresh-native/`: `latency-summary.json`,
`idle-summary.json`, per-tab-count idle JSON reports, baseline/feature renderer
JSON and logs, scroll logs, native smoke logs, and probe sources. The valid idle
reports include `cpu_timebase_factor`; earlier exploratory reports without it
used unconverted clock ticks and are excluded. Files prefixed
`excluded-config-baseline` are also excluded. Temporary probes and binaries are
local evidence, not committed product changes.

### Hardening follow-up, 2026-09-20

The independent review of `94f8028..52b1037` found no verified correctness defect
in the wake handoff, bounded drains, cancellation ordering, child ownership,
hidden-tab handling, or weak frame registrations. That is inspection evidence,
not a replacement for regression tests. The focused follow-up adds
`mise run smoke:macos-refresh` to exercise the real callback wiring:

- Minimize the native window and observe AppKit occlusion before producing
  output. Title events must continue while snapshots remain blocked behind one
  pending callback. Restore the window and require the latest output to appear
  without another producer event.
- Remove the tab while its activity task is waiting, retaining the core terminal.
  Require the task future to drop before waking the runtime, then request a
  snapshot to prove the core terminal remains usable.
- Create a replacement tab with the old callback pending. Require one callback
  for the window, then restore frames and observe the replacement snapshot and
  release of the old view. GPUI may retain the old rendered scene until repaint.

The smoke observes production task lifetime through a probe installed only by
its entrypoint. It counts the weak clock references owned by actual deferred
registrations/native callbacks; it does not substitute a frame dispatcher.
Bounded polling observes state changes rather than assuming animation or PTY
completion after a fixed delay. This is native macOS coverage; it does not extend
Linux frame-delivery coverage or measure latency.

Two temporary negative controls failed at their intended assertions: detaching
instead of owning the activity task left its probe waiting after removal;
removing the frame-registration guard created a duplicate callback during tab
replacement. Both source changes were restored before the final passing run.

#### Native 60 Hz coverage

Keeping the display-mode helper alive during the benchmark resolved the earlier
60 Hz setup problem. CoreGraphics reported mode 115 at 60 Hz; GPUI delivered
about 60 callbacks/s, with median intervals near 16,667 µs. The default flood
benchmark applied 60 snapshots/s across ten measured intervals. Applied latency
was 15,780 µs median and 22,020 µs maximum; painted latency was 31,193 µs median
and 34,802 µs maximum. These are absolute 60 Hz measurements, not a comparison
against a fresh 60 Hz baseline. The original mode 114 at 120 Hz was restored and
queried afterward.

#### Longer echo comparison

Four 30-second runs in baseline/feature/feature/baseline order each delivered
27 measured intervals at about 120 callbacks/s:

| Build | Applied median µs, by run | Painted median µs, by run |
| --- | --- | --- |
| Baseline | 137, 108 | 5,817, 5,829 |
| Feature | 181, 183 | 6,239, 6,385 |

The small increase persists in these longer runs. It should not be dismissed as
noise or described as a uniform latency improvement. Source inspection found no
mandatory additional frame wait for isolated output: the previous callback
normally replenishes admission long before the next roughly 100 ms update.
The feature does route activity through the workspace and bounded event drain,
and the runtime now uses a separate coalesced wake instead of a timed receive.
These are candidates for tracing, not established explanations of the delta.

A temporary probe carried a producer wall-clock timestamp in the terminal text
and measured only replies whose original `invalidated_at` was present. That
avoids counting baseline duplicate snapshots. After unlocking, a paired
30-second comparison measured producer-to-application medians of 220 versus
389 µs and producer-to-paint medians of 6,786 versus 6,979 µs. Both reported
27 intervals and about ten updates/s. Earlier valid origin runs measured
340/6,093 µs for baseline and 539/7,192 and 517/6,618 µs for feature.
The producer-origin results do not support dismissing the increase as merely
an earlier invalidation timestamp. They also show that its painted magnitude
varies. Wall-clock stability and lack of coalesced-away emissions are assumptions
of this short diagnostic; it still does not measure physical presentation.

#### Memory decomposition

An initial VM comparison at 50 tabs measured 4,752 KiB versus 5,680 KiB of
resident stack memory, despite roughly 100 MiB more reserved stack address space.
The added resident stacks therefore explain under 1 MiB of the total increase.
Allocated heap bytes were 76.6 MiB versus 92.2 MiB. The allocation histogram
contained roughly 50 additional blocks in the 272 KiB size class; a correlation
with thread count alone does not identify their owner.

A one-tab allocation-stack probe then identified 278,528-byte allocations through
`dyld::ThreadLocalVariables::instantiateVariable` at Rust thread startup. The
linked executable's `__thread_bss` contains a 256 KiB
`Thread.maybeAttachSignalStack.global.signal_stack` symbol. The pinned Zig 0.16.0
standard library declares that buffer as `threadlocal`, with the default
`signal_stack_size = 1 << 18`. The statically linked image's TLS block is
materialized even for Rust threads that do not parse terminal output.
Fifty additional blocks at the observed allocation size account for 13.28 MiB,
which explains most of the increase. Reducing ordinary thread stack reservation
would not fix this cost. Changing the native library's signal-stack policy is a
separate safety and upstream integration decision, not part of this pass.

A second setup waited for every tab's `READY` snapshot before creating the next
tab. Resident memory still differed by about 15.2 MiB, and allocated heap bytes
were 75.7 MiB versus 90.5 MiB. Initial snapshot population does not explain away
the difference. This setup does not guarantee identical prepared renderer-cache
population, so it cannot attribute every remaining byte to the child waiter.

Artifacts are under `target/bench/refresh-hardening/`. A screen lock interrupted
one producer-origin run and prevented a frame-dependent allocation fixture from
starting; those runs are excluded. Product sources were restored byte-for-byte
and production binaries rebuilt after temporary probes.

### Scroll admission follow-up (2026-09-21)

The original shared frame allowance regressed scrolling on Linux under Xvfb.
Same-runner comparisons rebuilt baseline `c3ab933` and the PR's `0a5d541`
implementation in separate worktrees and target directories. The diagnostic
branch applied a bounded viewport allowance only after measuring both originals.
Each candidate run retained 70 measured snapshot samples, 65 matching paint
samples, and maximum in-flight and queued counts of one.

P95 input-to-matching-paint elapsed time, in milliseconds:

| Runner | Baseline | Original PR | Extra viewport allowance, two runs |
| --- | --- | --- | --- |
| AMD EPYC 9V45 | 4.249 | 25.596 | 16.352, 7.798 |
| AMD EPYC 7763 | 12.493 | 44.692 | 12.581, 12.507 |

The [9V45 comparison](https://github.com/jimeh/huterm/actions/runs/35547784317)
shows an improvement with remaining variation. The
[7763 comparison](https://github.com/jimeh/huterm/actions/runs/35548088830)
reproduces the original 33.4 ms budget failure and brings both candidate runs
back to baseline. Diagnostic workflow steps continue after benchmark failures
to collect every comparison arm; the workflow's overall success is not evidence
that every benchmark passed. These timings end at CPU paint encoding, not
physical display presentation.

A separate [GPUI trace](https://github.com/jimeh/huterm/actions/runs/35547291793)
on an Intel Xeon 8370C recorded about 15 ms inside native presentation, while
frame acquisition and GPUI's explicit previous-frame GPU wait took only
microseconds. Completed snapshots waited for that UI-thread call to return.
The X11 refresh timer also sometimes advanced over a missed 16 ms slot,
producing 32 ms callback intervals. This establishes the blocking boundary,
not the internal driver cost. Removing the benchmark's early redraw or deferring
snapshot starts until after drawing had not improved the earlier failure.

Display mode now admits one normal snapshot and at most one additional snapshot
for pending viewport work between observed frames. Output or link work alone
cannot use the extra allowance. Both allowances replenish only on a native
frame; visibility, one in-flight request, and coalesced pending scroll still
apply. Returning to live output also qualifies. This deliberately refines the
original one-snapshot policy instead of removing its bounds.

Three regression scenarios failed at their intended assertions under the old
policy, then passed with the viewport allowance. They exercise admission through
the real scroll controller, including completion, pending invalidation,
in-flight retries, hidden views, and further scroll requests without a frame.
The native refresh smoke additionally covers a single extra viewport request
while frames are paused and catch-up after resume.

After concurrent VM testing finished, the idle Mac15,8 comparison used the
built-in display (ID 1) with observed median callbacks near 8.33 ms. All release
builds completed before measurement, with separate target directories. Run order
was baseline, original PR, fixed, fixed, original PR, baseline.

| macOS revision | P95 input-to-matching-paint, two runs (ms) |
| --- | --- |
| Baseline `94f8028` | 9.429, 9.031 |
| Original PR `0a5d541` | 12.134, 11.646 |
| Fixed `24a7def` | 5.509, 9.564 |

The baseline carries the preimplementation benchmark harness; its relevant
runtime, desktop, and scroll benchmark source matches `c3ab933`. Each run had
70 snapshot samples and 65 to 69 matching paint samples. All presentation and
budget gates passed, with maximum in-flight and queued counts of one. The
original PR also incurred a smaller penalty on macOS. The fix returned results
to approximately the baseline range, with variation between runs; these two
runs do not establish a uniform 5.5 ms latency. Measurements taken during VM
load remain excluded.

On `24a7def`, native macOS refresh, input, desktop integration, and Quake smokes
passed.
The refresh smoke observed the bounded extra scroll request while frames were
paused, then the final coalesced viewport after resume. The integration smoke
also exercised the held-link acknowledgement added to prevent a late native
press from reaching a newly visible tab bar.
The Quake smoke passed all 40 animation cases and native focus/fullscreen
transitions. An earlier hosted focus timeout did not recur locally or in the
latest CI run; no Quake production code was changed.

### Frame-driven animations (2026-09-22)

Step 4 moves terminal and tab scrollbars, the resize indicator, visual bell,
tab scrolling, palette scrollbar, and tab reveal off the 16 ms pump. They share
the snapshot clock's window callback and one timer for the earliest animation
deadline. Static holds do not request frames. Notifications and render-time
layout changes arm animation state; destruction removes weak registrations. Snapshot
allowances and runtime activity handling are unchanged. The remaining pump
still handles pointer sampling, retries, palette availability, close, and
fullscreen coordination.

A native refresh smoke observes distinct intermediate resize-indicator opacities
through the production scheduler. On the Mac15,8 built-in display, it recorded:

| Physical display mode | Intermediate fade samples | Median animation interval | Median delivered callback interval |
| --- | ---: | ---: | ---: |
| 120 Hz | 40 | 9.069 ms | 8.845 ms |
| 60 Hz | 26 | 16.650 ms | 16.665 ms |

Both runs passed hold/fade completion, idle callback cleanup, bell expiry while
frames were paused, bounded scroll admission, and stale callback cleanup after
tab replacement. CoreGraphics mode 97 supplied 60 Hz; mode 96 at 120 Hz was
restored and verified. These measurements describe animation updates, not
physical display presentation or power consumption.

The fresh release baseline at `4723405` measured 7.942 ms p95 scroll input to
matching paint encoding. An initial step-4 run measured 17.008 ms, and another
paired comparison measured baseline 8.931/9.053 ms versus 16.624/13.332 ms.
A temporary scheduling trace found no active animation requesting frames in
this benchmark; that run measured 5.854 ms. The trace was removed. Initial
animation registration now schedules existing state directly instead of
notifying a redraw solely to register it.

An intermediate comparison preserved the baseline executable, completed both builds
before measurement, and ran baseline, step 4, step 4, baseline on the same
120 Hz display:

| Build | P95 input-to-matching-paint, two runs |
| --- | --- |
| Baseline `4723405` | 5.273 / 4.656 ms |
| Step 4 | 5.486 / 4.837 ms |

Every run passed the presentation and queue budgets. These runs each had
70 snapshot samples, 65 matching paint samples, and maximum in-flight and queued
counts of one.

After the render-time scheduling correction below, the final comparison ran in
the same baseline, step 4, step 4, baseline order without concurrent builds or
VM smokes. Baseline p95 measured 2.910 / 5.460 ms; step 4 measured
12.872 / 10.050 ms. All four runs passed the budgets, with 70 snapshot samples,
65 to 69 matching paint samples, and maximum in-flight and queued counts of one.
The latest pairs show a scroll-latency penalty despite earlier close pairs;
its cause remains unresolved. Do not claim latency neutrality or improvement
from this change. That difference prompted the repeat measurements below.
No new idle-CPU or power-saving claim is made; the recurring pump timer remains.

Controlled-time tests cover hold extension, fade completion, settled hover,
finishing a partial reveal during its dismissal hold, and fresh dismissal holds
after settled hover re-entry or renewed activity. These reveal boundary
regressions failed at their intended assertions before their fixes. The native
refresh checks also passed in a Tart macOS guest. The broader Linux desktop
smokes passed. The macOS VM suite encountered the previously observed Quake
visible-but-inactive focus timeout; its isolated rerun passed all 40 animation
cases and focus checks. No Quake production behavior was changed.

The final `mise run verify` passed, including 394 GPUI unit tests and 462 script
tests. Focused macOS and Linux fullscreen/tab-reveal reruns passed after the
partial-reveal correction and explicit animation rearming on tab activation.

A one-pixel native resize exposed a missing render-time wake: the indicator
activated without a grid resize or entity notification and stayed opaque. The
new regression failed on that opacity assertion before explicit scheduling
after layout changes, then passed. Tab layout also rearms after render-time
hover and drag-geometry changes; reveal no longer registers a second GPUI
animation callback from rendering.

### Animation review follow-up (2026-09-23)

At `106b57b`, drag release renews the scrollbar expansion hold even when no
animation ticks occurred during a long interaction. Deadline extensions also
reuse an earlier armed timer: that wake consults current work and rearms if
needed. Earlier required deadlines still preempt the timer, and removing the
last deadline cancels it. Both controlled-time regressions failed at their
intended assertions before the fixes.

After Time Machine finished, comparisons used the preserved `4723405` baseline
and the corrected `fbcb2d1` release binary on the same 120 Hz display. No build
or VM smoke ran during measurement. Background macOS services remained active;
the short-run set began with a one-minute load average of 14.37. The order was
baseline, branch, branch, baseline, then the same order again.

| Build | P95 input-to-matching-paint encoding, four short runs |
| --- | --- |
| Baseline `4723405` | 16.878 / 9.630 / 10.574 / 9.641 ms |
| Corrected branch `fbcb2d1` | 9.557 / 9.148 / 9.953 / 9.915 ms |

Each run supplied 70 post-warmup snapshot samples and 65 to 70 matching paint
samples. All budgets passed and maximum in-flight and queued counts stayed at
one. An additional longer collection used the unchanged workload and budget
checker with a temporary runner collecting 750 snapshots instead of 75. That
window includes the workload's live output after four seconds, so it is not a
like-for-like extension of the initial static portion. Baseline p95 was
6.254 / 9.895 ms; branch p95 was 10.335 / 9.931 ms. All budgets passed.

These repeats did not reproduce a consistent branch-specific latency penalty.
The earlier slower results remain recorded; host and UI reply-delivery timing
still limit attribution. This is evidence against the earlier large consistent
gap, not proof of identical latency or a speedup. The timer change fixes churn
in real scroll/hover activity; the synthetic driver does not activate scrollbar
holds, so its measurements cannot establish a gain from timer reuse.

`mise run verify` passed at `fbcb2d1`, including 396 GPUI tests and 463 script
tests. The post-fix native refresh smoke passed hold/fade, same-grid resize,
paused-frame expiry, bounded scroll admission, and stale callback cleanup. It
recorded 48 intermediate fade samples at a median 8.299 ms, with delivered
callbacks at 8.328 ms. The built-in display remains at 120 Hz.

The review follow-up also wires this native refresh smoke into the macOS CI
job and the default VM smoke set. `mise run vm:macos:smoke -- macos-refresh`
passed with the new task routing. The supervised `ci:smoke:step` entrypoint
also passed in the VM with `HUTERM_CI_SMOKE_STEP=macos-refresh`, and
`mise run ci:workflows` passed. Cadence budgets remain opt-in for explicitly
selected physical display modes.

### Pointer and pending-work checkpoint (2026-09-23)

Baseline: merged `main` at `42117b6`, before steps 5 and 6. The native Mac15,8
was unlocked with one connected built-in display, maximum 120 Hz, scale 1,
and logical dimensions 2294 by 1490. Delivered callbacks were approximately
120/s with 8.33 ms median intervals. The initial load average was about 6;
normal desktop applications remained active. Builds finished before serial
measurement, with no concurrent agent builds or VM smokes.

Three 12-second echo and flood runs used the existing release workload and
runner. First intervals were excluded by the runner. Echo applied medians were
107 / 167 / 134 microseconds; paint-encoding medians were
5.449 / 6.924 / 6.200 ms. Flood delivered 120 snapshots/s in each run, with
paint-encoding medians of 14.916 / 14.721 / 14.615 ms. These are elapsed pipeline
measurements, not CPU utilization or physical presentation latency.

Three scroll runs passed every snapshot, presentation, and queue budget. P95
input-to-matching-paint encoding was 8.741 / 6.524 / 3.846 ms. Maximum in-flight
and queued requests stayed at one. This baseline variability limits claims
based on small timing differences.

The renderer scenarios ran three times each. Median preparation/paint encoding
was 13.0 / 533.3 microseconds for scrolling (one rebuilt row),
189.6 / 595.2 microseconds for churn, and 142.4 / 1030.0 microseconds for boxes.
The native animation smoke passed every marker with its explicit 120 Hz budget.

Idle measurements used a temporary startup-only hook to create tabs through the
normal path, waiting for each tab's initial snapshot before creating the next.
The hook ended before sampling and was removed from source after building a
separate probe executable. No continuous frame observer ran in this probe.
Process labels were disabled. Each entry is the median of three five-second
samples after two seconds of settling; hidden means AppKit-hidden.

| Tabs | State | CPU, % of one core | Interrupt wakeups/s | Resident MiB |
| ---: | --- | ---: | ---: | ---: |
| 1 | Visible | 1.741 | 179.818 | 101.25 |
| 1 | Hidden | 0.795 | 59.939 | 100.86 |
| 10 | Visible | 1.750 | 180.341 | 120.80 |
| 10 | Hidden | 0.772 | 59.947 | 120.41 |
| 50 | Visible | 1.778 | 180.021 | 194.02 |
| 50 | Hidden | 0.908 | 60.147 | 193.58 |

CPU counters include the Mach timebase conversion. Resident memory includes the
process and its native libraries, not child-shell memory. These observations do
not establish power consumption or attribute memory to individual subsystems.

Raw logs, JSON reports, preserved release binaries, and profiling artifacts are
local under `target/bench/pending-work-2026-09-23/`. A separate Metal System Trace
captured 1,377 target-process drawable presentation requests over 11.702 seconds,
with median/p95 intervals of 8.341/9.540 ms. These requests confirm approximately
120 Hz submission under instrumentation; they do not measure physical display
latency or GPU utilization. The recorder exited 54 after terminating its launched
process at the capture limit, but the saved trace exported successfully.

The Allocations instrument failed to attach to the target process. Its trace is
not valid allocation evidence. Resident-memory measurements above remain valid;
allocation-rate attribution is still pending.

### Pointer and pending-work implementation (2026-09-23)

Production revision: `425d51d`, compared with the preserved `42117b6` binaries
above. The window pump now only reconciles fullscreen state. Pointer events and
window changes update reveal intent; each terminal wakes its own pending-work
task only when needed. The 16 ms retry interval is a busy-runtime backstop, not
a rendering cadence. The opt-in scroll workload retains its timed drive loop.

The serial 120 Hz release runs passed all output and scroll budgets:

| Measurement | Baseline, three runs | Feature, three runs |
| --- | --- | --- |
| Echo applied median, microseconds | 107 / 167 / 134 | 131 / 137 / 140 |
| Echo paint median, ms | 5.449 / 6.924 / 6.200 | 6.354 / 5.729 / 6.239 |
| Flood snapshots/s | 120 / 120 / 120 | 120 / 120 / 120 |
| Flood paint median, ms | 14.916 / 14.721 / 14.615 | 13.952 / 13.952 / 14.025 |
| Scroll input-to-paint p95, ms | 8.741 / 6.524 / 3.846 | 4.218 / 6.835 / 8.278 |

Scroll queues remained bounded at one in-flight and one queued request. The
renderer scenarios remained similar: scroll preparation/paint encoding was
13.0/541.4 microseconds, churn 194.1/585.2, and boxes 143.3/1014.1. The feature
runs began with load averages around 11 after builds and VM checks had stopped,
versus about 5 at baseline. These data support preserved pacing and latency
budgets; they do not establish a visual-latency improvement.

The first idle sequence stopped when the Mac locked before the ten-tab sample.
Its one-tab samples are excluded from the comparison because the lock boundary
was not recorded. The repeated comparison checks the lock state before and after
each run and alternates baseline/feature order for one and fifty tabs.

The repeated unlocked comparison used the same three five-second samples per
state. One-tab runs used baseline then feature; fifty-tab runs reversed that
order. Builds and VMs had stopped; normal desktop applications remained active.
A process-scoped `caffeinate` assertion kept the display awake during this run.
The initial load average was 14 after the VM, so absolute CPU values should not
be compared with another machine or an otherwise idle host.

| Tabs | State | Baseline CPU, % core | Feature CPU, % core | Baseline MiB | Feature MiB |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | Visible | 1.323 | 0.976 | 102.12 | 102.55 |
| 1 | Hidden | 0.592 | 0.594 | 101.69 | 102.19 |
| 50 | Visible | 1.667 | 1.237 | 193.53 | 194.34 |
| 50 | Hidden | 0.876 | 0.773 | 193.05 | 193.92 |

Visible idle CPU fell 26.2% with one tab and 25.8% with fifty tabs in these pairs,
about 0.35 and 0.43 percentage points of one core respectively. Hidden one-tab
CPU was unchanged; hidden fifty-tab CPU fell 11.8%. Interrupt wakeups remained
approximately 180/s visible and 60/s hidden in both builds. The fullscreen timer
and GPUI display link remain, so this change reduces work per wake rather than
eliminating those wakes. Resident memory was 0.43 to 0.87 MiB higher in these
samples; the new task/channel contribution was not isolated. There is no measured
memory saving.

The ten-tab feature checkpoint was 1.412% visible / 0.690% hidden CPU,
179.816 / 59.994 interrupt wakeups/s, and 120.67 / 120.25 MiB resident. It was
not paired with a fresh ten-tab baseline and is included only as a scaling
checkpoint. The earlier ten-tab baseline is recorded above.

The final physical-display refresh smoke passed every marker and the explicit
12,000-microsecond animation budget, with 50 intermediate fade samples and an
8,328-microsecond median interval. No fixed 120 Hz scheduling interval was added.

Verification includes the full `mise run verify` gate, Linux fullscreen, palette,
and input smokes, and macOS refresh, desktop integration, fullscreen, palette,
and Quit smokes. The new native pending-work regression queues input and
controls while
frames are paused, requires a shell title acknowledgement, preserves snapshot
allowances, and verifies task cancellation after view release. Disabling the
admission wake made it fail at the expected pending-work assertion; restoring
the wake made it pass.

### Reproducible idle checkpoint tooling

`mise run bench:idle` builds the release `idle_bench` example and measures one
and fifty tabs **per window**, with one and two windows, visible and AppKit-hidden.
It requires an unlocked macOS console session. Close build/VM workloads before
collecting comparison evidence, and preserve the same desktop/display conditions.
The task respects `CARGO_TARGET_DIR`; use separate baseline and feature targets:

```sh
CARGO_TARGET_DIR=target/fullscreen-baseline mise run bench:idle -- \
  --duration 5 --repeats 3 --settle 2 --output target/bench/fullscreen-baseline.json
```

Startup uses production `new_tab` and `new_window` commands, waits for each
fixture shell's initial snapshot, and asserts installed native fullscreen
adapters and no Quake ownership. Its startup task ends at the readiness line.
The fixture shell blocks in `read`; configuration uses `tabs.label = "title"`
and no Quake profiles. Inherited `HUTERM_*` instrumentation is removed unless
explicitly supplied in an arm's `env`. The sampling interval has no Huterm control
commands or recurring benchmark state probe. Settling is an excluded interval,
not proof of startup readiness.

To compare preserved binaries without rebuilding, pass `--arms arms.json`.
Each arm requires `label`, `executable`, and the exact source `revision`; optional
`env` supplies development switches. Paths are relative to the working directory.
For example, replace the revision placeholders with the recorded commit SHAs:

```json
[
  {"label":"baseline","executable":"target/fullscreen-baseline/release/examples/idle_bench","revision":"BASELINE_SHA"},
  {"label":"feature","executable":"target/fullscreen-feature/release/examples/idle_bench","revision":"FEATURE_SHA"}
]
```

Run `mise run bench:idle -- --arms arms.json --output target/bench/paired.json`.
Arm order reverses across repetitions and scenarios. `--tabs 1,50`, `--windows
1,2`, `--duration`, `--repeats`, and `--settle` select the matrix. A third arm can
reuse a binary with a different explicit `env` to isolate a development switch.

The report retains executable hashes, revision labels, runner dirt, configuration,
load averages, host/display context, startup window scale/bounds, and per-sample
CPU, interrupt wakeups, resident memory, and thread counts. CPU uses Mach timebase
conversion; 100% is one logical core. RSS and CPU exclude child shells.
A zero display refresh rate means the OS did not report a fixed rate.
Visible means AppKit-unhidden; two overlapping windows need not both be
unoccluded.
The sampler checks console lock state before and after each interval and rejects
samples on a lock notification during it. A process-scoped `caffeinate` assertion
prevents idle display sleep, not deliberate locking. Invalid samples remain marked
invalid in the report, and the run stops. Per-process logs, native sampler, and
cleanup results remain in the report's artifact directory. Cleanup requests native
Quit, waits up to thirty seconds for multi-terminal cleanup, then reports failure
and kills only that fixture PID.

### Fullscreen scheduling checkpoint (2026-09-23)

The fresh production baseline is `f3ec40e645293f68da952d77320db56406e874b0`,
recorded before scheduler edits. `target/bench/fullscreen-baseline-complete.json`
contains 24 valid unlocked samples and twelve clean native Quit results, using
three five-second samples per case after two seconds of settling. The built-in
display reported 120 Hz and window scale 1. Initial host load averages were
10.16 / 12.39 / 10.01; ordinary desktop applications remained active.

| Windows | Tabs per window | State | CPU, % of one core | Interrupt wakeups/s | Resident MiB | Threads |
| ---: | ---: | --- | ---: | ---: | ---: | ---: |
| 1 | 1 | Visible | 1.558 | 179.649 | 101.47 | 10 |
| 1 | 1 | Hidden | 0.824 | 60.083 | 101.56 | 9 |
| 1 | 50 | Visible | 1.518 | 180.090 | 193.62 | 206 |
| 1 | 50 | Hidden | 0.865 | 60.100 | 193.64 | 205 |
| 2 | 1 | Visible | 2.192 | 300.085 | 106.25 | 16 |
| 2 | 1 | Hidden | 0.889 | 61.542 | 106.27 | 13 |
| 2 | 50 | Visible | 1.866 | 300.813 | 286.95 | 408 |
| 2 | 50 | Hidden | 0.906 | 59.692 | 286.92 | 405 |

These are process counters, not attribution to particular timers. Two windows
can share wakeups; removing two timers does not imply twice the wakeup reduction.
`target/bench/fullscreen-checkpoint-2026-09-23/` retains the baseline executable
hashes and three echo, flood, and scroll runs. All scroll budget checks passed,
with maximum in-flight and queued requests both one. The completed idle and
latency comparisons follow below.

The comparison executable built at `1d589d1` supported the temporary old-pump
switch used for the three-arm measurement below. The final source removes both
the legacy loop and its switch. Normal idle arms leave diagnostics disabled.

An explicit `force_fallback: true` arm reuses the smoke-only adapter construction
skip. Startup validates missing adapters and fallback eligibility in every window,
then reports the actual adapter count; only this arm accepts zero. Its startup task
ends before sampling, without the fullscreen smoke's recurring state writer.
The smoke seam enables pass/timer counters, so fallback measurements include that
small instrumentation difference and must be reported separately.

Fullscreen correctness uses the pump-off smoke. The focused native retry path is:

```sh
mise exec -- bun scripts/check-fullscreen.ts \
  target/debug/examples/fullscreen_smoke --scheduler-only
```

`--fallback-only` exercises the deliberately missing adapter. Normal smoke runs
include both paths. Actual native refit attempt intervals must each be at least
16 ms, including a fresh notification during a pending retry. The read-only state
probe checks settled task counters and absence of an armed fullscreen timer.
The fallback probe acknowledges AppKit Did notifications independently before
issuing its opposite external toggle, because style-mask changes can precede
animation completion. That acknowledgement never wakes the production scheduler.

## Fullscreen three-arm comparison (2026-09-23)

The unlocked native host used the same 120 Hz display for baseline `f3ec40e` and
feature `1d589d1`. Each cell below is the median of three five-second samples.
The 72 accepted samples exclude six opening samples that overlapped script tests;
a replacement run supplied those six samples. Raw reports and exclusions are in
`target/bench/fullscreen-three-arm-summary.json`, with source reports named there.
CPU is percent of one logical core; wakeups are process interrupt wakeups/second.

| Windows | Tabs/window | Visibility | CPU baseline / pump on / pump off | Wakeups baseline / pump on / pump off |
| --- | --- | --- | --- | --- |
| 1 | 1 | visible | 1.458% / 1.296% / 0.660% | 180.5 / 179.9 / 121.8 |
| 1 | 1 | hidden | 0.695% / 0.670% / 0.025% | 60.1 / 59.9 / 1.6 |
| 1 | 50 | visible | 1.395% / 1.537% / 0.711% | 179.9 / 179.8 / 121.6 |
| 1 | 50 | hidden | 0.637% / 0.683% / 0.016% | 60.3 / 60.1 / 1.4 |
| 2 | 1 | visible | 2.016% / 1.972% / 1.123% | 300.1 / 300.0 / 242.0 |
| 2 | 1 | hidden | 0.775% / 0.907% / 0.018% | 60.1 / 60.3 / 1.4 |
| 2 | 50 | visible | 1.992% / 1.936% / 1.064% | 321.5 / 300.4 / 241.9 |
| 2 | 50 | hidden | 0.967% / 0.851% / 0.024% | 59.9 / 60.3 / 1.4 |

Removing the pump reduces visible CPU 44–55% and hidden CPU 96–98% versus the
baseline. Hidden wakeups fall from about 60/s to 1.4–1.6/s. Visible wakeups retain
the display-link cost. RSS remains within about 1 MiB of baseline (102–287 MiB),
and thread counts differ by at most two, with the same scaling by terminal count.
The pump-on control retains approximately baseline wakeups, supporting attribution
to removal of the timer rather than the new reconciliation body alone.

These idle measurements do not establish input latency or physical display
presentation performance.

### Final latency and fallback checks

The final pump-free build at `44efdf9` was compared with preserved baseline
`f3ec40e` in three alternating rounds per workload. Echo and flood runs lasted
12 seconds each; scroll used its existing sample and queue-completion gates.
Logs and executable hashes are in `target/bench/fullscreen-paired-latency/`.
The table gives medians across the three run summaries, not pooled percentiles.

| Metric | Baseline | Feature |
| --- | ---: | ---: |
| Echo output-to-applied snapshot, median | 116 µs | 105 µs |
| Echo output-to-paint marker, median | 4,965 µs | 5,417 µs |
| Flood snapshots/second | 118 | 120 |
| Flood output-to-paint marker, median | 14,526 µs | 14,624 µs |
| Scroll input-to-paint marker, run p95 | 9,857 µs | 8,854 µs |

All six echo runs passed the 5,000 µs applied-snapshot budget. All six scroll
runs passed snapshot, paint, presentation, latency, and queue gates, with maximum
in-flight and queued requests both one. Scroll p95 ranged from 5,509–10,625 µs
in baseline and 7,340–9,784 µs in feature. These overlapping results support
retaining current pacing; they do not establish a latency improvement. Paint
markers are CPU-side observations, not physical presentation timestamps.

A separate final-build comparison used one window and one tab, with three
five-second samples per visible/hidden state and alternating normal/fallback
order (`target/bench/fullscreen-fallback.json`). Normal versus forced-fallback
CPU was 0.690% versus 1.485% visible, and 0.015% versus 0.814% hidden. Wakeups were
121.8 versus 180.0/s visible and 1.6 versus 60.3/s hidden. The fallback includes
smoke counters as noted above. It intentionally retains polling; normal windows
assert successful observer installation and never enter that path.

`mise run verify` passed 596 Rust tests and 468 script tests, with three ignored
Rust benchmarks. Native fullscreen, Quake, refresh, Quit, and presentation-query
checks passed; Docker Linux fullscreen, Quake, and presentation-query checks
passed. The native refresh smoke measured an 8,325 µs median frame interval.
An isolated captured-revision perturbation bypassing the refit deadline failed
at the intended assertion with 42, 23, and 22 µs retry intervals. Restoring the
source made the same smoke pass. The strengthened fallback smoke also proves
repeated idle timer firings before external native transitions and Quit disarm.

Physical notch checks passed on the built-in display. Multi-display migration,
hotplug, and a native 60 Hz run remain untested for this change. Remaining polling
includes the explicit no-adapter compatibility sampler, separate Quake pump,
conditional macOS pointer probe, busy-runtime retries, and GPUI display-link work.

## Quake event scheduling comparison (2026-09-24)

PR #159 removes the separate global 16 ms Quake pump. The production baseline
is `6200a9c`, before scheduler changes; the feature binary is `8e6ed85`. Both
were rebuilt with the identical `idle_bench.rs` fixture from `8e6ed85`, using
separate target directories. The baseline has no other source changes. Fixture
hash, baseline patch, executable hashes, reports and per-process logs are retained
under `target/bench/quake-checkpoint-2026-09-23/`.

The fixture opens genuine Quake profiles with persistent shells and
`hide_on_focus_loss=false`. It initializes each tab before measuring. During
initialization only, it grants bounded snapshot credit through normal admission:
AppKit sometimes reports a visible/key window as occluded and withholds frames,
which stalled both baseline and feature startup. This credit ends before READY
and does not change production pacing. The benchmark therefore does not validate
production cold-start frame delivery under occlusion.

Each sample measures five seconds after two seconds of settling. Two rounds
alternate baseline/feature order across configurations and reverse it in the
second round. The Mac15,8 M3 Max host ran macOS 27 with its built-in display at
120 Hz, 2294 × 1490 logical points, scale 1. There were no concurrent builds or
native smoke tests. Background host load remained variable: one-minute load
ranged from 11.1–33.9 during ordinary controls and 26.1–46.6 during the stricter
Quake run. CPU is a percentage of one logical core, not whole-machine use.

The final comparison uses 32 ordinary-window samples from `paired-final.json`
and 32 Quake samples from `paired-strict-quake.json`. All were valid and all
processes exited through native Quit with code zero, without forced cleanup.
The latter run uses sampler revision `bb036d2`, which requires every requested
Quake window ID to exist and belong to the process before and after sampling.
It replaces the initial Quake samples, whose hidden-window check could accept a
missing ID. Smoke instrumentation is disabled in every performance arm.

### Quake idle results

Values are means of the two samples per arm and configuration.

| Windows | Tabs/window | Visibility | CPU baseline / feature | Wakeups/s baseline / feature |
| --- | --- | --- | --- | --- |
| 1 | 1 | visible | 2.171% / 1.206% | 179.3 / 122.8 |
| 1 | 1 | hidden | 0.994% / 0.099% | 60.5 / 2.0 |
| 1 | 50 | visible | 1.842% / 0.958% | 180.4 / 123.1 |
| 1 | 50 | hidden | 1.040% / 0.087% | 60.1 / 2.3 |
| 2 | 1 | visible | 2.580% / 1.734% | 300.0 / 244.3 |
| 2 | 1 | hidden | 0.923% / 0.091% | 60.0 / 2.3 |
| 2 | 50 | visible | 2.438% / 1.572% | 300.3 / 243.9 |
| 2 | 50 | hidden | 1.239% / 0.098% | 59.8 / 2.1 |

Hidden Quake CPU falls 90–92% and interrupt wakeups fall about 96%, from roughly
60/s to 2.0–2.3/s. Visible CPU falls 33–48%; visible wakeups fall 19–32% but retain
GPUI's display-link cost. The visible, non-fullscreen macOS work-area fallback
still samples once per second per Quake window because public notifications do
not cover every external Dock change. Healthy hidden owners have no periodic
Quake work.

Quake RSS spans about 110–433 MiB in baseline and 110–434 MiB in feature. Mean
RSS differences per configuration are below 1.5 MiB, and mean post-sample thread
counts differ by at most one. These measurements support no material memory or
thread-count improvement. They also do not establish physical input latency.

### Ordinary-window controls

| Windows | Tabs/window | Visibility | CPU baseline / feature | Wakeups/s baseline / feature |
| --- | --- | --- | --- | --- |
| 1 | 1 | visible | 0.852% / 0.859% | 121.4 / 121.5 |
| 1 | 1 | hidden | 0.031% / 0.030% | 1.4 / 1.4 |
| 1 | 50 | visible | 0.900% / 0.844% | 121.5 / 121.8 |
| 1 | 50 | hidden | 0.035% / 0.065% | 1.6 / 1.7 |
| 2 | 1 | visible | 1.199% / 1.298% | 241.8 / 242.5 |
| 2 | 1 | hidden | 0.049% / 0.042% | 1.6 / 1.9 |
| 2 | 50 | visible | 1.394% / 1.218% | 242.2 / 242.3 |
| 2 | 50 | hidden | 0.055% / 0.075% | 2.1 / 2.0 |

Ordinary wakeups remain approximately 121–122/s with one visible window and
242/s with two. Hidden controls stay around 1.4–2.1/s. CPU varies in both
directions, including a 0.10 percentage-point increase for two visible one-tab
windows; it does not show a consistent regression. The unchanged ordinary
terminal paths and overlapping individual samples support retaining the current
pacing. The short runs and variable host load do not establish exact CPU parity.

### Animation, latency and verification

The final native 40-case Quake smoke includes intermediate slide/fade states,
reversal, focus return, panel then app-switch behaviour, profile conversion,
retained PTYs and cleanup. Its trace contains 8,984 consecutive within-transition
Animate effect intervals below 100 ms, with a median of 8,333 µs; 8,756 are below
12 ms. These are CPU-side native-effect timestamps, not physical presentation
measurements. Docker X11 passed the same 40-case matrix, including mandatory
intermediate geometry and compositor alpha checks.

Two alternating baseline/feature rounds of ordinary echo, flood and scroll
passed on feature ancestor `f83e1ce`. Later production changes are confined to
Quake scheduling and opt-in diagnostics, so that evidence carries forward:

| Metric | Baseline run summaries | Feature run summaries |
| --- | --- | --- |
| Echo output-to-applied snapshot, median | 185 / 134 µs | 202 / 131 µs |
| Echo output-to-paint marker, median | 4,706 / 6,686 µs | 6,576 / 5,916 µs |
| Flood snapshots/second | 115 / 120 | 120 / 120 |
| Flood output-to-paint marker, median | 14,986 / 15,066 µs | 13,827 / 14,396 µs |
| Scroll input-to-paint marker, p95 | 9,904 / 8,501 µs | 8,956 / 6,300 µs |

All four scroll runs passed snapshot, paint, presentation, latency and queue
budgets, with maximum in-flight and queued requests both one. These small
comparisons show no material regression; they do not establish a statistically
significant latency improvement.

`mise run verify` passed on the initial implementation. Subsequent lifecycle
corrections passed 43 focused macOS and 42 focused Linux Quake tests, Clippy,
and both complete native smoke matrices. The paused-reversal regression failed
at its intended assertion before the fix. Final script checks passed 477 tests.
Idle smokes also reject self-wake loops and stale native display-link callbacks.
They wait for actual timer disarm as well as expired policy deadlines before
observing idle work; a hosted Linux failure exposed the distinction.
A fresh universal package at `0ed2dd9` verified both executable slices and the
bundle minimum as macOS 14.0; later changes do not alter packaging policy.

Physical 60 Hz, multi-display migration, hotplug, external Dock changes, native
Intel, and macOS 14 runtime coverage remain unavailable for this change. No VM
was used. Real platform observer-registration failures were not injected end to
end. Explicit failure backoff and X11 safety sampling remain; the native
fullscreen compatibility sampler, conditional pointer probe, busy-runtime
retries and GPUI display-link work are separate owners.

## Snapshot rebuild cost (2026-09-25)

This section covers the work for [#162](https://github.com/jimeh/huterm/issues/162),
planned in [snapshot rebuild cost](../plans/snapshot-rebuild-cost.md). The
baseline is `5be9643`, with the extended fixtures from `8ceec74`; the feature
build is `212dfdf`.

### Snapshot rebuild engine results

`mise run bench:engine` ran natively on a Mac15,8 M3 Max host with macOS 27, on
a 120 by 40 grid for 200 iterations per fixture. The feature column is the
median of three runs' p50 values. Row counts are totals over 200 snapshots.
Extracted rows are read from Ghostty; allocated rows receive a new `Arc`.

| Fixture | Snapshot p50 before | After | Rows extracted before | After | Row `Arc`s allocated after |
| --- | ---: | ---: | ---: | ---: | ---: |
| `ascii` | 181.5 µs | 134.9 µs | 8,000 | 8,000 | 39 |
| `styled` | 183.2 µs | 118.1 µs | 8,000 | 8,000 | 7,800 |
| `unicode` | 185.2 µs | 133.6 µs | 8,000 | 8,000 | 39 |
| `sparse` | 5.5 µs | 4.0 µs | 201 | 201 | 200 |
| `full` | 200.9 µs | 131.3 µs | 8,000 | 8,000 | 8,000 |
| `prompt` | 176.4 µs | 7.4 µs | 8,000 | 401 | 1 |
| `title` | 176.4 µs | 4.1 µs | 8,000 | 201 | 200 |
| `osc8` | 177.5 µs | 129.6 µs | 8,000 | 7,923 | 200 |
| `box-cjk` | 176.0 µs | 3.9 µs | 8,000 | 201 | 200 |
| `color` | 177.5 µs | 128.5 µs | 8,000 | 8,000 | 1 |
| `scroll` | 180.4 µs | 135.4 µs | 7,532 | 7,532 | 600 |
| `scroll-capped` | 179.2 µs | 131.5 µs | 8,000 | 8,000 | 600 |

Prompt, title, and box-drawing or CJK fixtures no longer probe palette
overrides, so they extract only the rows they touch. The OSC 8 fixture still
extracts almost every row; the override hint does not flag it, so the damage
comes from Ghostty itself, and the cause is not established. Full rebuilds fell
by about 30% from per-cell `CellText` storage. Scrolling fixtures now reuse the
`Arc` for every unchanged shifted row, including at the 16 MiB scrollback
budget, where history size stops growing. Row matching adds roughly 10-20 µs to
full rebuilds on the runtime thread; before, the renderer compared the same
rows on the UI thread and rebuilt every prepared row once scrollback was full.

A `sample` profile of repeated `full` snapshots attributed about 60% of
snapshot time to Ghostty getters and their binding wrappers (`row_cells_get`,
`cell_get`, `style`, `content_tag`, `wide`, `codepoint`) and the rest to Rust
cell construction. Batched reads through `row_cells_get_multi` would need a
vendored binding patch and are deferred.

### Snapshot rebuild renderer results

`bench:renderer-scenarios` ran in the Linux arm64 Docker container under Xvfb
and `twm`, five runs per scenario, against a baseline built from `d70366a`
(before `CellText`). Scenarios ran one at a time with up to three attempts,
because Xvfb intermittently delivered too few frames while the host was loaded.

| Scenario | Prepare median | Change | Paint median | Change |
| --- | ---: | ---: | ---: | ---: |
| `ascii` | 194.3 µs | -0.8% | 432.8 µs | -1.6% |
| `blocks` | 214.5 µs | -0.6% | 237.7 µs | -1.4% |
| `boxes` | 177.5 µs | +2.3% | 996.3 µs | -3.7% |
| `churn` | 243.7 µs | +11.5% | 557.1 µs | -3.1% |
| `scroll` | 13.7 µs | -14.6% | 444.1 µs | -5.0% |
| `selection` | 196.8 µs | +0.3% | 489.5 µs | -4.3% |

The first `CellText` build raised full-rebuild prepare by 31-41%: `as_str`
validated inline bytes on each of several reads per cell, about 2 ns against
0.4 ns for a `String`. Borrowing single ASCII bytes from a static table and
keying the renderer's layout cache by scalar removed that regression.

`churn` prepare remains about 15% slower in two further alternating
baseline/feature pairs (214/213 µs against 246/246 µs), with identical rebuilt
rows and cache hits. Isolated release micro-benchmarks of the renderer's per-cell
text path were faster than the old path on both macOS (102 against 116 µs per
8,000 cells) and Linux (67 against 91 µs), and `row_sources` costs about 1 µs.
The remaining difference is unexplained. Most of the per-cell time in those
micro-benchmarks is SipHash lookups in the non-ASCII scalar layout map, a
follow-up for [#164](https://github.com/jimeh/huterm/issues/164).
