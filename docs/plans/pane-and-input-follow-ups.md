# Pane and input follow-ups

Status: implemented

Six follow-ups to the pane layout, context menu, and modifier drag work,
delivered in one PR on `origin/main` at `2f58b14a` or later.

## Constraints

- Parallel threads own Settings structure (with short schema descriptions
  and new defaults for `inactive_dim`, `size_indicator_linger_ms`, and
  `drag_modifier`), input sync across panes (keyboard routing), and themes.
  This PR adds schema keys and enum variants but does not restructure
  Settings. New schema descriptions are one sentence where possible.
- `docs/agents/desktop-input.md` rules hold: macOS Control-left remapping,
  the cursor rule from #343, focus-first clicks, and the modifier-drag
  `held` record.
- Native code stays in the existing macOS modules and is gated on
  `target_os = "macos"`. Nothing reaches `huterm-protocol` or `huterm-core`
  except the new built-in layout.
- No test, window test, or CI run plays audio.

## Decisions

Jim chose a drill-in submenu for Huterm's own menu and a cascading
`NSMenu` submenu for native menus. He also accepted Main Centered in the
default cycle, the macOS-only audible bell, and wheel routing to unfocused
panes only in an active window, all in one PR.

## 1. Main Centered layout

- Add `("main_centered", "Main Centered", "columns(rows(*), pane 50%,
  rows(*))")` to `BUILT_IN_LAYOUTS` after `main_top`, making it
  `[NamedLayout; 6]`. The default `layout_cycle` lists every built-in, so
  it includes the new layout.
- Panes fill in order, so the centre pane is pane `ceil((n - 1) / 2) + 1`
  and shifts as panes are added. With two panes the right stack is empty
  and the panes take one third and two thirds. The template holds up to 513
  panes, so it stays available where previously only `tiled` was.
