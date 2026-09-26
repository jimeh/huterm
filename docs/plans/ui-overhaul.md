# UI overhaul

Status: agreed on 2026-09-26 from the [final mockup][mockup]. Jim chose the
"palette family" direction from the [direction prototype][directions] and
refined it through later mockups. This plan implements what the final mockup
shows. The mockup is the visual reference; this document is the behavioural
contract. The [placement mockup][placement] explored where the `⋯` button goes
for side tab bars without a title bar. Jim chose its option B, with the
refinement described under [Window menu button](#window-menu-button); that
mockup still draws `+` as a small square, so this document governs.

[mockup]: https://plans.jimeh.dev/fdpdvp7p64t667xsdrk2vrnz2m/huterm-ui-menu.html
[placement]: https://plans.jimeh.dev/kps4l62x7hkml62piwmksobkmi/huterm-ui-placement.html
[directions]: https://plans.jimeh.dev/dawd3fkwfofewjklunkknsbp7i/huterm-dialogs-overlays.html

## Outcome and scope

Give Huterm's window chrome and transient UI one visual and keyboard grammar,
borrowed from the command palette: raised theme-derived panels, rows with an
accent bar, visible key hints, and catalog commands for every keyboard action.

The overhaul revamps these existing elements:

- **Close confirmation** for a tab, a window, or Quit: a keyboard-operable
  dialog that lists the processes it will end.
- **Status bar**: replaced by dismissible notices with severity, source, and
  actions.
- **Scroll label**: a clickable pill showing the position within the total
  scrollback.
- **Size label**: a raised panel.
- **About**: a themed panel instead of the native alert on macOS and GPUI's
  unthemed fallback prompt on Linux.
- **Window title**: the native title follows the active tab, and the macOS
  title strip shows it instead of a static label.

It adds these missing elements:

- **Window menu**: a `⋯` button at the far right of the title bar, opening a
  menu of common actions, including the command palette, About, and Check for
  Updates. On Linux it is the application's only menu.
- **Tab context menu** on right-click, with rename, copy-directory, and close
  actions, including closing other tabs or the tabs after one.
- **Tabs in the title bar**, as a `tabs.position` option.
- **Running indicator**: a small dot on tabs whose program holds the
  foreground.
- **Keyboard access** to menus, notices, and dialogs through catalog commands,
  plus a Linux `ctrl-,` binding for Open Settings.

The command palette itself does not change: its search, ranking, empty-query
order, row geometry, and bindings stay as they are. The menu only opens it.

Out of scope: a right-click menu inside the terminal, a tab and terminal
switcher, persisting notices across restarts, prompt-aware close checks
through shell integration, GPUI's Wayland backend, detecting a Linux user's
window-button layout, and a Linux updater.

## Behaviour today

References are against `cdb720d`.

- **Close confirmation.** `WorkspaceView::render`
  (`crates/huterm-gpui/src/desktop/windows.rs`, near the `cancel-close` and
  `confirm-close` element IDs) draws a bordered box over a 90% scrim. Its
  buttons are clickable divs with no focus state, and a hardcoded `on_key_down`
  branch accepts only Escape. The message names the tab but not its processes,
  and the window case says "Close this final view".
- **Close scope.** `CloseTarget` is `Tab`, `Window`, or `Application`, and
  `merge_close` widens a pending target. Core's `CloseRequest` closes one tab,
  one window view, a session, or the application. `close_tab` takes no
  argument and closes the active tab.
- **Status strings.** `WorkspaceView.status` and `TerminalView.status` are
  `Option<String>`. `combine_config_diagnostics` joins config errors, keymap
  errors, binding conflicts, and warnings with `"; "`, and command, close, and
  fullscreen failures overwrite the same field. Each view renders it as a
  full-width bottom bar that cannot be dismissed.
- **Labels.** `scroll_position_label` ("1,284 lines up") and the resize
  indicator ("80 x 24") in `TerminalView::render`
  (`crates/huterm-gpui/src/desktop.rs`) use the terminal background with no
  border.
- **Title bar.** macOS windows use a transparent title bar with full-size
  content. `titlebar_inset` reserves a 32-point strip (`TITLEBAR_HEIGHT`)
  outside fullscreen, painted with a static "Huterm" label 84 points from the
  left and marked `WindowControlArea::Drag`. On Linux the window manager draws
  the title bar.
- **Menus.** `install_menus` returns early on Linux, so Linux has no menu at
  all. Tabs have no right-click or middle-click handling.
- **About.** The `about` executor calls `window.prompt` with the version and
  `APP_ID`. macOS shows an NSAlert. X11 has no native prompt, so GPUI renders
  its own unthemed fallback.
- **Updates.** `check_for_updates` works only in updater-enabled macOS release
  builds (`macos-updater` feature); elsewhere it returns an unavailable error.
- **Linux bindings.** `keymap::defaults` binds the palette to `ctrl-shift-p`
  but has no Settings binding.
- **Build identity.** Nothing embeds a source revision in the binary.

## Upstream changes that affect this plan

The plan was revised after rebasing onto `cdb720d`.

- Foreground process detection (#160) replaced `ps` with `huterm-procinfo`.
  `jobs::evidence` computes a display command, using argv for a non-shell root,
  but `JobProcess` exposes only `identity` (start time plus kernel name).
- Smart tab labels (#160) changed `Tab::title` to take `TabsConfig`. Dialogs,
  menus, and the title bar name tabs through it.
- Quake (#159) and fullscreen scheduling use events and one earliest-deadline
  timer per owner. Notice expiry uses the window's `Animated` schedule and adds
  no periodic work, especially to hidden Quake windows.

## Platform findings

- **GPUI drag regions do nothing on macOS or X11.** Only the Windows backend
  implements `WindowControlArea` hit testing. The macOS title strip drags
  because AppKit handles the title-bar region natively. With tabs there, AppKit
  may start a window drag when a tab reorder begins.
- **Linux client-side decorations need a compositor.** GPUI's X11 backend
  supports `WindowDecorations::Client` through `_MOTIF_WM_HINTS`,
  `start_window_move` and `start_window_resize` through `_NET_WM_MOVERESIZE`,
  `show_window_menu`, and `set_client_inset` through `_GTK_FRAME_EXTENTS`.
  Without a compositing manager it falls back to server decorations, so Huterm
  must read `window.window_decorations()` rather than assume success. Huterm
  builds only GPUI's `x11` feature, so GNOME Wayland sessions run it through
  XWayland, which Mutter composites.
- **CI has no compositor.** Linux smokes run Openbox under Xvfb, where
  client-side decorations fall back.
- **Popovers.** GPUI's `anchored` and `deferred` elements position a menu
  relative to a trigger and paint it above later siblings.

## Design specification

Every colour derives from the active theme through `Theme::ui()` and the
terminal palette. Raised surfaces mix the terminal background toward the
foreground and carry a border and shadow, so they stay distinct over output in
light and dark themes.

### Close confirmation

- Opens only when an assessment finds running or unknown processes; idle
  closes proceed without a dialog, as today.
- A 60% scrim covers the window. The panel has an explicit viewport-clamped
  width, and its text and button containers do not shrink (AGENTS.md).
- The header shows a severity mark, a title, and a subtitle:

  | Target | Title | Primary button |
  | --- | --- | --- |
  | One tab | `Close "<tab title>"?` | Close Tab |
  | Several tabs | `Close <n> tabs?` | Close Tabs |
  | Window | `Close this window?` | Close Window |
  | Application | `Quit Huterm?` | Quit |

  The subtitle states how many processes will end; for a window it adds that
  the window's session closes too. For several tabs it says how many of them
  are busy, or `All <n> have running processes`. When any
  state is unknown, it says so in plain words and the mark changes from `!` to
  `?`.
- Rows show the display command in monospace, its PID, and "Foreground" or
  "Background job" with the command line. An accent bar marks foreground
  processes. Dialogs covering more than one tab group rows under tab headings,
  unknown-state tabs first, with a yellow "unknown" tag.
- At most six process rows show, then an "and N more" row.
- The footer holds only two buttons: `Cancel esc` and a destructive primary
  button with `↩`. It has no other hints.
- The primary button has focus when the dialog opens, so Enter confirms and
  Escape cancels. Tab, Shift-Tab, Left, and Right move focus between the
  buttons, with a visible focus ring. Pressing the close shortcut again does
  **not** confirm.
- Cancelling returns focus to the terminal, whose jobs keep running.

### Notices

- Up to three toasts stack at the bottom right of the terminal bounds, newest
  nearest the corner, with a "+N more" chip above them. When the scroll pill is
  visible, the stack sits above it.
- Each toast has a severity mark and stripe (error red, warning yellow, info
  accent), a bold title, the message, an optional source location in
  monospace, action links, and a dismiss button.
- Config and keymap notices persist and are keyed by source. A reload replaces
  every notice from its source: a fixed file clears them, and a file that still
  fails raises them again, even after dismissal.
- Command and terminal failures expire after about six seconds, with a thin
  progress line along the bottom edge. Hovering or focusing pauses expiry.
- Terminal failures from any tab, including hidden ones, join the window's
  stack and name their tab.
- While notices are waiting, the `⋯` button shows a yellow dot (hidden while
  its menu is open), and the window menu shows "Show Notices" with a count,
  which runs `focus_notices`.
- Keyboard: "Focus Notices" moves focus to the newest toast. Up and Down move,
  Enter runs the first action, and Escape or Delete dismisses.

### Scroll pill and size panel

- Scrolled back, a pill at the bottom centre of the terminal bounds reads
  "↑ 1,284 of 9,870 lines", then a divider and "Jump to live" with the
  `scroll_to_bottom` binding. Both numbers come from the displayed snapshot:
  its offset and its retained history rows. Clicking runs `scroll_to_bottom`;
  the pill consumes the press so it cannot start a selection or application
  mouse input. It uses the existing indicator hold and fade.
- While the grid size changes, a centred panel shows `columns × rows` in large
  monospace with a "columns × rows" caption, reusing `resize_visibility`.

### Window menu button

- A Lucide `ellipsis` icon at 14 points in the standard 26-point control, with
  a "Menu" tooltip. `window.menu_button = false` removes it; every item stays
  reachable through shortcuts or the palette.
- With a title bar, it takes the title bar's rightmost slot:
  - macOS windowed: the title strip, in every tab position.
  - Linux with tabs in the title bar: the drawn title bar, left of the window
    controls.
- Without a title bar, meaning fullscreen on either platform or a Linux window
  whose title bar the window manager draws, it joins the tab bar:
  - A top or bottom tab bar: its far right end. A menu opened from a bottom
    bar opens upward.
  - A left or right tab bar: beside `+`, in the row that follows the last tab.
    Today `+` spans that row's full width, less the row margins. The row keeps
    its height and position; `+` gives up its right end to a square 26-point
    `⋯` control after a small gap and fills the remaining width with its icon
    still centred. The row follows the last tab and pins to the bottom when the
    column overflows, so the button moves with it, and its menu opens below the
    button or above it once the row sits in the lower half.
  - With a title bar, or with `window.menu_button = false`, `+` keeps the full
    row.
  - When the tab bar is hidden, such as an auto-hidden fullscreen bar that is
    not revealed, the button is hidden with it. The `open_menu` command still
    works.

### Window menu

Items, top to bottom, with separators between groups:

1. Command Palette…
2. New Tab, New Window
3. Rename Tab…, Close Tab, Close Window
4. An "Edit" row holding Copy and Paste buttons; Copy is disabled with a
   "Nothing selected" tooltip when the active terminal has no selection. Then
   Toggle Fullscreen.
5. Open Settings, Reload Configuration, and "Show Notices" with a count badge,
   present only while notices are waiting.
6. About Huterm, then "Check for Updates…" only in updater-enabled macOS
   release builds.
7. Quit Huterm

Shortcuts appear right-aligned in each platform's format, derived from the
compiled keymap so user bindings show. Items with no binding show none.

Rename Tab… runs `rename_tab` for the active tab, which collects the name in
the palette's existing argument prompt, prefilled with the current name.

macOS duplicates these items deliberately. The menu bar keeps Minimize, Zoom,
Hide, Hide Others, Show All, Services, Next Tab, Previous Tab, Switch to Last
Tab, Select Tab, and the scroll commands; all stay in the palette.

### Tab context menu

Right-clicking any tab opens a menu at the pointer, clamped to the window. It
does not activate the tab, and it outlines the targeted tab while open.

1. Rename Tab…: `rename_tab` for that tab.
2. Copy Directory Path: copies the tab's known directory. Disabled with a
   "Directory unknown" hint when none is known.
3. Close Tab: `close_tab` for that tab. It shows its shortcut only when the
   tab is active.
4. Close Other Tabs: disabled with only one tab.
5. Close Tabs to the Right, or "Close Tabs Below" for vertical tabs: disabled
   on the last tab.

Closing several tabs runs one assessment across them. If none is busy they
close at once; otherwise one "Close N tabs?" dialog confirms them together.

### Menu behaviour

Both menus share one component with the palette's styling: 28-point rows,
separators, a selected-row highlight in the accent colour, dim shortcut text,
disabled rows, and the inline button row.

- A pointer opens a menu with no selection; hovering selects. `open_menu`
  opens the window menu with its first item selected.
- Up and Down move between enabled rows, Left and Right between buttons in the
  Edit row, Home and End jump, and typed letters jump to the next item that
  starts with them. Enter or Space runs the selection.
- Escape, Tab, a click outside, or running an item closes the menu. Escape
  returns focus to the `⋯` button for the window menu, and to the terminal for
  the tab menu.
- The window menu opens below its button, or above it when the button is in
  the lower half of the window. It right-aligns when the button is in the right
  half, left-aligns otherwise, and scrolls when taller than the available
  space.
- Menus do not open while a close confirmation is showing.

### About panel

A centred panel on both platforms:

- The app icon, "Huterm", and `Version <x.y.z>`.
- A details list: Identifier (`APP_ID`), Platform (OS version, architecture,
  and on Linux the display backend and whether client-side decorations are
  active), Terminal engine (`libghostty-vt` version), and Build (profile and
  short source revision). Omit the Build revision when none is embedded.
- Buttons `Copy Details`, which copies the list as text, and `OK ↩`. Escape
  and Enter close it.

### Title bar

- `tabs.position = "titlebar"` merges the tab bar into the title-bar row.
  On macOS the row is 32 points, starts after the traffic lights, and keeps
  trailing empty space draggable; double-clicking that space calls
  `titlebar_double_click`. On Linux Huterm draws the whole row through
  client-side decorations: tabs, `+`, draggable empty space, the `⋯` button,
  then minimize, maximize, and close controls on the right.
- `titlebar` resolves to `top` wherever no title bar exists: fullscreen, Quake
  windows, and Linux sessions where client-side decorations fell back.
- With any other position on macOS, the title strip shows the active tab's
  title, centred, instead of "Huterm".
- The native window title becomes `<active tab title> — Huterm` and follows
  tab switches and title changes. Linux window managers show it in their title
  bar, and both platforms use it in window switchers.
- Tabs show a small yellow dot before the title while a program holds the
  foreground, using the existing `foreground_process()` metadata, which is set
  only then. Error, exit, and bell indicators take precedence. Background-only
  jobs, such as `tail -f &` at an idle prompt, show no dot; close assessment
  still finds them.
- Tab-strip overflow keeps its existing scroll arrows. The `⋯` button's
  distinct icon avoids confusion with the adjacent right arrow.

## Commands, bindings, and configuration

All keyboard operations are catalog commands with a required key context,
never hardcoded GPUI handlers (AGENTS.md). Printable type-ahead in menus uses
the same text-input route as the palette's text field.

| Command | Scope and context | Default binding | Status |
| --- | --- | --- | --- |
| `dialog_confirm`, `dialog_cancel` | Window, `confirming` | `enter`; `escape` | New |
| `dialog_focus_next`, `dialog_focus_previous` | Window, `confirming` | `tab`, `right`; `shift-tab`, `left` | New |
| `open_menu` | Window | None; F10 would take a key htop and mc use | New |
| `menu_select_next`, `menu_select_previous` | Window, `menu` | `down`; `up` | New |
| `menu_select_first`, `menu_select_last` | Window, `menu` | `home`; `end` | New |
| `menu_select_right`, `menu_select_left` | Window, `menu` | `right`; `left` | New |
| `menu_confirm`, `menu_close` | Window, `menu` | `enter`, `space`; `escape`, `tab` | New |
| `focus_notices`, `dismiss_all_notices` | Window | None; listed in the palette | New |
| `notice_next`, `notice_previous` | Window, `notices` | `down`; `up` | New |
| `notice_run_action`, `notice_dismiss` | Window, `notices` | `enter`; `escape`, `delete` | New |
| `close_tab` | Window | Unchanged | Gains optional `tab`, default active |
| `close_other_tabs`, `close_tabs_after` | Window | None | New, optional `tab`, default active |
| `copy_tab_directory` | Window | None | New, optional `tab`, default active |
| `open_settings` | Window | Linux gains `ctrl-,` | Changed binding |

Conditional bindings reserve no keys, so terminal input outside a dialog, menu,
or notice focus is unaffected. The new `tab` arguments are not prompted.

Configuration:

- `window.menu_button` (bool, default `true`).
- `tabs.position` gains `"titlebar"`.

Regenerate both schemas and extend `schemas/fixtures.json` with every new
command and option.

## Settled decisions and constraints

- The palette is unchanged.
- Close confirmation focuses the primary button; a repeated close shortcut
  never confirms.
- Menus and dialogs move focus away from terminal action handlers; terminal
  input stays blocked while `confirming` or a menu is active.
- `ChromeLayout` stays the single source of terminal bounds for painting, mouse
  input, scrollbars, and PTY sizing. Title-bar tabs change its inputs, not that
  ownership.
- Check for Updates stays hidden on Linux and in macOS builds without the
  updater until a Linux updater exists.
- One pull request, built as the ordered commits below.

## Implementation sequence

Commit 1 comes first. Commit 3 depends on 2, and 4 on 3. Commit 8 depends on 5
and 7, and 9 on 4 and 7. Commits 12 and 13 depend on 11. Commits 6, 10, and 11
depend only on 1.

### 1. Shared overlay module

Move `Swatch`, `key_cap`, and the footer-hint renderer from
`desktop/palette/mod.rs` into a shared module such as `desktop/overlay.rs`, and
add the raised-panel and scrim primitives. The palette's appearance and smoke
geometry must not change.

Verification: palette unit tests and `smoke:linux-palette` pass unchanged.

### 2. Process names and tab mapping in close evidence

Add a display-command field to `JobProcess`, filled from the command
`jobs::evidence` computes, and leave `identity` unchanged because consent
coverage compares it. Expose the terminal order of `CloseAssessment::jobs` so
the client maps each `JobState` to its tab.

Verification: core unit tests for the root-command and kernel-name cases.

### 3. Close confirmation dialog

Add the dialog commands and replace the inline element with the dialog
specified above. Remove the Escape-only `on_key_down` branch. Keep the
`cancel-close` and `confirm-close` element IDs.

Verification:

- Unit tests for dialog-model grouping, ordering, capping, subtitle copy for
  each target, and the unknown-state mark. Keymap tests that dialog bindings
  match only under `confirming`. An executor test that `close_tab` during a
  pending confirmation still reports "close confirmation pending".
- Extend `smoke:macos-input` and `smoke:linux-input` with a live child. Focus
  Cancel with Tab and press Enter, then prove the PTY is alive with a unique
  shell acknowledgement. Reopen and confirm with Enter.
- Run `smoke:macos-quit`, `smoke:macos-quake`, `smoke:linux-quake`, and both
  fullscreen smokes, which already drive close confirmation.

### 4. Targeted and multi-tab close

- Core: add a multi-tab `CloseRequest` and `CloseEffect` for several tabs in
  one workspace. Its ticket covers every affected terminal, and commit removes
  exactly those tabs or returns `StaleClose` without mutation.
- GPUI: extend `CloseTarget` with a tab set and define `merge_close` for it:
  tab targets union, and window or application targets still win.
- Give `close_tab` an optional `tab` argument, and add `close_other_tabs` and
  `close_tabs_after`, resolving tab sets on the UI thread from current order.

Verification: core tests that the ticket covers every terminal, that commit
removes only the requested tabs, and that structural changes produce
`StaleClose`. GPUI tests for `merge_close` and for tab-set resolution at the
first, middle, and last positions. Extend one input smoke with a two-busy-tab
Close Other Tabs, cancelled and then confirmed.

### 5. Notices

- Replace both status strings with a per-window `NoticeStack` holding severity,
  source, message, location, actions, and lifetime. Remove
  `combine_config_diagnostics`.
- Route terminal failures to the `WorkspaceView` through the per-tab activity
  path. Expiry registers one deadline through the window's `Animated`
  schedule.
- Render the toasts, dot, and badge, and add the notice commands.
- Replace the `status=` fields in the integration, fullscreen, palette, and
  Quake smoke state dumps with a stable serialization. Update
  `check-fullscreen.ts`, `check-palette.ts`, `check-desktop-integration.ts`,
  and `check-quake.ts` to assert severity and message.

Verification: `NoticeStack` unit tests for source replacement, re-raising
after dismissal, expiry order under a controllable clock, hover pause, and
overflow. The updated smokes still prove the fullscreen timeout, config reload
failure, legacy-engine warning, and tab-open failure. On macOS,
`mise run bench:idle` shows no wakeups with a persistent notice and one
deadline for an expiring one; a unit test asserts the same schedule on Linux.

### 6. Scroll pill and size panel

Implement both as specified, replacing `scroll_position_label` and the resize
label.

Verification: unit tests for the pill text from offset and history, including
thousands separators. `smoke:renderer` and `bench:scroll` show no per-frame cost
while hidden. A native smoke clicks the pill and observes offset zero.

### 7. Menu component

Build the shared menu: model rows (items, separators, button rows, disabled
items with hints), `anchored` placement with clamping and scrolling,
outside-click dismissal, focus return, hover selection, type-ahead, and the menu
commands under a `menu` context.

Verification: unit tests for keyboard movement over disabled items and button
rows, type-ahead wrapping, and placement clamping.

### 8. Window menu, button, and bindings

- Add `window.menu_button` and the `⋯` control with the placement rules and
  notice dot. `TabStrip` includes the control in slot geometry, reveal, wheel,
  and drag calculations wherever it joins a horizontal tab bar.
- In a vertical column, split the `new-tab` row into the narrowed `+` target
  and the square `⋯` target. The row's height and `TabStrip`'s reserved slot
  do not change, so strip scrolling, overflow pinning, and drop mapping stay
  as they are.
- Build the window menu from the command catalog, with shortcut text from the
  compiled keymap, the Copy selection state, and the Check for Updates build
  condition.
- Add `open_menu`, and add `ctrl-,` for `open_settings` on Linux. Fill that
  cell in the README binding table.

Verification: unit tests for placement across platforms, positions, and
fullscreen states; for the split row's widths at the minimum and default
sidebar widths, with and without the button; and for item visibility per
platform and build. An input smoke with a left bar in fullscreen clicks both
halves of the row and asserts a new tab and an open menu respectively. Keymap tests
for the Linux binding, including reserved-key tests. `smoke:linux-palette` and
`smoke:macos-palette` open the menu by pointer, run Command Palette…, and
assert the palette opened with unchanged row geometry.

### 9. Tab context menu

Handle right-click on tabs in horizontal and vertical strips without
activating them or disturbing reorder capture. Add `copy_tab_directory` through
the existing clipboard write path.

Verification: unit tests for item enablement at each tab position, the vertical
label, and the absent-directory case. An input smoke right-clicks an inactive
tab, confirms the active tab did not change, and runs Close Tabs to the Right.

### 10. About panel

Replace the `window.prompt` call with the panel. Embed a short source revision
at build time when the build provides one, such as the release workflow's SHA
or `HUTERM_SOURCE_REVISION`.

Verification: a unit test for the details text with and without a revision.
Check visually on macOS and under Linux Xvfb.

### 11. Title-bar position, titles, and running indicator

- Add the `titlebar` variant with one resolution function that `ChromeLayout`,
  `top_chrome_uses_bar`, tab visibility, and the notch shelf all use.
- Show the active tab's title in the macOS strip for the other positions, and
  set the native window title from the active tab with `set_window_title`,
  only when the text changes.
- Add the running dot to the tab indicator precedence in `tab_bar.rs`, and
  include its width in tab-width fitting.

Verification: resolution unit tests for every position, fullscreen state, Quake
window, and decoration outcome. Unit tests for the window-title text and for
indicator precedence. An integration smoke reads the X11 window name after a
tab switch.

### 12. Tabs in the macOS title bar

1. **Spike.** Put a drag-reorderable tab in the transparent title-bar region.
   Record whether AppKit starts a window drag, and whether clicks, close
   buttons, right-click, and double-click behave. If AppKit takes the drag, try
   a tracked GPUI vendor patch that keeps tab presses from moving the window,
   following `third-party/vendor/README.md`, before changing the design.
2. **Build.** Merge the strip and tab bar into one 32-point row, centre the
   traffic lights with `TitlebarOptions::traffic_light_position`, and keep the
   trailing space draggable with `titlebar_double_click` on double-click.

Verification: extend `smoke:macos-fullscreen` to enter and leave fullscreen
with `titlebar`, checking PTY dimensions against the published grid. Manually
check reordering, dragging, double-click, the notch shelf, and a single tab with
`always_show = false`.

### 13. Tabs in the Linux title bar

- Request `WindowDecorations::Client` for `titlebar`, read
  `window_decorations()` after each change, and resolve to `top` on fallback.
- Draw the row as specified. Empty space calls `start_window_move`,
  double-click toggles maximize, and right-click calls `show_window_menu`.
- Set a client inset for resize edges and shadow, map presses in it to
  `start_window_resize`, and drop the inset and rounded corners when GPUI
  reports tiling, maximize, or fullscreen. Quake windows keep their frameless
  path.

Verification: unit tests for fallback resolution and resize-edge mapping. Add a
compositor such as `xcompmgr` to one Linux smoke run with `titlebar`, asserting
that decorations are client-side, a window-control click reaches the window,
and fullscreen restores exact root geometry. Check manually in `vm:linux:dev`
(GNOME on Wayland through XWayland) and `vm:linux:dev:x11`: dragging, resizing
from every edge, maximizing, the window menu, and the no-compositor fallback.

## Success criteria

- Every close confirmation can be completed or cancelled from the keyboard,
  and lists what it ends or names the tab whose state is unknown.
- Every warning and error can be dismissed; config problems return only while
  they persist, and transient failures clear themselves.
- On Linux, a new user can reach the palette, Settings, About, and Quit by
  pointer without knowing a shortcut.
- Tabs can be renamed, closed, and bulk-closed from their context menu.
- `tabs.position = "titlebar"` works on macOS and composited Linux sessions and
  falls back to `top` everywhere else without layout errors.
- The palette's behaviour and smoke geometry are unchanged.
- No new periodic work while idle, and no change in snapshot or paint budgets.

## Documentation

- README: the new commands in the catalog, dialog, menu, and notice binding
  tables, the Linux `open_settings` binding, `window.menu_button`, and the
  `titlebar` position.
- AGENTS.md: the dialog and menu contexts, the notice replacement rule, the
  `titlebar` resolution function, and the platform findings above.
- Record the implementation outcome at the end of this plan.

## Mockup coverage

Each behaviour in the final mockup maps to a section above. Mockup-only
controls, such as the prototype toolbar, "Label parts", "Many tabs", and the
`P`, `M`, `W`, and `Q` stand-in keys, are not product behaviour.

| Mockup behaviour | Plan |
| --- | --- |
| Close dialog copy, rows, grouping, cap, two-button footer, focus, keys | Close confirmation; commit 3 |
| "Close N tabs?" from the tab menu, idle tabs closing at once | Tab context menu; commit 4 |
| Toasts, stripes, "+N more", expiry line, stack above the pill, dot, badge | Notices; commit 5 |
| "↑ 1,284 of 9,870 lines" pill with Jump to live | Scroll pill; commit 6 |
| Centred `columns × rows` panel while resizing | Size panel; commit 6 |
| `⋯` placement for each platform and position, tooltip, config toggle | Window menu button; commit 8 |
| Placement mockup option B: `⋯` beside `+` for side bars without a title bar | Window menu button; commit 8 |
| Top and bottom bars in fullscreen with `⋯` at the far right, menu opening upward from the bottom | Window menu button and Menu behaviour; commits 7 and 8 |
| Window menu items, Edit row, conditional Show Notices and Check for Updates | Window menu; commit 8 |
| Menu keyboard, hover, type-ahead, focus return, placement | Menu behaviour; commit 7 |
| Tab menu items, disabled states, vertical label, outline, no activation | Tab context menu; commit 9 |
| About panel details and buttons | About panel; commit 10 |
| Title strip showing the active tab's title | Title bar; commit 11 |
| Window-manager title "cargo build — Huterm" | Title bar; commit 11 |
| Yellow dot on busy tabs | Title bar; commit 11 |
| Tabs in the macOS and Linux title bars, window controls, drag space | Title bar; commits 12 and 13 |
| Unchanged palette opened from the menu | Outcome and scope; commit 8 |
