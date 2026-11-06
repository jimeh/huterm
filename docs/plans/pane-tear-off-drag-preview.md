# Pane tear-off and the floating drag preview

Status: implemented. Delivers
[#401](https://github.com/jimeh/huterm/issues/401), a follow-up to
[cross-window drags](cross-window-drags.md). Jim settled the decisions below
on 2026-11-04 and picked the mockups on 2026-11-05.

## Objective

A pane dragged out of every Huterm window opens in a new window, as a tab
already does. On macOS a small preview window follows the pointer whenever a
tab or pane drag leaves its source window, so the gesture shows what a
release does even where no Huterm window is drawn. Linux, and macOS with the
preview turned off, show the outcome with in-window cues.

## Decisions

| Topic | Decision |
| --- | --- |
| Pane tear-off | Every platform. Releasing a pane outside every Huterm window opens a new regular window with that pane as its only tab, through `move_pane` with `MoveDestination::NewTab` into the new window's workspace. A lone pane in a window's only tab is a no-op and shows no cue. A pane from a one-tab window with several panes tears off. |
| Command | `Move Pane to New Window` (`move_pane_to_new_window`) in the command catalog, palette, and pane context menu, with no default binding. It moves the focused pane, like `Move Pane to New Tab`, and shares the tear-off path. Unavailable, with a reason, for the lone pane of a window's only tab. |
| In-window cue (S1) | While a release would tear off a pane and no preview shows, the source pane draws its contents at 45%, a full-contrast dashed outline (as the dimmed tab slot does), and a centred `↗ New window` pill, the icon-only `↗` pill when the pane is too small for the label. Every platform. The tab bar pill is unchanged. |
| Preview over another Huterm window (C1) | The preview window draws the existing title chip, unchanged: same size, padding, radius, fill, 90% opacity, 12 points from the pointer, flipping across each screen edge it would overflow. The destination keeps its insertion marker, drop region, title-row region, and spring outline, and stops drawing its own chip. |
| Preview outside every Huterm window (M1, changed) | When a release would tear off: a 176×112 miniature window centred under the pointer, with traffic lights, a bar whose only tab is the dragged title, an empty terminal area, and an accent border. No pill inside it. When a release would do nothing (a lone tab or pane, a Huterm window that takes no drop such as Settings, a failed native query): the plain chip, dimmed, centred under the pointer. Another application's window counts as outside, as it does for tab tear-off today. |
| Preview scope | macOS only, a GPUI `WindowKind::PopUp` window. It shows whenever the pointer is outside the source window during a tab or pane drag that has a cross-window record. Inside the source window nothing changes. |
| Pills while the preview shows | The source's tab bar `↗ New window` pill and the S1 pane cue are not drawn while the preview is visible. If the preview cannot open, they draw as on Linux. |
| Setting | `[window] macos_drag_preview = true`: Settings → Windows → Window, label "Floating drag preview", advanced (`x-huterm-advanced`), `x-huterm-platform = "macos"`, applied live. Description: "Show a preview window under the pointer while a tab or pane is dragged outside its window. Off uses the in-window cues." |
| Linux | No floating window. The tab bar pill and S1 are the tear-off cues. |

## Constraints

- Everything in [cross-window drags](cross-window-drags.md#constraints)
  still binds: runtime-scoped IDs, generic Mux operations committed on a
  worker, both endpoints reserved before dispatch, no reads of a window that
  is on GPUI's update stack, `huterm-protocol` and `huterm-core` free of GPUI
  types, and the capture-listener rules in
  [desktop input](../agents/desktop-input.md).
- No core or protocol change beyond the new `CommandId` and its spec.
- The preview never takes focus, key status, or mouse events, never
  activates the application, and never appears in window lists, the Window
  menu, smoke window indices, activation order, or `CrossWindows::windows`.
- Native AppKit calls follow
  [window presentation](../agents/window-presentation.md): main thread
  only, frame setters outside GPUI borrows (as `window_locator::place`).
- Smoke presses on tabs or panes wait for `drawn_tab_ids` to match
  `tab_ids` (the #402 rule in [desktop smokes](../agents/desktop-smokes.md)).

## Approach

### Pane tear-off and the command

- `finish_cross_drag`'s `Located::Outside` arm tears off a pane payload when
  `pane_tear_off_available` holds: the window has two or more tabs, or the
  payload's tab has two or more panes. `tear_off_cue` uses the same rule
  for panes and keeps the tab rule (`tabs.len() > 1`) for tabs.
- Generalize `tear_off` to both payloads. After the destination window opens, a
  pane payload dispatches a `Transfer::Pane(PaneMove::Move { .. })` to
  `MoveDestination::NewTab { workspace, before: None }` in the new window's
  workspace; a tab payload keeps `Transfer::Tab`. Window-creation and move
  failures roll back exactly as tab tear-off does; notices say "Move pane"
  instead of "Move tab". The pending transfer lists the one moved pane and
  focuses it.
- Release drops the `PaneDrag` before the asynchronous window creation, and
  `dispatch_transfer` reads spring-back from a live `PaneDrag`. Capture the
  pane drag's restoration state (its `back` spring-back target and the bar
  scroll) in `tear_off` before anything is awaited, and restore it on
  session-creation, window-creation, and move failures, as `transfer_scroll`
  does for reorders.
- macOS placement: a pane tear-off places the new window as a tab tear-off
  does, with the pointer over its tab bar near the start (use the pane
  drag's press offset into the title bar as the grab, clamped to the bar).
  The command uses default placement.
- `move_pane_to_new_window`: catalog spec next to `MOVE_PANE_TO_NEW_TAB`,
  its Settings keybinding group in `settings/keybindings.rs` (the catalog
  test requires one), dispatch next to `MOVE_TAB_TO_NEW_WINDOW`, the pane
  context menu entry after "Move Pane to New Tab" (disabled with the
  reason for a lone pane in the only tab), palette availability through `check_pane_tear_off_available`
  mirroring `check_tab_tear_off_available`. Update `docs/keybindings.md`.

### Source-pane cue (S1)

- In `drag_overlay`'s `cross_outside` arm, when `tear_off_cue` holds for a
  pane payload and no preview is visible, draw over the source pane's drawn
  frame (`drawn_frame(pane)`): a 45% dim layer in the terminal colour, a 1pt
  dashed outline in the tab foreground at full contrast inset like
  `drop_box`, and the pill centred, using `floating_colors` and the bar
  pill's label rule (`tear_off_label`, compact `TEAR_OFF_COMPACT` width when
  the pane is narrower than `TEAR_OFF_PILL` plus margins). Record its bounds
  in a `drawn_pane_tear_off` cell, as `drawn_tear_off` does, and publish it
  in smoke state.
- A modifier-grab drag gets the same cue; it does not depend on the pane
  title bar.

### Floating preview (macOS)

- **State.** `CrossWindows` gains a `DragPreview` record: the GPUI handle of
  the preview window (if open), its native window number, the mode last
  drawn, and whether it is natively visible. The preview opens when a drag
  first needs it and closes when the drag record ends. It is never retained
  between drags, so `cx.windows().is_empty()` guards (`maybe_exit`, Dock
  reopen) and other window iteration never see it outside a drag. A failed
  open marks the drag as preview-less, and the in-window cues draw.
- **Mode.** A pure function of the drag record and the setting:
  `None` (setting off, not macOS, no record, or pointer inside the source),
  `Chip` (located in another Huterm window), `Window` (outside and a release
  would tear off), or `Dimmed` (outside, blocked, or a no-op release). Unit
  test it.
- **Sync.** One `sync_drag_preview` runs after every change to the drag
  record: `route_cross_drag`, `end_cross_drag`, `finish_cross_drag`,
  `closed`, and a setting change. It defers through `cx.defer` so native
  frame changes happen outside GPUI borrows, updates the preview view's
  title and mode, sizes and moves the native window (chip: measured chip
  size, origin 12 points from the pointer with per-axis flips clamped to the
  pointer's screen `visibleFrame`; miniature and dimmed chip: centred on the
  pointer), and shows it with `orderFront:`, never
  `makeKeyAndOrderFront:`. Screen points come from
  `window_locator::screen_point` on the source at the event's pointer, never
  a cached cursor.
- **Window.** `WindowKind::PopUp`, `focus: false`, `show: false` at open,
  transparent background, not movable or resizable. GPUI still gives a
  PopUp `Titled | FullSizeContentView` with no titlebar, so after open set
  the style mask natively to borderless plus `NonactivatingPanel`, and hide
  the standard window buttons if any remain. Also natively:
  `setIgnoresMouseEvents: YES`, `setHasShadow: NO` (the view
  draws its own shadow inside a margin), and keep GPUI's popup level and
  `CanJoinAllSpaces | FullScreenAuxiliary`. Its root is a small
  `DragPreviewView`, not a `WorkspaceView`, so every `downcast::<WorkspaceView>`
  path skips it. Code that indexes or counts windows during a drag
  (`smoke_state`, `cross_drop_label`, the Window menu) must enumerate
  terminal windows only; audit the `cx.windows()` uses that index or count.
- **Locator.** `window_locator::locate` on macOS starts the hit test below
  the preview: `windowNumberAtPoint:belowWindowWithWindowNumber:` with the
  preview's window number when one is open (0 otherwise). Ignoring mouse
  events alone is not enough, because the miniature sits under the pointer.
  If the topmost window at the point is ever the preview anyway, skip it and
  query again below it.
- **Destination chip.** While the preview is natively visible in `Chip`
  mode, the destination's `drag_overlay` incoming arm draws targets but no
  chip. A wanted but failed or not yet shown preview leaves the chip.
- **Pills.** `tear_off_cue`-driven pills (bar and S1) are suppressed while
  the preview is visible.
- **Lifecycle.** Hidden on release, Escape, blur of the source, an opening
  menu, a close request, quit, and when either window closes, because each
  ends or clears the drag record and every record change syncs. A tear-off
  closes the preview before the new window opens.

### Setting

Add `macos_drag_preview: bool` (default `true`) to `WindowConfig` in
`huterm-config` with the extensions above, regenerate
`schemas/huterm.schema.json`, document it in `docs/configuration.md`, and
read it live from the source window's config at sync time.

## Failure paths

| Case | Handling |
| --- | --- |
| Lone pane in the only tab, outside every window | No cue, no preview miniature (dimmed chip on macOS), release does nothing. |
| Pane closed, moved, or exited mid-drag | Existing validity checks end the drag; the preview hides. |
| Window creation or move fails during pane tear-off | As tab tear-off: no move or rollback of the empty session, notice "Cannot open the destination window" / "Cannot move terminal", pane stays. |
| Preview window fails to open | Log it, and draw the in-window cues and the destination chip for that drag; drags work. |
| Pointer passes over the preview itself | The locator searches below it, so targets and tear-off resolve as if it were absent. |
| Preview mode changes mid-move (outside → over a window) | Each sample re-locates below the preview and syncs mode, size, and position from that sample. |
| Setting toggled during a drag | The next sync uses the new value; turning it off hides the preview and the pills return. |
| Source or destination closes mid-drag | `closed` clears the record and syncs, hiding the preview. |
| Quit or terminating | No tear-off (existing guard); the preview closes with the app. |
| Fullscreen source or destination, other Spaces | PopUp collection behaviour keeps the preview visible over fullscreen windows. Not smoke-covered; the UX review checks a fullscreen source on the current Space and a visible Quake source. |

## Non-goals

- A live terminal thumbnail in the miniature (GPUI has no offscreen capture).
- A floating preview on Linux.
- Moving a pane or tab to an existing window by command.
- Animating the preview between modes.

## Verification

| Concern | Evidence |
| --- | --- |
| Pane tear-off success: terminal, process, history, and ID survive; source keeps its other panes | Window test with the fake locator (`window_tests/cross_window.rs`), asserting membership, IDs, focus, and activation after install. |
| Lone pane no-op and no cue | Window test: `tear_off_cue` false, no S1 drawn, release changes nothing. |
| Pane from a one-tab window with several panes | Window test. |
| Window-creation failure and move failure roll back with a notice | Window tests reusing `fail_window` and the worker-hook injected move failure. |
| `Move Pane to New Window` success, availability reason, palette and pane menu entry | Window test for the command; `pane_menu` unit test for the entry and its disabled state; catalog test if the catalog has one for entries. |
| S1 drawn only while outside, available, and preview not visible; compact pill in a small pane | Window tests reading `drawn_pane_tear_off`. |
| Preview mode function | Unit tests for every mode and the setting-off and non-macOS cases. |
| Destination draws no chip in `Chip` mode; pills suppressed while visible | Window tests with a test hook that marks the preview visible. |
| Smoke indices and window lists skip the preview | Window test opening the preview window and checking `smoke_state` indices and the Window menu. |
| Preview closes at every drag end, so exit and reopen guards are unaffected | Window tests for release, Escape, and either window closing, asserting the preview window is gone. |
| Destination keeps its chip when the preview failed to open | Window test with an injected open failure. |
| Pane tear-off failure restores spring-back and scroll | Window test: spring-load another tab, tear off with an injected failure, assert the original tab and scroll return. |
| Locator skips the preview under the pointer | `smoke:macos-input`: a tab drag that goes outside (miniature under the pointer), then onto the second window's slot; assert `w1.drop=window:1:slot:0` and the release lands there. |
| Preview is natively shown, placed, and never key | Cross-window smoke state publishes `preview` (`none`, `chip`, `window`, or `dimmed`) plus native readback: `preview_visible` (`isVisible`), `preview_frame` (screen frame), and `key_window` (the key window's number). `smoke:macos-input` asserts: outside, mode `window`, visible, frame centred on the pointer's screen point; over the second window, mode `chip`; the source stays key throughout; after release and after Escape, no preview; with `macos_drag_preview = false` (live config reload), mode `none` and the source's `tear_off` cue. |
| Native macOS pane tear-off and focus | `smoke:macos-input`: pane tear-off to empty screen; three windows, stable terminal IDs, the source stays active during the drag (the preview never becomes key). |
| Native Linux pane tear-off and S1 | `smoke:linux-input`: pane tear-off and the S1 cue's published bounds while outside; `preview=none` throughout. |
| Setting and schema | Config tests for the default and parsing; `mise run check` covers schema drift. |
| Experience | `ux-review` on macOS through Codex computer use on a scratch bundle and config, compared with mockups M1 (as changed) and S1: the chip, miniature, dimmed chip, S1, the setting off, light and dark themes. Linux S1 by a manual check in `vm:linux:dev` or Jim's check. |
| Gates | Focused `cargo test -p huterm-gpui`, `mise run check`, `mise run verify` before handoff. |

## Documentation

Update `docs/agents/desktop-input.md` (pane tear-off, the preview and its
locator rule), `docs/agents/window-presentation.md` (the PopUp preview's
native handling), `docs/agents/desktop-smokes.md` (the `preview` field),
`docs/configuration.md`, `docs/keybindings.md`, and the status and non-goals
of [cross-window drags](cross-window-drags.md), which this plan supersedes
for pane tear-off and the floating drag image.

## Open questions

- None blocking. C1 keeps its 12-point offset (coordinator recommendation,
  Jim did not object); centring it would cover the targets under the
  pointer.
