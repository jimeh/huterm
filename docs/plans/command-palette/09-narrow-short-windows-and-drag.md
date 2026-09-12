# Chunk 9: narrow and short windows, scrollbar drag

Part of [the command palette plan](../command-palette.md). Fixes three
issues found by hand after chunk 8: row text wrapping in narrow windows, the
panel not shrinking in short windows, and a scrollbar thumb click that
leaves the palette in a drag state.

Owner: Codex implementer. Files: `crates/huterm-gpui/src/desktop/palette/mod.rs`,
`scripts/check-palette.ts` and `crates/huterm-gpui/src/desktop/palette_smoke.rs`
only if a smoke step is added. Do not touch the terminal view, quake modules,
`text_field.rs`, or anything under `third-party`.

## Invariants (AGENTS.md)

- GPUI element `on_mouse_move` and `on_mouse_up` fire only while the
  element's hitbox is hovered. Register drag tracking through
  `Window::on_mouse_event` during canvas paint to receive movement and
  release outside the element and outside the window. `text_field.rs`'s
  render does exactly this for its own drag; copy the pattern.
- `block_mouse_except_scroll` on a hitbox stops every hitbox painted before
  it, including ancestors, from being hovered (`hit_test` in
  `third-party/vendor/gpui-0.2.2/src/window.rs` sets `hover_hitbox_count`
  at the blocking hitbox). Handlers on the panel or overlay therefore do not
  fire while the pointer is over the scrollbar strip.
- Rows are exactly `ROW_HEIGHT` tall; the smoke relies on that pitch.

## Fixes

### 1. Truncate row text with an ellipsis

`render_search` rows and `render_slots` picker rows. The text column is
`div().flex_1().min_w_0()` with a title and a description child.

- Description and picker label/detail: add `.truncate()` (GPUI's
  `Styled::truncate` sets nowrap, ellipsis, and hidden overflow) to each
  text child. Keep the column's `min_w_0()`; add `.overflow_hidden()` to
  the column as well.
- Title with match highlights: `highlighted_title` builds one div per
  character in a flex row, which cannot ellipsize. Replace it with a single
  `gpui::StyledText::new(title).with_highlights(&window.text_style(),
  highlights)` where `highlights` is one `(byte_range, HighlightStyle)` per
  matched character (accent colour, semibold), wrapped in a
  `div().truncate().text_size(px(13.5))`. `title_indices` are byte offsets
  into the title; build each range as `index..index + ch.len_utf8()`.
  Merge adjacent matched characters into one range. `render_search` needs
  `window: &Window` to read the text style; thread it from `render`.
- Give the trailing `meta` column `.flex_none()` so key caps and the scope
  label never shrink or wrap; the text column gives way instead.

Test: a unit test for the highlight-range builder (`tfs` against "Toggle
Fullscreen" yields ranges for `T`, `F`, and `s`; adjacent indices merge).
Visual truncation is verified by the orchestrator.

### 2. Clamp the panel to the window height

`render`: the list uses a fixed `LIST_MAX_HEIGHT`. Compute the available
list height from the viewport instead:

```rust
const PANEL_CHROME_HEIGHT: f32 = PANEL_MAX_HEIGHT - LIST_MAX_HEIGHT;
let viewport = f32::from(window.viewport_size().height);
let available = viewport - PANEL_TOP_INSET * 2.0 - PANEL_CHROME_HEIGHT;
let list_max = available.clamp(ROW_HEIGHT + LIST_PADDING * 2.0, LIST_MAX_HEIGHT);
```

Pass `list_max` into `render_search` and `render_slots` and use it for the
list's `max_h`. Keep at least one full row visible. For centred placement,
`panel_top` must use the clamped panel height
(`list_max + PANEL_CHROME_HEIGHT`), not `PANEL_MAX_HEIGHT`, so a short
window still centres the panel; give `panel_top` a `panel_height`
parameter. Test `panel_top` with a height below the panel's maximum for
both placements.

### 3. Scrollbar thumb click and drag release

Symptoms: clicking the thumb leaves the palette dragging; while dragging,
pointer movement outside the window stops updating. Cause: the strip blocks
the panel's and overlay's `on_mouse_up` and `on_mouse_move` from the hover
set, and those handlers only ever ran while hovered anyway.

Fix in `render`:

- Remove the drag `on_mouse_move` and `on_mouse_up` listeners from the
  overlay and the panel (keep the overlay's `on_mouse_up` that only stops
  propagation, and the panel's, without the release call).
- Add a `canvas` child to the `palette-list-frame` (absolute, `inset_0`,
  painted after the strip) whose paint closure, when
  `self.scrollbar_drag.is_some()` at render time, registers
  `window.on_mouse_event::<MouseMoveEvent>` and
  `window.on_mouse_event::<MouseUpEvent>` in the bubble phase that update
  the palette entity through a cloned `Entity<CommandPalette>` (use
  `cx.entity()` in `render`), calling `scrollbar_drag_to(event.position)`
  and `scrollbar_release()`. This is the `text_field.rs` pattern; it works
  outside the window because the listeners are window-level.
- `scrollbar_press` stays on the strip. Ensure the strip does not need its
  own `on_mouse_up`.
- A plain click on the thumb must end with `scrollbar_drag == None` after
  the release and no scroll change.

Test: if the palette smoke harness has native mouse press, move, and
release commands (inspect `scripts/check-palette.ts` and
`palette_smoke.rs` for `clickOverlay`, `mouse-down`, `mouse-move`,
`mouse-up` or similar), add a step after step 7 that presses on the
scrollbar thumb, moves the pointer below the window's bottom edge, expects
the list offset to change (expose `scroll_offset` in the smoke state
string if needed), releases, and expects `scrollbar_drag=false` in the
state string. If the harness only supports clicks, add a step that clicks
the thumb and asserts `scrollbar_drag=false` afterwards, and say in the
report that the outside-window drag is a manual check.

## Verification

```sh
cargo test --locked -p huterm-gpui palette
bash scripts/build-exec.sh cargo clippy --locked --workspace --all-targets \
  --features gpui/runtime_shaders -- -D warnings
cargo fmt --all --check
mise run check:scripts
```

The native smoke needs a display; the orchestrator runs it. Report which
commands you could not run.

## Self-review checklist

- Rows keep their fixed height and the smoke's row geometry.
- No handler on the panel or overlay is relied on for scrollbar release.
- No vendored, terminal view, quake, or text field files changed.
