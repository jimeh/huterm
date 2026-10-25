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

## ref-counted-set-dead-id-reuse

Fixes [jimeh/huterm#170](https://github.com/jimeh/huterm/issues/170): rewriting
a row's OSC 8 linked text with a different hyperlink made Ghostty clone the
page and report a full `RenderState` redraw, so every row was rebuilt.

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
the same type, so a full style set also stops cloning its page. The patch adds
a set-level Zig test. Huterm's
`non_color_osc_and_utf8_output_keep_snapshots_partial` engine test and the
`osc8` and `osc8-id` fixtures of `mise run bench:engine` fail without it.

The fix has not been reported upstream yet, so the manifest has no upstream
link. Drop the patch once the pin includes an equivalent upstream fix.
Follow-ups to upstream's
[offset data structure fixes](https://github.com/ghostty-org/ghostty/pull/13304)
are expected to rework `addContext`, so expect this patch to conflict on a
later pin bump.
