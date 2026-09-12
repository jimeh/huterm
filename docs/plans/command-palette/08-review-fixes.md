# Chunk 8: dual-review fixes

Part of [the command palette plan](../command-palette.md). Fixes the twelve
confirmed findings from the dual review of PR #91 at `17ab00f`. Requires
chunks 1 to 7 merged.

Owner: Codex implementer. Dependencies: none beyond the branch head. Files:
`crates/huterm-gpui/src/desktop/palette/{mod.rs,slots.rs}`,
`crates/huterm-gpui/src/desktop/windows.rs`,
`crates/huterm-gpui/src/ui/text_field.rs`, `crates/huterm-gpui/src/keymap.rs`,
`crates/huterm-protocol/src/command.rs` (test only), `scripts/check-palette.ts`,
`docs/plans/command-palette.md` (one sentence). Do not touch the terminal
view, quake modules, or anything under `third-party`.

## Invariants (AGENTS.md)

- Never read a window handle or the dispatching window's root view inside
  its action handler; GPUI panics and AppKit aborts. Entity reads are fine.
- Never lock Mux from rendering or input callbacks.
- Every palette keyboard operation is a `Palette`-scope catalog command
  forwarded from `CommandPalette::run`; nothing is hardcoded in components.
- GPUI `cx.emit` is delivered after the current update returns, not inline.
- Text-field cursor changes must not emit `Changed`; only content changes do.

## Fixes

Numbers match the review report. Do them in this order; each is small.

### 3. Forward `text_delete_word_forward` from the palette

`palette/mod.rs`, the `ids::TEXT_DELETE_BACKWARD | ...` arm in `run`: add
`ids::TEXT_DELETE_WORD_FORWARD`. Add a test in `mod.rs` that every
`text_*` catalog command except `TEXT_DELETE_BACKWARD`'s pop special case
is accepted by `run` (build a palette in a test `App` if one exists; if the
module has no entity tests, assert instead that the forwarded id list,
extracted into a `const FORWARDED_TEXT_COMMANDS: &[CommandId]`, covers
every catalog command whose id starts with `text_`).

### 4. Empty slots contribute no value to the invocation

`slots.rs`, `SlotEditor::invocation`: include a slot's value only when its
state is `Committed`, `Prefilled`, `Sole`, or `Explicit`. `Empty` keeps its
value as the editing preselection only. Check `complete()`,
`runs_without_prompt`, and the `runs` hint in `render_slots` still behave;
the hint should read "run" when the active text is non-empty or the
remaining required slots are satisfied by non-Empty values. Test: type a
name, `next_slot` to the tab, `previous_slot`, `set_text("")`, `next_slot`,
then `commit` on the tab picker; the result must be `Commit::Next` back to
the name (name is `Always`), not a run with the discarded name.

### 12. No panic for commands whose arguments are all unprompted

`slots.rs`: `SlotEditor::new` can yield zero slots. Make `expand_search` in
`mod.rs` treat "no prompted slots" like "no arguments" (return without
entering the slot stage), and make `active()` callers safe by having
`SlotEditor::new` callers check `slots().is_empty()` before `enter_slots`.
Add a protocol test in `command.rs` that every non-`Palette` command with
arguments has at least one prompted argument, so a future catalog entry
fails loudly instead of at runtime.

### 7. Make the `syncing` guard real

`palette/mod.rs`: the `Changed` subscription runs after `syncing` has been
reset because `emit` is deferred. Replace the flag with a comparison: in the
subscription, read the field text and return early when it equals the
stage's current text (`search.query` or `editor.text()`). Remove the
`syncing` field and every assignment. Keep `prefill_name` and the retained
query path working: they set the editor text and the field text to the same
value, so the comparison short-circuits.

### 9. Select-all must not re-rank

`text_field.rs`: `select_all` (the command) must only `cx.notify()`, like
`move_cursor`. Keep `select_all_text` as is. Test in `mod.rs` or
`text_field.rs` is not required beyond the existing buffer tests; state in
the report that you checked `TEXT_SELECT_ALL` no longer emits `Changed`.

### 10. Snap pointer hits to grapheme starts

`text_field.rs`, `TextField::index_at`: replace the snapping expression with
the largest grapheme start that is `<= index`:

```rust
let index = self
    .buffer
    .content
    .grapheme_indices(true)
    .map(|(start, _)| start)
    .take_while(|start| *start <= index)
    .last()
    .unwrap_or(0);
```

Remember `closest_index_for_x` can return `content.len()`; keep the `.min`
before snapping and allow `content.len()` as a result. Test: content
`"e\u{301}x"` (e plus combining acute), an index of 1 snaps to 0, an index of
3 stays 3, an index of 4 stays 4.

### 8. Clear pending Enter on every editing action

