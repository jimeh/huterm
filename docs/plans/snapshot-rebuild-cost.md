# Snapshot rebuild cost

Status: implemented for [#162](https://github.com/jimeh/huterm/issues/162); see
[Implementation outcome](#implementation-outcome) and the
[measurements](../performance/gpui-terminal-renderer.md#snapshot-rebuild-cost-2026-09-25).
This is the first of three performance PRs. Input scheduling ([#163](https://github.com/jimeh/huterm/issues/163))
and GPUI paint and chrome work ([#164](https://github.com/jimeh/huterm/issues/164))
follow separately.

## Outcome and scope

Make runtime snapshot construction proportional to what changed. Ordinary
output should not trigger full-grid rebuilds. A rebuilt row should not
allocate once per cell. Rows whose content has not changed should keep their
`Arc<TerminalRow>` so that the core and the renderer both skip them.

The baseline on 2026-09-25 (`mise run bench:engine`, release, 120 by 40, Apple
M3 Max) measured parse p50 at 2.7 to 19 µs. Full-rebuild snapshot p50 was
188 to 206 µs. A snapshot with one dirty row took 5.6 µs.

This PR covers:

- Narrowing the color-override hint so only color-changing sequences trigger a
  palette probe.
- Caching `TerminalModes` per engine generation.
- Replacing `protocol::Cell`'s `String` with a compact text type, and reading
  cell text without per-cell allocation.
- Reusing row `Arc`s when a rebuilt row matches a retained row, including rows
  shifted by live output.
- Suppressing repeated identical title events, if that proves safe.

Out of scope:

- PTY reader batching and the writer's per-dequeue runtime wake.
- Link-lookup scrollback walking.
- The macOS 1 MiB process-argument buffer.
- The `select(2)` descriptor limit.

Record these in #162 as follow-ups. Batched native cell reads through
`ghostty_render_state_row_cells_get_multi` are conditional, as step 7 explains.

## Settled decisions and constraints

- `protocol::Cell` may change to a compact text representation. Consumers and
  tests move to the new type in the same PR.
- `huterm-protocol` keeps its empty dependency set, which `mise run architecture`
  enforces. The workspace denies `unsafe_code`, so the compact type must be
  built with safe standard-library code only.
- Keep the AGENTS invariants on snapshots. Complete snapshots share immutable
  `Arc<TerminalRow>` values, and the core never clears engine damage before its
  owned cache is coherent. Effective colors are compared before damage is
  consumed. Content generation excludes scrolling.
- The native Ghostty source is pinned and hash-verified. Do not patch it to
  expose palette dirty flags or its override mask. The adapter keeps probing
  overrides, and only the trigger changes.
- Validate on macOS natively and on Linux through Docker (`mise run linux:test`).
  Do not use a macOS VM.

## Implementation sequence

Land each step as its own commit, keeping its tests and benchmark evidence with
it.

### 1. Benchmark fixtures and baseline

Extend `engine_benchmark` in `crates/huterm-core/src/engine.rs` with these
fixtures:

- An OSC 133 plus OSC 7 prompt cycle.
- A title updated on every chunk (OSC 2).
- OSC 8 hyperlinked `ls`-style output.
- `╝` and CJK text interleaved with SGR sequences.
- A color OSC (OSC 4 and OSC 11) as a control that must still rebuild.
- A live scrolling flood.
- A scrolling flood after scrollback has reached its 16 MiB budget.

The benchmark currently reports rebuilt rows as the complement of `Arc`s reused
from the previous snapshot. After step 5, that no longer holds, because an
unchanged row can be extracted and compared before its `Arc` is reused. Add a
`#[cfg(test)]` accessor on the adapter that reports three independent counts for
the last snapshot:

- Rows extracted from Ghostty.
- Rows that received a newly allocated `Arc`.
- Rows that reused a retained `Arc`, at any index.

Report all three for each fixture. Record the baseline before changing
behavior.

### 2. Narrow the color-override hint

`EscapeHint::observe` in `crates/huterm-core/src/engine/ghostty.rs` currently
flags every OSC terminator and treats raw `0x9c` and `0x9d` bytes as C1 controls
everywhere. Change it so that `colors_dirty` is set only for sequences the pinned
Ghostty build treats as color operations.

- **OSC numbers.** Flag an OSC only when its leading number is one that
  Ghostty's `osc.zig` dispatches to its color parsers: 4, 5, 10 through 19,
  21 (kitty color protocol), 104, and 110 through 119. Keep RIS (`ESC c`)
  flagged. Accumulate the number across chunk boundaries.
- **C1 bytes.** Model Ghostty's C1 handling from `stream.zig` and
  `parse_table.zig`. In the ground state, Ghostty decodes bytes as UTF-8, so a
  raw `0x9d` there is never an OSC introducer. Its "anywhere" C1 transitions
  apply only while the parser is outside ground. Inside `osc_string`, bytes
  0x20 through 0xFF, including `0x9c` and `0x9d`, are payload.
- **Conservative misses.** Where exact modeling would be costly, the hint may
  flag a sequence that is not a color operation. It must never miss one.
- **Retained colors.** Reset `self.colors` only when the probed
  `palette_overrides` or `default_overrides` differ from their previous values.
  The probe's own palette writes still mark Ghostty's palette dirty, so a real
  color operation still produces a full rebuild.

Tests:

- Unit cases:
  - Split OSC numbers, such as `ESC ] 1` followed by `04;...`.
  - `╝` followed by SGR must not flag.
  - OSC 2, 7, 8 and 133 must not flag.
  - OSC 21 and OSC 104 with no parameters must flag.
  - C1 OSC entered from within a CSI sequence must flag.
- A differential test that uses the full probe as an oracle. Run it after every
  chunk of generated byte streams that mix text, UTF-8, SGR, CSI, and color and
  non-color OSC sequences in 7-bit and C1 forms, split at random boundaries.
  Whenever the oracle observes a change in effective colors or override state,
  assert that the hint flagged that chunk.
- Keep the existing override, split-sequence, and reset tests.
- Before trusting the differential test, perturb the hint once by dropping
  OSC 11 from the set, and confirm that the test fails at its assertion.

### 3. Cache terminal modes per generation

Store the last `TerminalModes` in the adapter together with the generation that
produced it. `modes()` returns the cached value while the generation is
unchanged. The cache is cleared by `process`, `resize`, and
`update_presentation`. Existing mode and mouse-encoding tests cover correctness.
Add one test showing that a mode change inside `process` is visible through the
next `modes()` call.

### 4. Compact cell text in the protocol

Add a `CellText` type to `huterm-protocol` and change `Cell::text` to it.

- **Representation.** Store short UTF-8 text inline, falling back to the heap
  only for grapheme clusters that do not fit. A single scalar value must never
  allocate. `CellText` must be no larger than `String`.
- **Canonical form.** Any text that fits inline is always stored inline, so
  equality and hashing do not depend on how a value was constructed.
- **API.** Provide:
  - `as_str()`, and `Deref<Target = str>` so that existing `&str` consumers
    compile unchanged.
  - `From<char>`, `From<&str>` and `From<String>`.
  - `PartialEq` with `str` and `&str`.
  - `Clone`, `Debug`, `Eq` and `Hash`.
  - A blank constant for `" "`.
  - An `as_char()` fast path for single-scalar cells, which the renderer's
    `single_scalar` and built-in lookup can use.
- **Adapter.** In `snapshot_cell`, branch on the raw cell's content tag, which
  the adapter already reads:
  - `Codepoint` builds `CellText` from `raw.codepoint()`, with 0 meaning blank.
  - `CodepointGrapheme` reads UTF-8 into one scratch `String` owned by the
    adapter, then builds `CellText` from it.
  - The background-only tags produce blank text.

  This removes the per-cell allocation and the zero-capacity retry in
  `graphemes_utf8`.
- **Consumers.** Update the renderer, the benchmarks, smoke fixtures, and the
  tests that construct `Cell` values or collect text through `cell.text.as_str()`.
  Most call sites compile unchanged through `Deref`. Sites that build a `String`,
  such as `renderer/bench.rs`, switch to `.into()`.

Tests:

- `CellText` unit tests at the inline capacity boundary, covering scalar,
  combining-mark, and ZWJ-emoji clusters. Include equality and hashing across
  constructors.
- The existing `unicode` engine fixture and wide-cell tests continue to assert
  `e\u{301}`, wide spacers, and blank handling.
- `mise run architecture` confirms that the protocol crate still has no
  dependencies.

### 5. Reuse unchanged row Arcs, including after a viewport shift

The current `snapshot()` allocates a new row `Arc` for every rebuilt row.
Ghostty reports a full redraw whenever the viewport pin moves, which happens on
every new line while following live output.

- **Scratch row.** Read each rebuilt row into a scratch `Vec<Cell>` owned by the
  adapter.
- **Match by content.** Reuse any retained `Arc` whose row content equals the
  scratch row. Retained rows are immutable, so matching content is enough for
  correctness, whatever the row's previous position. One `Arc` may fill several
  positions, for example repeated blank rows.
- **Candidate order.** For each rebuilt row, try these retained rows in order,
  stopping at the first match:
  - The row at the same index.
  - The row at the history-derived shift. That shift is the one `rows_to_rebuild`
    in `crates/huterm-gpui/src/renderer.rs` computes from the bottom offsets and
    history sizes.
  - The row at the shift that last matched in this snapshot.
  - Otherwise, a scan of all retained rows, bounded by the viewport height.

  Do not infer one shift for the whole snapshot from a single anchor row. Once
  scrollback has reached its byte budget, pruning can hold `history_size` steady
  while content shifts, so the derived shift is often zero. A blank, repeated,
  or changed first row would then fix the wrong shift for every row below it.
  Row comparisons exit at the first differing cell, so the bounded scan is cheap
  compared with extraction.
- **Changed rows.** Only a row that matches no retained row moves out of the
  scratch buffer into a new `Arc`.
- **Renderer.** Align previous prepared rows by `Arc` identity before falling
  back to its history-derived shift and equality check. Budget-capped shifts can
  then reuse prepared rows too. Identity alignment must accept one `Arc` at
  several positions.
- **Dirty flags.** Clear per-row dirty flags only for rows that were visited as
  dirty, while preserving the coherence invariant above.

Tests:

- Engine tests showing that live scrolling reuses shifted `Arc`s, both before
  and at the scrollback budget.
- At the scrollback budget, a viewport whose first rows are blank or repeated,
  such as `[blank, blank, A, B]` becoming `[blank, A, B, C]`, reuses the `Arc`s
  for `A` and `B`. So does a scroll whose first row changed.
- A partial update that rewrites a row with identical content keeps its `Arc`.
- Renderer `rows_to_rebuild` tests for identity alignment when `history_size`
  does not change.
- Existing snapshot, selection, and link tests still pass.
- Run `bench:renderer-scenarios` against its saved baseline. The `scroll`
  scenario must not regress.

### 6. Suppress identical title events (conditional)

Remember the last published title in the runtime and skip `TitleChanged` when
the text is unchanged. The existing `title_needs_group` attribution logic stays
as it is. Before landing this step, confirm that no client relies on a repeated
`TitleChanged` event, for example to rebuild a title cache after attachment,
retargeting, or configuration reload. If one does, drop this step and record it
in #162.

### 7. Measure, then decide on batched native reads

Rerun the extended `bench:engine` and record the results. Profile a
full-rebuild fixture with Instruments on macOS, or with `perf` in the Linux
container, to find how the remaining per-cell time splits between native calls
and Rust work.

Batching `raw_cell`, `style`, `content_tag`, `wide`, and the background reads
through `row_cells_get_multi` needs a safe wrapper patch to the vendored
`libghostty-vt` crate, using the `vendor:start` / `vendor:finish` workflow.
Do this only if native calls still dominate and the expected gain justifies
maintaining the patch. Otherwise record the measurement in #162 and defer
batching.

## Verification

- Run focused `cargo test -p huterm-core` and `cargo test -p huterm-protocol`
  while iterating.
- Before handoff, run `mise run verify` on macOS. This is the repository's
  CI-equivalent gate, and it includes `vendor:check` for step 7's patch.
- On Linux, run `mise run linux:exec -- mise run check` and
  `mise run linux:exec -- mise run test` in Docker. `linux:test` runs only
  `test:rust`, so it does not cover Clippy or the full test task.
- Compare `mise run bench:engine` against the step 1 baseline. It is headless,
  so it runs natively on macOS and in the Linux container.
- Run `bench:renderer-scenarios` on macOS with `-- --compare` against a baseline
  recorded before step 4. This confirms the `CellText` change and the renderer
  alignment do not regress prepare time.
- Run `mise run ci:benchmarks` to confirm the output-latency, scroll, and
  link-lookup budgets still hold.
- Manual checks on the macOS host and in the Linux dev VM:
  - A shell with prompt integration.
  - A double-line box-drawing TUI.
  - CJK output.
  - `ls --hyperlink`.
  - A long `seq` flood past the scrollback budget.
  - A theme reload after color OSC overrides.

  Colors, selection, links, and scrolling must behave as before.

## Success criteria

- Prompt, title, OSC 8, box-drawing, and CJK fixtures without color changes
  extract only the rows their text touched, as the extracted-row count shows.
  `Arc` reuse alone does not prove this.
- Color OSC and RIS fixtures still update default colors and override state,
  and theme reloads preserve explicit overrides.
- Full-rebuild snapshot p50 improves measurably against the 188 to 206 µs
  baseline.
- The scrolling fixtures reuse shifted `Arc`s, both before and at the
  scrollback budget.
- Blank and single-scalar cells no longer allocate.

## Documentation

- Update the AGENTS.md paragraph on Ghostty color-only OSC updates so it says
  that color-operation hints, not all OSC terminators, trigger the override
  probe. Document how the hint relates to Ghostty's C1 handling.
- Add the before and after benchmark results to
  `docs/performance/gpui-terminal-renderer.md`.
- Mark this plan as implemented, with a link to the PR.

## Implementation outcome

The steps landed in order. These points differ from the plan above:

- Step 2 also includes OSC 21, the kitty color protocol, which Ghostty
  dispatches to its color parser, and treats `0x9c` inside an OSC as payload,
  matching `parse_table.zig`.
- Step 4 keeps `CellText::as_char`, which decodes stored bytes without
  validation. The first build validated inline bytes on every `as_str` call and
  raised renderer prepare by 31-41%. Single ASCII bytes now borrow from a static
  table, and the renderer keys glyph layouts and built-in glyphs by scalar.
  `CellText` equality compares canonical stored bytes.
- Step 5 allows two viewport heights of scanned row comparisons per snapshot,
  so a changed first row cannot exhaust the scan before the shifted rows below
  it match. A theme-update test now asserts full re-extraction instead of new
  row `Arc`s, because identical rows may keep theirs.
- Step 6 suppresses unchanged titles in `TerminalView`, not in the runtime.
  `Mux::attach` clients learn a title only from later reports, so runtime
  suppression would hide it from late attachers.
- Step 7 deferred batched native reads. Ghostty getters and binding wrappers
  are about 60% of full-rebuild snapshot time, but the remaining gain does not
  yet justify a vendored binding patch. Caching resolved styles across cells
  that share a `style_id` needs no patch and is a smaller follow-up.

Two further measured changes followed. The PTY reader batches output the PTY
already holds and waits for readiness without a read that must block. The
renderer hashes non-ASCII glyph scalars with a multiplicative hasher, which
also removed the `churn` scenario's prepare regression.

Open follow-ups:

- The OSC 8 fixture still extracts almost every row: Ghostty reports a full
  redraw when linked text is rewritten. A page copy on each rewrite is
  suspected, with its trigger unconfirmed. A fix belongs in Ghostty.
- Three foreground-job tests in `huterm-core` fail intermittently under full
  parallel load on the base commit as well as this branch.
