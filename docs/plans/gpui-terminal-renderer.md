# GPUI terminal renderer performance plan

## Outcome

Replace the current per-cell `shape_line` and `ShapedLine::paint` path with a
stateful terminal renderer that retains compact glyph layouts and prepared rows
while continuing to use GPUI's public scene APIs.

The immediate target is GPUI-thread CPU work. This establishes a usable
renderer baseline for later `libghostty-vt` and direct-GPU experiments without
changing PTY ownership or the client protocol.

## Scope

This slice retains:

- Alacritty as the canonical terminal engine.
- Full immutable `TerminalSnapshot` messages.
- The existing 16 ms event pump and snapshot coalescing.
- GPUI as the window, layout, input, and compositor owner.

It changes `huterm-gpui` and makes one focused `huterm-core` correction:
snapshot construction obtains Alacritty's `RenderableContent` once per
snapshot, not once per cell.

It does not add `wgpu`, `libghostty-vt`, raw terminal streaming, or a new public
renderer abstraction. Those changes need separate measurements and decisions.

## Baseline and attribution

The existing release trace at 100 by 32 cells measured about 208 ms of CPU
frame preparation, including about 202 ms inside per-cell `shape_line` calls.
GPUI already caches `LineLayout` for two frames, so that timing includes its
cache lookup, repeated font resolution, string allocation, construction of a
roughly 3 KiB `ShapedLine` value, and related per-cell overhead rather than
pure font shaping.

The external DOOM-fire workload emits roughly 2,400 frames per second directly
to a 100 by 32 PTY on the same host. It is not the limiting component. A
repository-owned animated-color workload and opt-in renderer timings make the
new result repeatable without importing GPL code.

Linux uses the generic `monospace` font family rather than requesting macOS-only
Menlo and repeatedly traversing GPUI's missing-font fallback path.

## Design

`TerminalView` owns an `Rc<RefCell<TerminalRenderer>>`. Each render clones that
handle into the canvas prepaint and paint closures. Prepaint updates retained
state and returns no frame-sized value; paint reads the retained rows.

`TerminalRenderer` contains:

- The `Arc<TerminalSnapshot>` used for the previous preparation.
- Bounds-relative `PreparedRow` values.
- A two-generation `GlyphLayoutCache`.
- One baseline calculated from the configured primary font.
- Optional CPU timing counters enabled by `HUTERM_RENDER_STATS=1`.

During prepaint:

- `Arc::ptr_eq` avoids all work for the same snapshot.
- A new grid size rebuilds every row.
- Otherwise, actual row slices are compared. An unchanged indexed row reuses
  its prepared backgrounds, glyph layouts, and decorations.
- A changed row is rebuilt from new cells.
- Cursor state is updated independently and never invalidates a row.
- Snapshot cells are traversed with `chunks_exact`; malformed trailing content
  cannot produce an out-of-bounds slice.

Indexed row retention targets shells and partially updating TUIs. It does not
improve full-screen animation or scrolling, where every indexed row changes;
the glyph cache handles repeated layouts in those workloads.

Prepared geometry stores row and column coordinates, never canvas origins.
Current bounds are applied only while painting, so a sub-cell resize or origin
change does not invalidate rows.

## Layout cache

The renderer calls public `WindowTextSystem::layout_line` and stores the
returned `Arc<LineLayout>`, not `ShapedLine`. Entries are keyed by text plus
bold and italic font variants. Foreground color, dimming, underline, and
strikeout remain paint-time state because they do not affect layout.

Single-scalar text uses a compact character key. Combining sequences use an
owned string only on insertion. A current/previous generation policy retains
layouts used in the two most recent preparations and bounds unused cache state
without per-hit LRU allocation. Prepared rows retain the layouts they still
display independently of cache eviction.

Cache policy and row-diff decisions are pure logic. Layout creation is injected
through a closure, so focused tests do not need GPUI's `test-support` feature or
a native window.

## Painting semantics

Painting is globally layered:

1. Paint backgrounds for every row.
2. For each row, open one row-sized clip layer and paint cached glyph IDs with
   `Window::paint_glyph` or `Window::paint_emoji`.
3. Paint merged underline and strikeout runs using one grid-wide baseline.
4. Paint the cursor last.

The fixed baseline comes from the primary font rather than varying with each
fallback glyph. Row clipping replaces thousands of per-cell `paint_layer`
calls while preventing overhanging glyphs from bleeding vertically. Wide
spacer cells never emit glyphs; a wide leading cell spans two columns for
decorations. Hidden cells emit no glyphs. Blank cells without decorations stay
absent from prepared glyph data.

The existing translucent cursor remains a deliberate limitation: block cursors
do not yet invert the covered glyph.

## Measurement harness

An example workload emits a continuously changing 100 by 32 colored grid. The
`bench:renderer` Mise task builds it and Huterm in release mode, runs both under
Xvfb, and enables `HUTERM_RENDER_STATS`.

Statistics report wall-clock frame count and CPU time spent preparing retained
state and encoding paint primitives, plus changed rows and cache hits/misses.
They do not claim GPU timing or portable throughput: Xvfb and Mesa software
rendering are suitable for repeatable CPU-path evidence, not GPU comparison.
The before-and-after figures and host details are recorded under
`docs/performance/`.

## Implementation sequence

1. Record the existing trace, direct-workload capacity, and environment caveats.
2. Correct the per-cell `RenderableContent` construction in snapshot creation.
3. Add the two-generation compact layout cache with focused policy tests.
4. Add bounds-relative prepared rows and pure invalidation tests.
5. Replace per-cell shaped-line painting with direct, globally layered painting.
6. Add the opt-in timing counters and repository-owned animated workload task.
7. Run focused tests, project checks, Linux desktop smoke, and broad verification.
8. Run the release benchmark and record the comparison.

## Verification

Automated evidence:

- Cache hits do not invoke the layout closure again.
- Rotating generations evicts layouts unused for two preparations.
- Identical snapshots rebuild no rows.
- One changed cell rebuilds only its row.
- Grid-size changes rebuild every row.
- Existing unit and PTY integration tests remain green.
- `mise run verify` passes.

Runtime evidence:

- `mise run smoke:linux` keeps the desktop client alive for its full smoke
  window and shuts down cleanly.
- `mise run bench:renderer` drives a changing 100 by 32 release-mode grid under
  Xvfb and produces renderer statistics.
- Warm CPU preparation remains below the 16.7 ms frame budget on the current
  Linux host.

## Risks

- Direct glyph painting must preserve fallback-font and combining-glyph
  placement around the fixed grid baseline.
- Cached layouts must not capture color or decoration state accidentally.
- Full cell snapshots and comparisons still allocate and scan the grid. They
  remain candidates for a later protocol/engine optimization.
- GPUI primitive preparation or submission can become the next bottleneck after
  shaped-line overhead is removed. Profile that stage before adding another
  renderer backend.

## Claude review

Claude reviewed the first draft against the repository and GPUI 0.2.2. This
revision incorporates its material findings: compact `LineLayout` storage,
explicit attribution rather than calling all time font shaping, relative
geometry, row clipping and global paint order, a fixed baseline, pure test
seams, two-generation cache keys, Linux font selection, and an executable
measurement harness. Indexed row retention remains intentionally limited to
partial updates; full-screen workloads depend on layout reuse instead.

## Unresolved questions

None block this slice. A later `libghostty-vt` plan must decide how raw output,
engine snapshots, and client-side render state replace the current semantic
snapshot path.
