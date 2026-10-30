# Pane title bars

Status: implemented. Jim approved the configuration and hover design on
2026-10-30. PR 1 (pane names and the title bar menu) merged in #329. On
2026-10-30 Jim folded the planned PR 3 (`hover`) into PR 2, so the position,
`hidden`, and `hover` modes shipped as one pull request, #340. The
follow-ups at the end list what that delivery left uncovered.

Split panes gain names and a context menu on their title bars, and the bars
gain a position and a visibility mode:

1. A custom pane name, set with Rename Pane, which carries over when a pane
   becomes a tab and when a single-pane tab becomes a pane.
2. A right-click menu on the pane title bar with the pane's actions.
3. `[panes] title_bar_position = "top" | "bottom"`.
4. `[panes] title_bar = "visible" | "hover" | "hidden"`, where `hover`
   overlays the terminal and fades in when the pointer reaches the bar's
   area.

## Current state

Observed on `main` at `d3739b2f`:

- `PaneInfo` (`crates/huterm-protocol/src/pane_layout.rs`) holds a pane's
  identity, terminal, and launched program name. Sessions, workspaces, and
  tabs have custom names in core (`Mux::rename_*`), carried to clients by
  `*Renamed` hierarchy events; panes have none.
- `WorkspaceView::pane_title_bar` (`desktop/windows/tab_bar.rs`) draws the
  bar with a left-press handler only. A pane's label is
  `PaneView::label`: the terminal title through `[tabs] label` and
  `[tabs] directory`, never a custom name. A tab's label is its custom name,
  else its focused pane's label.
- `move_pane` to a new tab (`extract_pane` in `crates/huterm-core/src/mux/moves.rs`)
  opens the tab with no custom name. `move_tab_to_pane` and a move of a
  single-pane tab's pane beside another pane drop the removed tab's name.
- Layout undo (`mux/restore.rs`) restores recorded layouts and keeps each
  existing tab's current name; a tab it reopens gets its recorded name.

## PR 1: pane names and the title bar menu

### Approach

**Storage.** `PaneInfo` gains `custom_name: Option<String>`. The name lives in
the layout, so every move, swap, and arrangement carries it with the pane, and
layout snapshots record it. Core owns it; the protocol type stays
dependency-neutral.

**Rename.** `Mux::rename_pane(pane, name)` mirrors `rename_tab`: it rejects a
blank name, foreign or missing panes, changes nothing for an equal name, and
emits `HierarchyEvent::PaneRenamed { pane, custom_name }`. The reducer updates
the pane in its tab's layout and touches the tab, not its layout, so no
geometry resync follows. The digest encodes a pane's custom name after its
fallback name. `same_shape` and `same_structure` compare panes by identity and
terminal, so a rename never makes an arrangement, move, or undo refuse with
`ChildrenChanged` or `LayoutChanged`.

**Command.** `rename_pane` is a Runtime command after `rename_session` in the
catalog (palette ties fall back to catalog order, and the palette smoke's
`rename` query expects Rename Tab). Arguments: `name`, and `pane` (Optional,
prompted), which the palette defaults to the window's focused pane and the
window fills from it when a binding omits it. A blank name clears the custom
name, as for tabs. No default key binding.

**Label precedence.** A pane's label is its custom name, else today's label
(terminal title or directory through `[tabs] label`). A tab's label is its
custom name, else its focused pane's label, which now includes that pane's
custom name. The window title follows the tab label. Palette pane rows show
the custom name first and prefill it for Rename Pane.

**Carry-over.** Names move between levels; they are never copied:

| Operation | Name effect |
| --- | --- |
| A pane of a split tab moves to a new tab (drag to the bar, `move_pane_to_new_tab`) | The new tab takes the pane's custom name; the pane's is cleared. |
| A single-pane tab nests into another tab (`move_tab_to_pane`, reorder pane mode) | The pane takes the tab's custom name; with none, it keeps its own. |
| A single-pane tab's pane moves beside a pane of another tab (`move_pane_to_pane`) | As above, because that tab is removed. |
| A multi-pane tab nests into another tab | The tab's name is dropped, as today; pane names travel. |
| A lone pane moves to a new tab in another workspace | The whole tab moves; no name changes. |

**Undo.** A restore gives each pane its recorded name unless it was renamed
since, when it keeps its current name, the rule `restored_zoom` applies to
zoom. Undoing a pane-to-tab move therefore returns the name to the pane, and a
rename made after the move survives an undo. The rule crosses the boundary a
move carried the name over: a single-pane tab that the restore removes passes
a rename made since to its pane, unless that rename cleared the name and the
pane has a newer one, and a named tab that the restore reopens around one pane
takes a name the pane was given since, so a name is never dropped or copied.

