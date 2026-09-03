# GPUI terminal renderer measurements

Measured on 2026-09-03 using Linux 6.8.0 x86_64, an AMD Ryzen 5 5600GT with
six available cores, Rust 1.98.0, GPUI 0.2.2, and Mesa's Lavapipe Vulkan driver
under Xvfb and `twm`. All HUTerm builds used Cargo's release profile.

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
size-check screens, HUTerm's first 100 by 32 frame measured 2,652 microseconds
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
sustained visible FPS. A macOS run of the same workload remains necessary to
compare displayed throughput with Ghostty and Terminal.app.

## Reproduction

Install the Linux desktop dependencies from the development guide, including
`twm`, then run:

```sh
mise run bench:renderer
```

The task builds HUTerm and its workload in release mode, launches a 100 by 32
animated grid under Xvfb, and prints `huterm-render` timing lines. Setting only
`HUTERM_RENDER_STATS=1` while running HUTerm enables rolling renderer counters
on platforms that deliver continuous animation frames.
