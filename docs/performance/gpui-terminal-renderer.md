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
