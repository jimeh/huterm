# Command output navigation, selection, and status (#38)

Status: approved.

Jim settled the styles on 2026-11-14: every header variant (H1 to H3) and
every failure display (T1 to T3) from the mockups is a setting, with H3 and
T1 as the defaults. He also chose that navigation scrolls the shared
viewport, as Page Up does, so another viewer of the same terminal moves with
it. The issue's "without moving another viewer" criterion therefore covers
other panes and synced panes, not a second viewer of the same terminal.
Per-viewer viewports are a separate follow-up.

## Outcome

A user can jump between shell commands, select a command's output or its
whole block, and see which commands failed, all from the OSC 133 marks and
command records that #352 added. While a long command's output fills the
pane and its prompt has scrolled out of sight, a header pinned to the
pane's top shows that prompt, as iTerm2 does. Clicking the header scrolls
back to the prompt.

## Constraints

- `huterm-protocol` stays dependency-free. Records still carry no screen
  position: positions travel in snapshots, which are already rebuilt when
  the viewport or content changes.
- One runtime thread owns the engine. All anchor and grid reads happen there,
  during snapshot construction.
- The snapshot is part of the wire contract. Minor version 0 is unreleased,
  so its records may still change; the encoders destructure exhaustively, so
  new fields fail to compile until they have a wire form and digest words.
- `bench:engine` and `bench:wire` must stay inside their gates. Snapshot
  construction must not walk history in proportion to its length.
- Rows above the viewport cost one page walk each to read (see the engine
  guide). Every scan of them is bounded.
- The other running threads own selection anchoring across reflow and core
  viewer sizing. This work only creates selections, through the existing
  `Selection::cells`, and does not change how they are kept.

## Approach

### Core: command blocks in snapshots

`TerminalSnapshot` gains two fields:

```rust
/// Command blocks that reach into this viewport, in row order.
pub commands: Arc<[CommandBlock]>,
/// The prompt of the block that owns the viewport's top row, when that
/// prompt starts above the viewport.
pub sticky_prompt: Option<StickyPrompt>,

pub struct CommandBlock {
    pub sequence: u64,       // the record; status comes from TerminalStatus
    pub prompt: usize,       // first prompt row, rows from the live bottom
    pub output: Option<usize>, // first output row, if the block has output
    pub end: usize,          // last row, trailing blank or prompt rows trimmed
}

pub struct StickyPrompt {
    pub sequence: u64,
    pub rows: Vec<Arc<TerminalRow>>, // the prompt rows, at most 3
    pub input: usize,                // index of the row that holds the command
}
```

`engine/records.rs` computes them when a snapshot is built on the primary
screen. The alternate screen gets none.

- **Positions** come from each record's tracked anchor. Ghostty's
  `pointFromPin` walks page nodes. In viewport space it walks forward from
  the viewport's top page and returns no value for a pin above the viewport
  (`PageList.zig` `pointFromPin`). In screen space it walks from the top of
  history. Walking every record on every snapshot would cost up to 256
  history walks, so core assumes that records follow row order (sequence
  order is row order unless cursor addressing intervened):
  - It binary-searches the records in viewport space for the oldest one at
    or below the viewport's top. That takes about 8 probes.
  - It reads the following records forward until one lies below the
    viewport.
  - The record just before that boundary owns the top row. Its prompt
    position costs one walk in screen space, and only while the header is
    shown.
  A `bench:engine` fixture with 256 records, live and scrolled near the top
  of 100,000 rows, measures the cost before and after.