`palette/mod.rs`: set `self.pending = None` in `edit_slot`, `next_slot`,
`previous_slot`, and for every forwarded text command (movement and
selection included), before dispatching. The plan says any further input
clears the intent. Extend `pending_domain_defers_the_commit` or add a
sibling test: queue `Pending::Commit`, call `next_slot`, then land the
hierarchy; nothing executes.

### 11. Picker key caps

`palette/mod.rs`, the `select_tab` picker rows: request shortcuts with an
exact argument match only. In `shortcut_keys`, when `args` is `Some`, filter
bindings with `binding.matches_arguments(args)` rather than allowing
argument-free bindings. Then map positions to `select_tab` indices the way
`select_index` in `windows.rs` does: index 9 means the last tab. Show the
`index = 9` key cap on the last row only, positions 1 to 8 on rows 1 to 8,
and nothing on rows 9 and up when there are more than nine tabs. Test with
a ten-tab hierarchy: row 9 has no cap, row 10 shows the index-9 binding, and
a configured bare `select_tab` binding appears on no row.

### 2. Scrollbar strip owns its pointer events

`palette/mod.rs` render: rows extend under the scrollbar strip, so a press on
the thumb is also a press on the row and the release fires the row's
`on_click`. Add an absolutely positioned strip element inside the
`palette-list-frame`: `div().id("palette-scrollbar").absolute().top_0()
.bottom_0().right_0().w(px(width))` with `.block_mouse_except_scroll()`
(the `InteractiveElement` method that sets `HitboxBehavior::BlockMouseExceptScroll`
in the vendored GPUI 0.2.2), rendered only while `indicator.opacity > 0.0`,
where `width` is the expanded width when `scrollbar_expansion.active()` and
the normal width otherwise. Move the `on_mouse_down`, `on_mouse_move`, and
`on_hover` handlers from the frame onto this strip so rows never see strip
presses; keep the overlay's move and up handlers for the drag. Rows beneath
the strip must not highlight on hover while the strip is visible. Verify by
reading `hit_test` in `third-party/vendor/gpui-0.2.2/src/window.rs`; do not
edit vendored code.

### 5. Ignore completions and events from a dismissed palette

`windows.rs`: add a `palette_generation: u64` field incremented in
`open_palette`. Capture the generation in the hierarchy task and, in its
completion, return early when `self.palette` is `None` or its generation no
longer matches (store the generation on the palette entity or beside
`self.palette` as `Option<(Entity<CommandPalette>, u64)>`; the latter avoids
touching the palette). In `handle_palette_event`, return early when the
event's `palette` is not the current entity (`Entity::entity_id`
comparison). In the `Cancel` path nothing else is needed once stale events
are ignored. Test in `windows.rs` if an entity test harness exists there;
otherwise cover it in `scripts/check-palette.ts` with the existing
`busy-on` or deferred-hierarchy mechanism if one exists, and if neither is
practical say so in the report.

### 6. Fill unresolved identity defaults when the hierarchy lands

`palette/mod.rs`, `set_hierarchy`: after updating `self.target`, for each
slot in the editor that is `Empty`, has no value, is an identity kind, and
is `Optional`, fill it from `domain.default(kind)` and mark it `Prefilled`.
Then run `prefill_name`. Add `SlotEditor::fill_default(index, value)` or
similar; do not mutate slots from `mod.rs` directly. Test: `rename_session`
editor built with `target.session == None`, then a hierarchy arrives with a
session that has a custom name; the session slot is `Prefilled` and
`current_name` returns that name. Also cover `rename_workspace` when the
workspace is already known (no change).

### 1. Confirmation takes precedence over the palette

`windows.rs`: wherever a close confirmation is presented (`close.confirmation`
becomes `Some`), dismiss the palette first as a cancel: clear `self.palette`,
`palette_refresh_state`, retain the query as the cancel path does, and do
not restore focus to the terminal (confirmation owns focus). Find the single
function that installs the confirmation and add the dismissal there, so
Dock Quit, shell exit with confirmation, and window close all go through it.
Add a smoke step to `scripts/check-palette.ts` only if the harness already
has a confirmation trigger (a `busy-on` child that requires confirmation);
otherwise state the manual test in the report.

## Verification

```sh
cargo test --locked -p huterm-gpui
cargo test --locked -p huterm-protocol
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --all --check
mise run check
mise run check:scripts
```

The native palette smoke needs a display and Metal; the orchestrator runs
it. If `cargo test -p huterm-gpui` cannot build in your sandbox, say so in
the report and list which tests you could not run.

## Self-review checklist

- No `AnyWindowHandle` reads and no Mux locks added on the UI thread.
- `Empty` slots never reach an invocation.
- The scrollbar strip blocks row hover and clicks but not wheel scrolling.
- No terminal view, quake, or vendored files changed.
- Every fix has either a test or a stated manual check.

Report: files changed, test names and results, deviations with reasons, and
anything that could not be verified in the sandbox.
