# Pane title bars

Status: in progress. Jim approved the configuration and hover design on
2026-10-30. PR 1 (pane names and the title bar menu) is in progress; PR 2
(position and `hidden`) and PR 3 (`hover`) follow, each from `main` after the
previous one merges.

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
a name given to it since to its pane, and a named tab that the restore reopens
around one pane takes a name the pane was given since, so a name is never
dropped or copied.

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

## PR 2: position and `hidden` (agreed design)

- `[panes] title_bar_position = "top" | "bottom"`, default `top`, live, with a
  new position enum rather than `TabPosition`.
- `[panes] title_bar = "hidden"` reserves no space and draws no bar; close,
  zoom, and move stay available as commands and in the terminal menu.
- `layout_panes`, `pane_rect`, `fit`'s minimums, `tab_panes`, `pane_grids`,
  the resize span, and the smoke state read one shared bar placement.
- Settings: Window › Panes rows, the position row hidden while the bar is
  `hidden` (`x-huterm-visible-when`, as for `pill_accent`).
- The focused pane gets a thin accent line on its edge while bars are hidden.

## PR 3: `hover` (agreed design)

- `title_bar = "hover"` reserves no space; the bar overlays the terminal.
- The trigger is the bar's full 24-point area with a 200 ms enter delay, the
  scrollbar's `HOVER_INTENT`. Input reaches the terminal until the bar
  reveals; terminal-owned gestures passing through are never interrupted.
- A keyboard focus change reveals the focused pane's bar for about 1 s, as
  the tab bar's `reveal_for_activity`. A pane drag or the bar's open menu
  holds it revealed.
- The fullscreen auto-hide tab bar's top trigger wins over top-row bars.
- `TerminalView.tab_overlay` generalizes to a list of overlay areas, reusing
  `tab_visibility`'s `Presentation` and `Reveal`.
- The focused-pane accent line from PR 2 applies here too.