- **Validation and aliasing:** a record is used only when its anchor row is
  still marked as a primary prompt. Positions must also strictly increase
  with sequence. When a newer record's row is at or above an older record's,
  as when in-place erasure (or `scrollback = 0`, which deletes rows in
  place) moves an older anchor onto a newer prompt (see the #352 plan), the
  older record is dropped. Its status therefore never paints over another
  command's block. A record that fails these checks gets no block, tint, or
  header.
- **Block extent:** a block runs from its prompt row to the row before the
  next block's prompt. The next block is the next validated record in row
  order, the pending prompt anchor, or, for the newest command, the cursor
  row. `output` is the first row after the prompt group (the primary row and
  the continuation rows after it) and is `None` if that row is past the end.
  `end` trims trailing rows that hold no text or carry a prompt mark,
  bounded to 64 rows. This trimming removes empty prompts from Enter and
  Ctrl-L left between two commands. OSC 133 A is a fresh-line prompt start
  in Ghostty (`fresh_line_new_prompt`), so output without a final newline
  stays on its own row and is never trimmed with the prompt. A shell that
  starts prompts with `P` alone can share a row; that is accepted.
- **Which blocks:** those that overlap the viewport, plus the block that owns
  the viewport's top row when its prompt is above the viewport.
- **Sticky prompt:** set when the top viewport row is neither a prompt row
  nor the owning block's first output row, and the owning block's prompt is
  above the viewport. Its rows are extracted through grid references with
  the same cell mapping as viewport rows (colors, styles, graphemes, and the
  text caps). They are cached by sequence, generation, and the effective
  colors, because presentation changes recolor rows without advancing the
  generation. An unchanged header keeps its `Arc` rows. `input` is the
  first prompt row with an input cell (Ghostty's cell semantic content), or
  else the last prompt row.
- **Accepted gap:** a prompt with no record, such as an empty Enter or a
  Ctrl-L redraw, does not start a block. Rows between it and the next
  record belong to the earlier block, apart from the trimmed tail.

- **Bindings:** `huterm-ghostty` gains the grid reference style read
  (`ghostty_grid_ref_style`) and the cell's semantic content
  (`GHOSTTY_CELL_DATA_SEMANTIC_CONTENT`). Both get safe wrappers,
  regenerated bindings (`mise run ghostty:bindings`), and contract tests.
  Tracked anchors are read through `ghostty_tracked_grid_ref_snapshot`,
  which needs no walk.

### Core: prompt navigation

`ScrollCommand` gains `Prompt(i32)`, which maps to Ghostty's
`GHOSTTY_SCROLL_VIEWPORT_DELTA_PROMPT`. A negative value moves toward
history and puts that prompt at the viewport's top. A prompt in the active
area returns the viewport to live, and with no prompt the viewport does not
move. Because the command uses Ghostty's row marks, it also stops at prompts
that have no record. It travels as a scroll, with the existing scroll
numbering, capability check, and return-to-live rules, and needs a wire tag.
`Engine::requested_viewport`, which predicts a scroll's offset, cannot
predict a prompt jump. For `Prompt` it applies the scroll and reads the
resulting viewport instead of predicting it.

### Wire

- A `ScrollCommand` tag for `Prompt`.
- Update encoding for `commands` and `sticky_prompt`. Sticky rows travel as
  rows, each with the same caps and run forms as viewport rows. Both fields
  get digest words and caps headroom.
- Minor 0 vectors and `SAMPLE_VIEW_DIGEST` are regenerated, because minor 0
  is unreleased. `huterm-server`'s `ModelTerminal` handles `Prompt`.

### Desktop

- **Commands:** `previous_command` and `next_command` scroll by one prompt.
  `select_command_output` and `select_command` select the target block's
  output or its whole block, then extract the text the way Select All does.
  The target is the block that owns the viewport's top row while the view is
  scrolled, and the newest block at live output. With no target, nothing
  happens. All four act on the focused pane only. Proposed keys:
  `previous_command` and `next_command` are cmd-shift-up and cmd-shift-down
  on macOS, ctrl-shift-up and ctrl-shift-down on Linux.
  `select_command_output` is cmd-shift-a on macOS and has no Linux default,
  because ctrl-shift-a is Select All. `select_command` has no default key.
  Palette entries and the Edit menu get all four.
- **Scroll controller:** `Prompt` cannot be predicted, so it travels as
  its own pending intent:
  - A request is admitted even when the desired offset equals the
    displayed one.
  - Repeated prompt jumps before a request starts add up (`Prompt(a + b)`).
  - A wheel, page, or Live intent that follows a pending prompt jump
    replaces it, since the newer gesture wins.
  - A prompt jump that follows a pending relative intent replaces it as
    well.
  - Typing drops it, as it drops other pending intents.
  - On completion the desired offset becomes the reply's viewport.
- **Sticky header**, in the style `[terminal] sticky_prompt` selects:
  `"command_status"` (H3, the default: the command row plus the record's
  state at the right), `"command"` (H1: the command row only), `"prompt"`
  (H2: every prompt row, at most 3), or `"off"`. It is painted over the
  grid's top row, inside the grid's bounds, with a lifted background from
  the theme (background mixed toward foreground) and a 1px rule. It is
  hidden on the alternate screen, in panes under 6 rows, and when the
  setting is `"off"`. Pressing the left button on it scrolls
  the prompt is at the top, by sending `Prompt(-1)`, which Ghostty resolves
  on the runtime against the current grid. Ghostty finds the nearest prompt
  above the viewport's top, which is the header's prompt. Output or a
  resize between paint and click therefore cannot misplace it, as an
  absolute offset from an older snapshot could. The press starts no
  selection and is not reported to a mouse-tracking program. The
  pointer becomes a pointing hand. A drag that starts below it and moves up
  passes under it and keeps selecting.
- **Failure status**, in the style `[terminal] command_status` selects:
  `"tint"` (T1, the default: a faint red background on failed blocks),
  `"tint_marks"` (T2: T1 plus a 3px mark beside each prompt, green for exit
  0 and red for nonzero), `"bar"` (T3: a 3px red bar along each failed
  block, with no tint), or `"off"`. It is drawn from the
  snapshot's blocks and the latest status. Only `Finished` with a nonzero
  exit code counts as a failure. `Finished` without a code, `Abandoned`, and
  `Running` are never marked as successes, and only exit 0 gets a success
  mark. The tint is a translucent color painted after cell backgrounds and
  before selection, so ANSI backgrounds and selections stay legible. It
  follows the pane's opacity and unfocused tint like other backgrounds.
  Marks and bars sit in the left padding.
- **Settings:** both keys appear under Terminal in Settings, are in the
  schema, and are documented in `docs/configuration.md`.
- **Repaint:** `same_presentation` also compares `commands` and
  `sticky_prompt`. The view also repaints when the status's command records
  change, even with no new snapshot: a D can arrive with no visible row
  changed, and status and snapshot arrive independently in either order.
- **Accessibility:** the accessibility document holds only the displayed
  rows, so the prompt the header shows is not in it. The header is a
  button node next to the terminal node, not text inside it. Its label is
  the command row's text and its state (for example, "Command prompt: rg -n
  TODO crates, running"), and pressing it scrolls to the prompt. The
  terminal document is unchanged, so no text is read twice. The tint is
  visual only. Previous Command is the keyboard equivalent of the
  header.

## Failure paths and edge cases

| Case | Handling |
| --- | --- |
| Records evicted, or capped at 256 | Older prompts have no block. Navigation still reaches them through row marks. |
| Partial eviction: the prompt has left history but the output remains | The record is gone, so the output belongs to no block: no header or tint. |
| Missing D (abandoned) or a D without a code | No tint and no success mark. |
| Running command | Its block ends at the cursor row. The header can show it as running (H3). |
| Resize and reflow | Anchors follow reflow, and positions are recomputed for each snapshot. |
| Anchor remapped by in-place erasure | Fails validation, so the record gets no block. |
| Alternate screen | No blocks, header, or tint. Navigation scrolls nothing, because there is no history. |
| Clear scrollback or reset | Anchors lose their value. The records leave at the next pass, and their blocks disappear. |
| Restored session | Restore starts fresh terminals without history (`mux/capture.rs`), so there is nothing to navigate until new commands run. |
| Several viewers | Blocks and the header come from the shared snapshot. Navigation moves the shared viewport (see Open questions). |
| Pane input sync | The commands are not input, so they are never broadcast. |

## Non-goals

- Per-viewer viewports.
- Copy-output commands, and command text read from the grid (#81).
- Status marks in the scrollbar, a command list, or a command history UI.
- Making tracked anchors survive restore.
- Blocks for prompts without records.

## Verification

| Concern | Evidence |
| --- | --- |
| Block extents: output start, trimmed end, empty prompts between commands, no output, running block ends at the cursor | Core engine tests |
| Positions across wrapped output and resize (narrowing and widening) | Core engine test |
| Partial eviction, record cap, clear scrollback | Core engine tests |
| Missing completion and correct status association by sequence | Core engine tests (Abandoned, Finished without a code, nonzero) |
| Anchor remapped onto replacement text is rejected | Core engine test with ED 2 away from a prompt |
| Sticky prompt: present only while scrolled into output; absent at the prompt, on the alternate screen, and with no record; input row index; multi-line prompt cap | Core engine tests |
| `Prompt` scroll: previous, next, last prompt to live, none found | Core engine test, plus a runtime test for scroll numbering |
| Snapshot cost | `bench:engine` with a full record list, live and scrolled, before and after |
| Wire round trip, caps, digest, vectors | `huterm-wire` tests and vectors; `bench:wire` gate; `mise run fuzz:wire` smoke |
| Controller handling of `Prompt`: admission at an unchanged offset, accumulation, replacement by wheel and Live, typing | `scroll/tests.rs` |
| Header click under output arriving between paint and execution | Core runtime test: `Prompt(-1)` after more output lands the header's prompt at the top |
| Aliased anchors: a newer prompt drawn on an older anchor's row, and `scrollback = 0` | Core engine tests |
| Output without a final newline before the next prompt | Core engine test |
| Theme change while the header shows | Core engine test: header rows recolor |
| Status before or after the snapshot, D with no row change | GPUI view tests |
| Header accessibility node and label | GPUI accessibility tree test |
| Two viewers of one terminal | Core runtime test with two viewers; behavior per Open question 3 |
| Target choice, selection ranges, header hit-testing and click scroll, no selection or mouse report from a header press, tint failure-only rules | GPUI view tests (`view_tests`) |
| A drag that passes under the header still selects | GPUI view test |
| Settings toggles and schema | config tests and the schema check |
| Native: navigate and select in a real shell, while another pane does not move | `smoke:macos-input` extension, or a desktop harness test if the smoke cannot drive it; manual check on Linux in the GNOME VM if needed |
| Tint legibility with themes, selection, ANSI backgrounds, opacity | Screenshots: dark and light themes, a selection, an ANSI background, a translucent window; compared with the chosen mockup |

## Open questions

None. Key bindings and setting names follow the proposals above unless
review finds a conflict.