**Menu.** A right press on a pane title bar, including its buttons, focuses
the pane and opens the pane menu at the pointer, as a left press focuses it
and the tab strip opens a tab's menu. `MenuKind::Pane { tab, pane }`; rows
come from a pure `pane_menu.rs` next to `tab_menu.rs`, in the current
`listbox::Model` style:

- Rename Pane…, Copy Directory Path
- Split Right, Split Down
- Zoom Pane or Unzoom Pane, Move Pane to New Tab, Move Pane to Another Pane…
- Close Pane

Rows show their shortcuts. Picks run on the focused pane, which is the menu's
pane; a pick whose command takes an optional `pane` gets the menu's pane
explicitly, and tab-taking picks get its tab. The menu closes when its pane
leaves the active tab. Opening refuses where the tab menu refuses. The shared
dispatch in `windows.rs` changes only to add the kind, its model, and the pane
argument.

### Observable success

- Rename Pane from the bar menu, the palette, or a binding sets the bar's
  title; a blank name restores the terminal title.
- Dragging a renamed pane to the tab bar makes a tab with that name; dragging
  a renamed single-pane tab into a pane makes a pane with that name. Undo
  reverses both, names included.
- A tab without a custom name shows its focused pane's custom name.

### Failure paths

- A pane closed or moved away while its menu is open closes the menu.
- A rename whose pane vanished before the worker ran reports a stale target.
- A blank name is never stored; the executor maps it to `None`.

### Non-goals

- Names in client layout restoration (pane layouts are not restored yet).
- Inline rename on the bar; renaming uses the palette, as tabs do.
- Native `NSMenu` presentation; the native context menu work adapts this menu.

### Verification

| Concern | Evidence |
| --- | --- |
| Reducer applies, rejects a missing pane, touches only the tab | protocol hierarchy unit tests |
| Digest covers pane names; shape and structure ignore them | protocol unit tests |
| `rename_pane` set, clear, blank, stale, foreign; executor maps the target | core mux and command tests |
| Each carry-over row, and undo and redo of each | core moves and restore tests |
| A rename after a move survives its undo | core restore test |
| Menu rows, zoom label, unknown directory | `pane_menu.rs` unit tests |
| Right press focuses the pane and opens the pane menu; a pick renames through the palette | GPUI window test |
| Palette defaults the focused pane, prefills its name, and move pickers still exclude it | palette tests |
| Bar, tab, and window labels follow the name | GPUI window test |
| Catalog, schema, and keybinding docs stay aligned | slot tests over every command, schema generation check |
| The bar menu and rename flow look and behave right on macOS | Codex computer-use UX review on a scratch bundle |

## PR 2: position, `hidden`, and `hover`

### PR 2 agreed design

- `[panes] title_bar_position = "top" | "bottom"`, default `top`, live, with a
  new position enum rather than `TabPosition`.
- `[panes] title_bar = "visible" | "hover" | "hidden"`, default `visible`,
  live. `hidden` reserves no space and draws no bar; close, zoom, and move stay
  available as commands and in the terminal menu.
- `hover` reserves no space; the bar overlays the terminal. The trigger is the
  bar's full 24-point area with a 200 ms enter delay, matching the scrollbar's
  hover intent. Input reaches the terminal until the bar reveals;
  terminal-owned gestures passing through are never interrupted.
- A keyboard focus change reveals the focused pane's bar for about 1 s, as
  the tab bar's `reveal_for_activity`. A pane drag or the bar's open menu,
  Huterm's or a native `NSMenu`, holds it revealed.
- The fullscreen auto-hide tab bar's top trigger wins over top-row bars.
- `layout_panes`, `pane_rect`, `fit`'s minimums, `tab_panes`, `pane_grids`,
  the resize span, and the smoke state read one shared bar placement.
- Settings: Window › Panes rows, the position row hidden while the bar is
  `hidden` (`x-huterm-visible-when`, as for `pill_accent`).
- In `hover` and `hidden` modes the focused pane gets a thin accent line on
  its edge.

### PR 2 approach

**Config.** `PanesConfig` gains `title_bar: PaneTitleBarMode` (`visible`,
`hover`, `hidden`) and `title_bar_position: PaneTitleBarPosition` (`top`,
`bottom`), lowercase serde enums with schema labels, in Window › Panes before
Pane zoom, both `x-huterm-apply = "live"`.

