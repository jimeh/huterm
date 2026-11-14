# Selections that survive reflow

Status: approved on 2026-11-14. Jim chose to keep a settled selection on its
text through output (Ghostty's behavior) over clearing it when output rewrites
a selected row, or remapping it only across resizes.

Issue: [#373: Keep a terminal selection when only the cell size
changes][issue-373], including the reflow case added to it on 2026-11-07.

## Outcome and scope

A settled terminal selection keeps covering the same text when the terminal
resizes, whether the resize changes only the cell size, only the rows, or the
columns so that lines rewrap. It also keeps covering that text while output
scrolls it. It clears only when its text is gone or when the user moves on.

A settled selection is one with a range and no gesture in progress: a
released cell drag, a word or line selection whose expansion has finished,
Select All, and any selection a client sets programmatically (#38). A drag in
progress keeps today's behavior and clears when the content changes under it.

Out of scope: carrying the tracked selection over the runtime IPC wire
(see [remote viewers](#remote-viewers)), and rectangular selections, which
Huterm does not have.

## Findings

### Where selection lives today

- The selection is view-local in `huterm-gpui` (`desktop/selection.rs`): an
  anchor and head as `BufferPoint`s (rows above the live bottom, plus a
  column) and the snapshot `generation` they were made at. Word and line
  selections also hold the range the runtime expanded them to.
- Core extracts text (`extract_text`) and expands units (`select_units`) only
  at the generation the client names, and answers `None` otherwise.
- The snapshot `generation` advances on every content change: output,
  resize (including a cell-size-only change), clearing history, reset, and
  history eviction by a lower limit. `TerminalView::apply_snapshot` and the
  invalidation path in `refresh` clear the selection whenever the generation
  differs from the selection's.

So today any output clears a selection, not only a resize. This matters for
reflow: shells repaint their prompt after `SIGWINCH`, so a resize at a shell
prompt is followed by output. A fix that remapped the selection across the
resize but still cleared it on output would lose it in the most common case.

### What Ghostty offers

Ghostty's own selection survives reflow because its page list moves tracked
pins as it reflows, scrolls, and prunes. Huterm's binding already exposes this
at the pinned v1.4.0 through `ghostty_terminal_grid_ref_track` and the
`grid_ref_tracked.h` API, as `Terminal::track`, `Terminal::has_value`, and
`Terminal::tracked_point` (`huterm-ghostty`). Command records already use it
(`engine/records.rs`). Its contract tests show that a reference:

- follows its cell through scrolling and through reflow in both directions;
- loses its value when Ghostty discards the cell: RIS, the C reset, ED 3
  for history rows, and pruning a page past the history limit;
- keeps its row through in-place erasure (ED 2, delete line);
- stays on the screen it was created on, and keeps its value and position
  while the other screen is active;
- on the first half of a wide character that a narrower width pushes to the
  next row, stays on the spacer head left at the old row's end, while one
  on the second half follows the character.

Ghostty reflows only the primary screen, and only while wraparound is on; the
alternate screen resizes without reflow. Ghostty's own selection survives
output and clears on a screen switch, on reset, and, by default, on typing
(`selection-clear-on-typing`).

No new Ghostty API, patch, or version change is needed.

### Logical-line anchoring, considered and rejected

Anchoring each end to a logical line and an offset, and remapping it from the
snapshot's soft-wrap flags, does not work from the client. The snapshot holds
only the viewport, so the client cannot count logical lines in history, and
it cannot tell how far output scrolled once history is pruned. Core could
count them, but logical line numbers from the top of history change on every
prune, so core would rebuild what Ghostty's pins already do, less reliably.

## Design

Core keeps each viewer's settled selection on its text with two Ghostty
tracked references, and reports where it lies in every snapshot it builds for
that viewer. The client still owns the gesture and the selection's
presentation; core owns only the mapping through content changes.

### Protocol

Two dependency-neutral types in `huterm-protocol`:

- `TrackedSelection { id, generation, range }`: a settled selection a viewer
  asks its runtime to keep. `id` is the client's number for it, and
  `generation` is the snapshot generation `range` was made against.
- `SelectionPlacement { id, range: Option<BufferRange> }`: where that
  selection lies in one snapshot. `None` means it is gone.

### Core

- `TerminalViewer::track_selection(Option<TrackedSelection>)` sends a control
  on the same queue as snapshot, selection-text, and unit-expansion requests,
  so it runs in order with them. `None` forgets the viewer's selection.
- The engine keeps at most one tracked selection per viewer. When the
  generation matches, it tracks both ends on the active screen and records
  which screen that is. A stale generation records the selection as gone. A
  new selection replaces the old one, and references are freed on the owner
  thread.
- The owner thread ignores a tracking control from a revoked or dropped
  viewer, or one the arbiter has no entry for, and forgets the selection of
  every viewer the arbiter finalizes. Controls are not counted toward a
  dropped viewer's queue, so this check is what keeps references from
  outliving their viewer.
- `SnapshotReply` gains `selection: Option<SelectionPlacement>`, resolved in
  the same owner-thread step that builds the snapshot, so the range always
  matches the snapshot's generation. `None` means the viewer tracks nothing.
- Resolution reads only page metadata (`tracked_point`), never cells, so it
  restores no compressed page:
  - Both ends have values on the selection's screen while it is active: the
    range between them.
  - The start lost its value and the end did not: eviction or clearing
    history discarded the selection's oldest rows. The range shrinks to start
    at the oldest retained row, column 0.
  - The end lost its value, or another screen is active: gone. Ghostty
    discards cells in reading order, so a lost end means the whole selection
    is gone.
- Resizes:
  - After a column change that reflows the primary screen, an end left on a
    spacer head is tracked again at the wide character the wrap pushed to the
    next row: the start at its first column and the end at its second. A
    width change restores all history first, so this cell read restores
    nothing extra, and later resolutions need no cell reads.
  - Any grid change clears a selection made on the alternate screen. Ghostty
    resizes that screen without reflow, clipping cut columns and moving
    references onto the remaining rows, and full-screen programs repaint
    after a resize anyway.
  - A column decrease clears a primary-screen selection while wraparound is
    off, because Ghostty then clips instead of reflowing.
- Text for a tracked selection comes from its references, not from the
  client's range: `request_selection` gains a form that names the tracked
  selection's `id`. Copy then works while output keeps advancing the
  generation, where a range request would be stale before it ran.
- Selection tracking never fails the runtime. An error from creating or
  resolving a reference drops that viewer's selection (both references, even
  after a partial creation) and reads as gone; it never reaches the
  snapshot-failure path that closes the runtime.

### Desktop client

- When a selection settles, the view sends `track_selection` with a new
  `id`. Clearing sends `None`. Settling covers a released cell drag
  (including one released after edge scrolling), a word or line selection
  whose expansion completes after release, Select All, and programmatic
  selections.
- A settled selection no longer clears on a generation change, in
  `apply_snapshot` or in `refresh`'s invalidation path. Each snapshot reply
  decides it:
  - A placement with the current `id` moves the range, or clears the
    selection when it is gone.
  - A reply without the current `id` was built before the runtime saw the
    selection. If its generation is the selection's, the range is still
    exact. Otherwise the runtime will find the selection stale when it
    arrives, so the view clears it now, as today. This needs no
    acknowledgement and never leaves a selection waiting for a reply that
    may not come.
- The cached selection text belongs to the generation it was read at. Copy
  uses it only while the displayed snapshot has that generation; otherwise it
  asks the runtime by `id`. Text replies are matched to the selection `id`
  and viewer epoch before they are cached. CopyOrPaste still copies before it
  clears.
- Typing, pasting, and other input the view sends to the program clears the
  selection, as Ghostty's default does and as the program's echo does today.
  Commands such as Copy do not.
- After a viewer replacement, the view sends its settled selection to the
  new viewer, and the reply rule above applies.
- Clearing scrollback or resetting from the view still clears its own
  selection at once. Other viewers follow the resolution rules.

### Cases

| Case | Behavior |
| --- | --- |
| Cell-size-only change (#373) | Kept: the references do not move. |
| Height-only resize | Kept: rows move between history and the screen. |
| Width change that rewraps | Kept: the ends follow their cells, so the range covers the same text in reading order. |
| Wide character pushed to a new row by the wrap | Kept: an end left on the spacer head moves to the character. |
| Selection partly in scrollback | Kept: history rows reflow and scroll like screen rows. |
| Workspace panel toggle (#27) | Kept, as for any resize. |
| History eviction or a program's ED 3 removes the selection's start | Shrinks to the retained rows. |
| Eviction or ED 3 removes the whole selection | Clears. |
| Clear Scrollback or Reset in this view | Clears at once, as today. |
| RIS or reset from another source | Clears: every cell is discarded. |
| Output scrolls the selection | Kept on its text. |
| Output rewrites the selected cells in place, or deletes their rows | Kept on the cells or replacing rows, showing the new text, as Ghostty does. |
| Scrolling inside a scroll region, or any scrolling with `scrollback = 0` | Kept on the rows that replace its own: Ghostty deletes rows in place there, so the selection does not follow its text. |
| Entering the alternate screen, or leaving it | Clears when a snapshot shows the other screen. A round trip between two snapshots leaves the primary cells intact and keeps it. |
| Any resize while the selection is on the alternate screen | Clears. |
| Column decrease with wraparound off | Clears. |
| Word and line selections | Kept as their expanded range. |
| Typing or pasting into the terminal | Clears. |
| A drag still in progress | Clears on a content change, as today. |
| Several viewers of one terminal | Each viewer's selection is tracked and placed separately. |

### Remote viewers

The IPC wire does not carry `track_selection` or the placement in this
change, so a remote client behaves as today: its selection clears on a
generation change. `huterm-server` takes only the snapshot and link from
`SnapshotReply`, so the new field changes nothing there. Carrying the
selection needs a viewer request kind and a field appended to the terminal
update, which belongs with the first client that renders remote selections
(#45). Core's API takes and returns protocol types so that change is
additive.

## Costs and risks

- Ghostty does work for every live reference on each row it reflows. Each
  viewer adds at most two.
- Resolving a reference walks the page list to its page, once per end per
  snapshot for viewers with a selection, and reads no cells.
- A selection now outlives output. The cache rule above keeps Copy from
  returning text older than the displayed snapshot.

## Verification

- `huterm-ghostty` contract tests (done): a reference made in history follows
  reflow; a wide character pushed across a wrap leaves the first-half
  reference on a spacer head; a reference keeps its value while the other
  screen is active.
- `huterm-core` engine and runtime tests, asserting both the placement and
  the text extracted by `id`:
  - a width change that rewraps (narrower and back), a height-only change, a
    cell-size-only change, and a selection partly in history;
  - a wide character at the new wrap point, at either end of the range;
  - output that scrolls the selection; output that rewrites it in place;
  - eviction that shrinks and that clears; a program's ED 3 that shrinks;
    RIS;
  - entering the alternate screen; an alternate-screen selection and a
    resize; a column decrease with wraparound off;
  - a stale generation; replacement and forgetting; two viewers with
    separate selections through one resize;
  - a tracking control from a dropped or revoked viewer creates nothing,
    and finalization frees the selection;
  - resolving placements over compressed history leaves Ghostty's
    compressed page count unchanged, across repeated snapshots.
- `huterm-gpui` view tests:
  - a settled selection survives a snapshot with a new generation, moves to
    the placement, and Copy writes the remapped text; the accessibility
    range matches it;
  - a gone placement clears it; a reply without the current `id` keeps it at
    the same generation and clears it at another;
  - typing clears it; a drag in progress still clears;
  - Select All, a released word selection, and a cell drag released after
    edge scrolling each track;
  - CopyOrPaste copies the remapped text and then clears;
  - a viewer replacement resends the selection;
  - the cell-size-only case #373 asks for.
- Native check: select text at a zsh prompt, resize the window narrower and
  wider and toggle a workspace panel, and confirm the highlight and Copy
  cover the same text. This runs through the existing macOS input smoke if
  it can drive a resize, or by hand otherwise.

[issue-373]: https://github.com/jimeh/huterm/issues/373