- Docs: the `arrange_panes` row and the pane layouts table in
  `docs/keybindings.md` ("five" becomes "six"; the "swap a pane first to
  make it the main pane" advice gains the centre rule), the module docs in
  `layout_template.rs`, `docs/configuration.md#pane-layouts`, the
  `layout_cycle` doc comment, and the generated schema.
- Tests that encode five built-ins change: the protocol table, the cycle
  index and `tried` lists, the 258-pane "only tiled" cases in protocol and
  `pane_layouts.rs`, the two-pane distinct count, window and Settings tests
  that list the cycle, and `scripts/check-settings.ts`.
- Risk: a config with a user layout named `main_centered` now fails to
  load as a whole, since built-in names are reserved. Accepted; say so in
  the PR.

## 2. Arrange Panes submenu in the terminal menu

Spec:

- `menu_spec::Row` gains `Submenu { item: Item, rows: Vec<Row> }`.
- `terminal_menu` takes the layouts as a separate argument (its input is
  `Copy`) and adds "Arrange Panes" right after "Balance Panes", only when
  the tab is split, as Balance Panes is. Its rows are
  `PanesConfig::layouts()`: the built-ins in cycle order, then user layouts
  by name, as the palette lists them. TOML table order is not kept by the
  loader, so this is the config's order. Each item's id is
  `arrange_panes:<name>` and its label the layout's title. A layout that
  cannot hold the tab's pane count is disabled with "Holds at most N
  panes", using the existing `capacity` helper.
- `run_menu_pick` maps `arrange_panes:<name>` to `arrange_panes` with
  `layout = <name>` through `invoke_interactive`, so refusals, undo labels,
  and notices match the palette.

Native renderer:

- `NativeItem::Submenu { title, enabled, items }`. `appkit_menu::build`
  builds a child `NSMenu` with `setAutoenablesItems: NO` and the same
  appearance, and attaches it with `setSubmenu:`; the parent item has no
  action.
- Tags number pick items depth-first across the tree; `id_at` resolves a
  tag with the same numbering. `check_built` and `check_items` recurse.
- `smoke:macos-menus` builds a native terminal menu for a split tab with
  one user layout and checks the submenu's titles, tags, and `id_at`. The
  test presenter's `ids()` and `pick()` helpers recurse.

Huterm's own renderer (drill-in):

- `listbox::Row::Submenu { item, rows }`. The row shows `›` in place of a
  shortcut. A click (on release, as picks are), Enter, or Right on it shows
  a "‹" back row titled with the submenu's label, followed by its rows.
  Left or the back row returns to the parent with the submenu row
  selected; Escape closes the menu.
  Picks report the submenu item's id as usual.
- `menu_select_right` and `menu_select_left` also open and leave a
  submenu; update their titles and `docs/keybindings.md`.
- The open submenu is keyed by its row's id, so `set_model` on refresh
  keeps it; if that row disappears, as when the tab stops being split, the
  menu returns to the top level.
- Placement measures the rows the menu shows. To keep the panel still when
  drilling in, place it by the taller of the top level and its submenus.
- A test proves the release that opens a submenu never picks the row that
  appears under the pointer.
- Every `listbox::Row` match handles the new variant, including
  `menu_items_state` and the menu test helpers.

## 3. The wheel reaches programs in unfocused panes

- Add a wheel-only route in `TerminalView::scroll` that does not require
  pane focus. Press, motion, and release keep `application_mouse`'s focus
  requirement. The window must still be active, matching #343's cursor
  rule. Shift still forces local scrolling.
- Scrolling never changes focus. Presses keep focus-first behaviour.
- A wheel over a neighbouring pane during one of Huterm's own drags
  reaches that pane's program, as local scrolling already does. Accepted.
- Update `desktop-input.md`'s pointer section.

## 4. Option as Meta per side

- `MacosOptionAsAlt` gains `Left` and `Right` (`"left"`, `"right"`),
  labelled "Left Option key" and "Right Option key". The key stays
  macOS-only; Linux keeps sending Alt chords as Meta.
- A pure `option_sends_meta(policy, side, alt) -> bool`: `both` sends Meta
  whenever Alt is held; `left` and `right` need the matching side; `off`
  never does. `keyboard::translate` takes the decision, and
  `handle_keystroke`'s composition path uses the same decision in place of
  `option_as_alt != Off`, so the other Option key still composes.
- The side comes from `[NSApp currentEvent].modifierFlags`'
  device-dependent bits, `NX_DEVICELALTKEYMASK` (0x20) and
  `NX_DEVICERALTKEYMASK` (0x40), read beside `translate_current` in
  `native_quit/option.rs`. It applies the same checks: a key-down for this
  window, and an Option bit that agrees with the keystroke's Alt. It never
  fails; anything else means the side is unknown. `handle_keystroke` runs
  inside the native key dispatch, so the current event is that key-down.
- Docs: `docs/configuration.md#option-and-alt-as-meta` and the starter
  config comment. Remapping tools and Screen Sharing can omit the device
  bits, and then `left` and `right` send no Meta.
- `smoke:macos-input` posts Option-letter key-downs with `0x80000|0x20`
  and `0x80000|0x40` under `left` and `right`, and checks Meta against
  composed text.

## 5. Audible bell

- `[terminal.bell]` gains `audio = false`, `sound = "system"`, and
  `volume = 0.5`, a float from 0 to 1, validated for range and NaN.
  `BellConfig` and `TerminalConfig` drop `Eq` for the float.
- `sound` is an enum: `system` plus `basso`, `blow`, `bottle`, `frog`,
  `funk`, `glass`, `hero`, `morse`, `ping`, `pop`, `purr`, `sosumi`,
  `submarine`, and `tink` from `/System/Library/Sounds`, labelled "System
  alert" and by name.
- macOS only. The three keys carry `x-huterm-platform = "macos"`. On Linux,
  `audio = true` loads with a warning; it joins any existing config warning
  rather than replacing it.
- `system` calls `NSBeep()`, which plays the user's alert sound at macOS's
  alert volume, so `volume` applies only to named sounds. The volume
  description says so. Named sounds play through `NSSound` with
  `setVolume:`, loaded once and cached by name. A sound that fails to load
  falls back to `NSBeep()` and is logged once.
- Playback runs on the main thread, deferred out of the snapshot update:
  `-[NSSound play]` and `NSBeep` return at once. AppKit does not document
  `NSSound` as thread-safe, so no worker thread.
- An app-wide throttle on the GPUI side admits at most one sound per
  200 ms across all terminals, using an injectable clock in its tests. A
  terminal's bells in one update play once. Every terminal's bell plays,
  focused or not: hidden panes already drain bells. The visual bell is
  unchanged.
- Playback goes through a sink trait on `Desktop`, like `menu_presenter`;
  tests install a recording sink, so nothing plays in tests.
- Settings rows in Terminal's "Bell and clipboard" group, orders 71 to 73:
  "Audible bell" (switch), then "Bell sound" (select) and "Bell volume"
  (slider), both shown only while `audio` is on.
- Real playback is checked by hand, briefly and quietly.

## 6. Drags end on a remapped release

- On macOS, GPUI turns a Left release with Control held into a Right
  release, so a drag whose combination lacks Control never ends. Only on
  macOS, a Right release ends a Left-started window drag (divider, sidebar,
  reorder, pane drag, tab scrollbar) when no real Right press is held.
- While one of these drags runs, the window's press capture listener
  records a Right press and stops it, so no menu opens mid-drag; its
  release clears the record and ends nothing.
- Decide whether a release ends a drag first, then pass that to
  `pane_grab::released_held`: any drag-ending Right release records its
  modifiers plus Control, as a `remapped` pane drag's does.
- Pure decision function tested for both platforms. A macOS window test
  feeds the remapped release and fails before the fix; `smoke:macos-input`
  posts a real Left down, a Control `flagsChanged`, and a Left up with
  Control over a divider drag to prove GPUI's remap ends it.

## Verification

| Behaviour | Evidence |
| --- | --- |
| Main Centered parses and expands for 1 to 6 panes | protocol layout test |
| Default cycle includes it; arrange applies it | config default test; window test via `arrange_panes` |
| Submenu rows: split only, order, ids, capacity hints | `terminal_menu` tests |
| Native submenu tags and `id_at` | `menu_spec` tests; `smoke:macos-menus` |
| Drill-in open, back, refresh, pick, no pick on opening release | `huterm-ui` menu and listbox tests |
| Picking a layout arranges the tab, both renderers | window tests |
| Wheel over an unfocused reporting pane reaches it, focus unchanged; motion does not | window test with an active window |
| Option decision for each policy and side | unit tests; `smoke:macos-input` |
| Bell config parse, range, Linux warning | config tests |
| Bell throttle and sink | unit test with a controlled clock; view test with a recording sink |
| Drag ends on a remapped Right release; a real right click does not end it | window test that fails before the fix; `smoke:macos-input` |

Run `mise run check`, the focused tests, `smoke:macos-input`,
`smoke:macos-menus`, and `mise run verify` before handoff.

## Non-goals

- Linux audible bell, custom sound files, per-terminal bell settings, and
  making `volume` apply to the system alert sound.
- Cascading submenus in Huterm's own menu.
- Holding back wheel reports during Huterm's own drags.
