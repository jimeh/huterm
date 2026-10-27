# Scrollback history scale

Status: in progress.

`ScrollbackLimit::MAX` is 50,000 rows because link lookup above the viewport
slows as history grows. Ghostty resolves each `Point::Screen` grid reference by
walking page nodes from the first page of history, and the link scanner
resolved one reference per cell. This plan removes both costs, measures larger
histories, and raises the maximum to what the measurements support.

## Contract

1. **One walk per row.** `huterm-ghostty` adds `RowRef`, resolved through
   `ghostty_terminal_grid_ref` at the rightmost column the caller will read.
   Ghostty's `pin` validates that column against the page's own width, since
   incomplete reflow can leave a page narrower than the terminal. `RowRef`
   hands out cell references for any column up to the validated one by
   changing only `x`, refuses columns beyond it, and reads the row's wrap flag
   from the same reference. `GhosttyLinks` keeps the current row and the
   lookup's first row, so each row above the viewport resolves at most once
   per lookup.
2. **Walk from the nearer end.** A Zig-only Ghostty patch adds
   `PageList.pinNearest`, which resolves a screen or history point from the
   last page when that end is nearer, using the cached `total_rows`. It returns
   the same pin as `pin`, including bounds and the narrow-page check, and
   asserts that in slow-safety builds. Only the C API's
   `ghostty_terminal_grid_ref` uses it: Ghostty's internal callers can run
   while `total_rows` is mid-update, between C API calls it is consistent.
3. **Measure.** `bench:links` saturates history at the maximum by default and
   at chosen sizes through `HUTERM_BENCH_HISTORY_ROWS`, bypassing the
   configuration maximum through a test-only engine hook. `bench:engine` adds a
   saturated-history resize reflow fixture at the same sizes.
4. **Raise the maximum** to the largest value at which every gated benchmark
   keeps clear headroom, sized to whichever cost binds first, including reflow
   and the derived 4 KiB-per-row byte allowance.

Out of scope: an upstream Ghostty API to step a grid reference between rows.
It would replace step 1's column adjustment.

## Verification

- A `huterm-ghostty` contract test shows that a column-adjusted reference
  reads the same cell and row as a freshly resolved one, and that columns past
  the validated one are refused.
- A `huterm-ghostty` contract test shows that screen points resolve to the
  same reference as viewport points at every scroll position, across several
  pages, and fail past the last row.
- Zig tests in the patch compare `pinNearest` with `pin` for every row of
  multi-page lists, including after history is pruned, for both tags.
- Existing link tests in `huterm-core` cover lookup behavior; a perturbation
  of each change must fail at its intended assertion.
- `mise run check`, `mise run test`, `mise run test:ghostty-releasesafe`,
  `mise run license`, `bench:links`, and `bench:engine`, with before and after
  numbers for each step.
