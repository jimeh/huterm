# Ghostty native patches

Each `.patch` file here is a reviewed fix to the pinned Ghostty source, applied
in the order `scripts/ghostty-source.json` lists them. The manifest records
each file's SHA-256; `mise run ghostty:prepare` rejects a patch whose bytes
differ. The build applies the patches to its private copy of the source and
never changes `.native/ghostty/source`.

This directory does not ship with Huterm. See
[native patches](../../docs/agents/terminal-engines.md#native-patches) for
verification, the Git requirement, and how to add, rebase, or drop a patch.

Record each patch below: the defect it fixes, evidence, and the condition for
removing it.

## ref-counted-set-probe-invariant

Upstream's own fix, from
[`3267b63ef`](https://github.com/ghostty-org/ghostty/commit/3267b63ef9c3154c7c239ba71b378d8a663f0d98)
by Mitchell Hashimoto. It was part of upstream's
[offset data structure fixes](https://github.com/ghostty-org/ghostty/pull/13304),
which were closed unmerged in September 2026, so `cec8b92` lacks it.

When `insert` walks a probe chain and meets a dead item, it reclaims that
bucket for the item it holds without comparing probe sequence lengths. If the
held item's is shorter, lookups stop at it and never reach live entries further
along the chain. Release builds then add duplicate styles and hyperlinks under
new IDs. Debug and ReleaseSafe builds trip `insert`'s duplicate assertion when a
later delete shifts a hidden value back into reach, which `addWithId`'s
dead-ID path and the next patch's reuse path both do. The fix reclaims a dead
bucket only when the held item's probe sequence is at least as long.

The patch carries only upstream's code change and its two tests, adapted to
`cec8b92`: the branch's `testSetInit` and `verifyIntegrity` helpers don't exist
there, so the tests build their sets inline and skip the integrity call. Both
tests fail without the fix. Drop this patch once the pin includes the fix.

## ref-counted-set-dead-id-reuse

Fixes [jimeh/huterm#170](https://github.com/jimeh/huterm/issues/170): rewriting
a row's OSC 8 linked text with a different hyperlink made Ghostty clone the
page and report a full `RenderState` redraw, so every row was rebuilt. It
applies after `ref-counted-set-probe-invariant`, which keeps its delete and
insert from hiding or duplicating live values.

A standard page's hyperlink set has 4 buckets and 3 item slots
([page.zig](https://github.com/ghostty-org/ghostty/blob/cec8b92f1ecd176a20a56aa405021e7ae991b996/src/terminal/page.zig#L125),
[ref_counted_set.zig](https://github.com/ghostty-org/ghostty/blob/cec8b92f1ecd176a20a56aa405021e7ae991b996/src/terminal/ref_counted_set.zig#L185)).
ID 0 is reserved, so only IDs 1 and 2 are usable. A replaced link dies below a
live ID, where the tail trim in `addContext` cannot reap it
([L269](https://github.com/ghostty-org/ghostty/blob/cec8b92f1ecd176a20a56aa405021e7ae991b996/src/terminal/ref_counted_set.zig#L269)).
The next add then returns `NeedsRehash`
([L298-L308](https://github.com/ghostty-org/ghostty/blob/cec8b92f1ecd176a20a56aa405021e7ae991b996/src/terminal/ref_counted_set.zig#L298-L308)).
`Screen.startHyperlink` answers with a same-capacity page clone
([Screen.zig](https://github.com/ghostty-org/ghostty/blob/cec8b92f1ecd176a20a56aa405021e7ae991b996/src/terminal/Screen.zig#L2838),
[PageList.zig](https://github.com/ghostty-org/ghostty/blob/cec8b92f1ecd176a20a56aa405021e7ae991b996/src/terminal/PageList.zig#L4219)),
and `RenderState.update` rebuilds every row when the viewport pin's node
changes
([render.zig](https://github.com/ghostty-org/ghostty/blob/cec8b92f1ecd176a20a56aa405021e7ae991b996/src/terminal/render.zig#L1020)).
Each OSC 8 start without `id=` takes a new implicit ID, so rewriting the same
URI triggers this too.

The patch reuses a dead ID in place when the set is full and fewer than 90% of
its IDs are live, with the same `deleteItem` and `insert` steps that
`addWithIdContext` uses for a dead requested ID
([L363-L364](https://github.com/ghostty-org/ghostty/blob/cec8b92f1ecd176a20a56aa405021e7ae991b996/src/terminal/ref_counted_set.zig#L363-L364)).
Dead items have no cell references, so nothing is renumbered. Style sets use
the same type, so a full style set also stops cloning its page. If the living
count ever drifts so that no dead ID exists, `add` returns `OutOfMemory`
rather than reaching undefined behavior.

`NeedsRehash` stays in the error set, documented as not returned, so the patch
leaves its handlers in `Screen.zig`, `page.zig`, and `PageList.zig` untouched.
An upstream version of this fix could remove the variant and those handlers.

The patch adds tests at each level where the bug shows:

- `ref_counted_set.zig`: dead-ID reuse in a two-link set; with 12 usable
  IDs, `OutOfMemory` while 11 or more are live (90% of the capacity of 13),
  and reuse at 10; and seeded add and release
  churn on a 64-item set that checks ref counts and that every value reaches
  the `deleted` callback, which frees hyperlink strings, exactly once.
- `Screen.zig`: rewriting one row's hyperlink 16 times keeps the page node,
  the hyperlink set's capacity, and one live link.
- `render.zig`: rewrites with alternating implicit IDs, the same implicit-ID
  URI, and alternating explicit IDs keep every update `.partial` on the same
  page node.

Each of these tests fails without the reuse, even with the first patch
applied. Huterm's `non_color_osc_and_utf8_output_keep_snapshots_partial`
engine test and the `osc8` and `osc8-id` fixtures of `mise run bench:engine`
fail without it too.

The fix has not been reported upstream yet, so the manifest has no upstream
link. Drop the patch once the pin includes an equivalent upstream fix.

## page-list-pin-nearest

Makes `ghostty_terminal_grid_ref` cost independent of history length for
rows near the active area. `PageList.pin` resolves a screen or history point
as the first page's top-left moved down `y` rows
([PageList.zig](https://github.com/ghostty-org/ghostty/blob/cec8b92f1ecd176a20a56aa405021e7ae991b996/src/terminal/PageList.zig#L5696-L5708)),
which walks one page node at a time from the oldest history. Huterm's link
lookup resolves rows above the viewport through screen points, so each such
row cost a walk over every page of history above it.

The patch adds `PageList.pinNearest`, which returns the same pin as `pin` but
starts from the last page and moves up when that end is nearer. It uses the
cached `total_rows`, keeps `pin`'s column bound and narrow-page check, and in
slow-safety builds asserts that its result equals `pin`'s. Only the C API's
`grid_ref` entry point uses it. `total_rows` is stale inside some `PageList`
operations, such as reflow, `trimTrailingBlankRows`, and `erasePage`, and
`pin` has hundreds of internal callers, so they keep using `pin`. Between C
API calls no operation is in progress, and `PageList`'s integrity check
asserts that the cached count matches the pages after each one, except after
a failed reflow: `resizeCols` set `total_rows` only once every row was
reflowed, so an allocation failure returned with the old count while the
list held only the rows reflowed so far. `pinNearest` then resolved some
screen rows to the wrong row where `pin` refused them. The patch makes
`resizeCols` recount the list when it fails. Huterm closes a terminal whose
resize fails, but the C API stays usable after one.

`PageList.zig` tests compare `pinNearest` with `pin` for every row, one row
past the end, and four columns, for both tags: on one page, on five pages,
after erasing history from the front, after a reflow, and with pages
narrower than the list, as an incomplete reflow can leave them, and after a
widening reflow that fails at each of its allocations in turn. Removing the
narrow-page check, moving the start by one row, or removing the recount makes
them fail. Huterm's
`grid_ref_tests` check screen points against viewport points at every
scroll position and after pruning.

With a million rows of history, on an Apple silicon development machine,
`mise run bench:links` measured `scan-limit-above-viewport`'s p95 at 60 ms
without the patch and 0.21 ms with it. Its rows are near the bottom of
history. `scan-limit-mid-history` reads rows in the middle, far from both
ends: 30 ms without the patch and 27 ms with it. Huterm's row references,
which resolve each row once, bring that to 0.48 ms.

The fix has not been reported upstream, so the manifest has no upstream
link. Drop the patch once the pin resolves screen points from the nearer end,
or once Huterm steps references between rows through an upstream API such as
a `ghostty_grid_ref_up`, which would remove its screen-point walks.
