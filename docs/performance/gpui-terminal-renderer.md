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
write is an isolated update like an echoed keystroke. Pass `-- flood` for the
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