**Settings condition.** `x-huterm-visible-when` takes a list of `equals`
conditions, any of which shows the setting (#328), so the position row lists
`visible` and `hover`.

**Geometry.** `pane_geometry::TitleBars`, built from `[panes]`, says where a
split tab's bars go and how much each takes from its pane's frame.
`layout_panes` takes it; `pane_rect` puts a visible or hover bar along the
frame's top or bottom; a visible bar takes its height from the terminal,
while a hover bar overlays it and a hidden one has no rectangle. `PaneMetrics`
carries it, so `fit`'s minimum pane height subtracts only a reserved bar.
Every caller (`tab_panes`, `pane_grids`, the resize span, the layout commands)
builds it from the window's config, so a reload re-syncs every view's
geometry and applies the keys live. A tab of one pane still draws no bar.

**Hover reveal.** `pane_bars::PaneBarReveals` keeps one `Reveal` per pane
with the time the pointer entered its bar, and turns entry into hover once
the 200 ms intent passes. The window feeds it after every pointer event, from
the same deferred refresh as the tab bar, with each drawn bar's band, whether
a pane drag or that pane's menu holds it, and the pointer it last saw. The
pointer counts as nowhere while a terminal owns a gesture, outside the
window, or over the fullscreen tab bar's overlay or its top trigger edge.
`cycle_pane` and `focus_neighbor` reveal the newly focused pane's bar for a
second. The window's animation schedule merges the bars' fades and intent
deadlines.

**Pointer pass-through.** `TerminalView.tab_overlay` becomes a pair of
overlay areas, the tab bar's and the pane's revealed bar, which
`overlay_blocks_pointer` and the link, drop, and press paths read alike. A
hover bar draws, and blocks the terminal beneath it, only while its reveal
is above zero; a gesture the terminal already owns still wins.

**Drawing.** A bottom bar draws its border along its top edge. A hover bar
draws at its reveal's opacity. In `hover` and `hidden` modes the focused pane
of a split tab that shows more than one pane draws a 2-point accent line
along the edge its bar runs along, above the terminal, taking no pointer
input; a revealed bar covers it.

**Docs.** `docs/configuration.md` describes both keys and the hover
behaviour; the desktop input guide records the reveal rules and the overlay
pass-through.

### PR 2 observable success

- `title_bar_position = "bottom"` draws every split pane's bar at its bottom,
  with its title, buttons, drag, and menu working as at the top.
- `title_bar = "hidden"` draws no bars, gives terminals the full pane height,
  and marks the focused pane with an accent line.
- `title_bar = "hover"` gives terminals the full height, reveals a bar after
  the pointer rests in its band for 200 ms, keeps it while the pointer, a
  drag, or its menu stays, and fades it out after; keyboard focus moves
  reveal the new pane's bar briefly.
- All keys apply on reload without restarting, and Settings shows the
  position row only while the bar is not `hidden`.
- The defaults draw exactly what `main` draws today.

### PR 2 failure paths and risks

- A pane shorter than a bar keeps the bar clipped to its frame and an empty
  terminal, at either edge, as the top bar does today.
- Minimum-size checks use the reserved height, so `hover` and `hidden` let a
  split go one bar's height smaller, and a reload to `visible` can leave panes
  under the minimum until resized, as a font change can today.
- A selection drag that crosses a bar's band neither reveals it nor loses
  its release; a revealed bar never takes a gesture the terminal owns.
- A pane closed while its bar is revealed drops its reveal.
- A reload during a divider drag re-syncs geometry through the normal path.

### PR 2 non-goals

- A `toggle_pane_title_bars` command, per-layout overrides, or a translucent
  bar background.

### PR 2 verification

| Concern | Evidence |
| --- | --- |
| Top, bottom, hover, hidden, zoomed, and one-pane rectangles | `pane_geometry` unit tests |
| `fit` minimums with and without a reserved bar | `pane_geometry` unit tests |
| Config defaults, round trip, unknown values | config tests |
| The position row's condition | settings model test |
| Hover intent, hold, activity reveal, pruning | `pane_bars` unit tests |
| Overlay pass-through, held gestures | `overlay_blocks_pointer` tests |
| Reload to bottom, hidden, and hover moves bars and resizes grids | GPUI window test |
| Hover reveal after the intent, menu hold, keyboard reveal | GPUI window test |
| Pane close and zoom buttons at the bottom | GPUI window test |
| Defaults unchanged for native input | `smoke:macos-input` |
| Bars, cue, and fades in both themes; native menu from a bottom bar; Settings rows | Codex computer-use UX review |

## Follow-ups

- The pane title bar's title and its zoom and close buttons are missing from
  the accessibility tree; the buttons are not `Button`s yet. This predates
  the series.
- No automated test covers the fullscreen auto-hide tab bar's priority over
  top-row hover bars, or a terminal-owned selection drag crossing a bar's
  band. The Linux test platform never reports a hovered window, so window
  tests inject the sampled pointer.
- A native `NSMenu` holding its hover bar revealed, and the native menu's
  position under a bottom bar, were checked only by inspection.
