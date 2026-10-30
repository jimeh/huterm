# Modifier pane drags

Status: in progress

## Objective

While the user holds a configured modifier combination, a press anywhere in
a pane's terminal starts the same pane drag a title bar press starts. This
serves people who hide pane title bars, and anyone who wants a larger grab
target. The pointer shows an open hand while the combination is held over a
pane and a closed hand while the press is held. Every visible pane shows its
title in a compact badge centred in the pane, so the user knows what they
are about to move.

## Constraints

- Off by default (Jim's decision). Key: `[panes] drag_modifier`; absent
  means off.
- Keybinding token syntax, lowercase: `cmd` (or `super`, the spelling the
  Linux keybinding defaults use), `ctrl`, `alt`, `shift`, joined with
  hyphens, in any order, no duplicates. `fn`, `win`, `secondary`, and other
  spellings are rejected.
- Builds on the terminal-surface PR (#343, `221df8b2`):
  - The compact badge is `overlay::badge(swatch, text)` in `huterm-gpui`.
  - The terminal sets an I-beam or arrow cursor through its hitbox. The
    revealed hover pane bar, toasts, and the palette scrim set the arrow
    explicitly. `WorkspaceView::drag_cursor` paints one window cursor while
    one of Huterm's own drags runs. The open hand comes from the grab layer
    drawn above the terminal, and the closed hand goes through
    `drag_cursor`.
  - #343 added a root-div `on_modifiers_changed` listener that calls
    `refresh_hovered_cursor`. That one listener also records `held` and
    defers the pointer refresh. The dead canvas listener is removed, so the
    window keeps a single modifier listener.
- A parallel thread splits Settings' "Window and panes" into Windows and
  Panes sections. The new row goes in whichever Panes group exists at rebase
  time.
- `docs/agents/desktop-input.md` rules hold: divider zones keep their
  occluding grab, overlays drawn later keep their presses, and a pane drag
  keeps terminal focus.

## Facts from the code that shape the design

1. **macOS GPUI remaps every Control-left press.** `gpui-pre-macos`
   `window.rs:2859-2911` turns any Left press or release with Control into
   Right with Control cleared, whatever else is held, and
   `Window::dispatch_event` then overwrites `Window::modifiers()` from the
   remapped event. Motion arrives as a Left-pressed move that still carries
   Control. So `cmd-ctrl` reaches Huterm as a Right press carrying `cmd`.
   Today a Cmd+Control-click opens the terminal's context menu.
2. **The window's modifier listener is dead.** GPUI dispatches
   `ModifiersChanged` only along the focused element's dispatch path
   (`window.rs:6070`, `6351-6368`). The `window.on_modifiers_changed` call
   in the window's childless canvas (`windows.rs:11337`) is never on that
   path, so modifier changes do not refresh pointer state today. A listener
   on the `WorkspaceView` root div, an ancestor of every focusable element
   in the window, does receive them.
3. **Pointer modifiers Huterm uses today.** The link chord
   (`terminal.link_modifiers`, exact match, `cmd` on macOS and `ctrl` on
   Linux, plus Shift while an application reports the mouse). Shift, in any
   combination, bypasses application mouse reporting
   (`desktop.rs:1834`). Shift+right-click opens the menu while an
   application has the mouse. On macOS any Control-left is a right-click.
   There is no Option or Alt pointer gesture.
4. **Single-pane tabs have no title bar**, so a lone pane cannot be dragged
   today. A zoomed pane of a split tab keeps its bar and drags.
5. **Pane drags have no new-window drop.** Targets are a pane's edge or
   centre in the active tab, a tab bar slot (new tab), and an inactive
   label's spring zone. A release without a target cancels.
6. **Hover bars already sample the pointer.** `refresh_tab_visibility`
   computes `pane_bar_pointer`: the pointer while the window is hovered and
   no terminal gesture, drag, menu, native menu, focused overlay, or
   fullscreen tab bar claims it. Window tests feed it directly because the
   test platform never reports a hovered window. Licenses is not in
   `overlay_holds_focus`.
7. **Bubble order gives overlays priority.** GPUI runs bubble mouse
   listeners topmost first. Title bars, divider zones, toasts, and the
   palette are drawn after a pane's terminal, so a bubble listener drawn just
   above the terminal sees a press only after they decline it.

## Approach

### Configuration (`huterm-config`, `huterm-gpui/src/config.rs`)

- Add `PaneDragModifiers(u8)` with `parse(value, macos) -> Result<Self,
  String>` and an exact `matches_bits`. Share the token-to-bit table with
  `LinkModifiers`; `LinkModifiers` keeps its tokens and messages unchanged.
  `PanesConfig` gains `drag_modifier: Option<PaneDragModifiers>`.
- `parse` rejects, each with a message naming the gesture it would take:
  - unknown tokens, `fn`, duplicates, and an empty value;
  - any single modifier: `cmd` (opens links on macOS), `ctrl` (opens links
    on Linux, right-click on macOS), `alt` (Option or Alt pointer
    conventions such as rectangular selection, kept free), `shift`
    (bypasses mouse reporting);
  - on macOS, `ctrl-alt` (VoiceOver's keys) and `ctrl-shift` (the one-button
    Shift+right-click that opens the menu inside vim or tmux).
- Cross-field check in `RawConfig::validate_values`, which load and the
  Settings write validator both run: reject a `drag_modifier` equal to
  `terminal.link_modifiers`, or to it plus `shift`. The message names both
  keys and the value. An invalid reload keeps the running settings, as for
  any invalid config.
- Schema describes the value with a token pattern, since an enum cannot
  hold both spellings of `cmd` in every order. Its description gives the
  recommendations and says the duplicate, platform, and link checks run at
  load.
- `x-huterm-apply = "live"`.

### Held modifiers and arming (`windows/pane_grab.rs`, new)

`PaneGrab` is a pure state the window drives, like `PaneBarReveals`, with
caller-supplied instants.

- **`held`.** Recorded by the `WorkspaceView` root div's existing
  `.on_modifiers_changed` listener from #343, which also calls
  `defer_pointer_refresh`. The dead canvas listener goes. `held` is also
  refreshed from the modifiers of pointer events GPUI does not remap
  (`MouseMove`, `Scroll`, `MouseExited` in the window's capture listeners),
  reset from `window.modifiers()` on activation, and cleared when a native
  menu session ends. The macOS remap never writes it, so it keeps Control
  through a remapped press. The extra sources correct a stale value after an
  AppKit menu loop swallowed `flagsChanged` or X11 dropped a state event.
- **Armed** when `held` matches `drag_modifier`, the window is active,
  `pane_bar_pointer` is inside the terminal rect of a drawn pane of the
  active tab, that tab is split (zoomed included), and Licenses is not
  open.
- **Badges** show while armed once the combination has been held for
  `GRAB_BADGE_DELAY` (150 ms). A non-modifier key press, or a window
  activation, while the combination is held hides them until `held`
  changes. This matters because the default macOS pane bindings use
  `cmd-ctrl` plus a key (resize, layouts, balance) and the Linux ones use
  `ctrl-alt` and `ctrl-super`. The window's animation schedule carries
  the deadline.
- Arming recomputes on every pointer refresh, so deactivation, a tab
  switch, zoom, Quake hiding, a config reload, and a drag starting all
  disarm through the existing sample.

### Press (`pane_elements`, `pane_drag.rs`)

While `drag_modifier` is set, each drawn pane of a split active tab gets a
**grab layer** over its terminal rect, drawn after the terminal and its dim
layer. The layer is always present, not only while armed, so a press never
depends on the arming frame. It has no `occlude`, so hover, wheel, and
other presses reach the terminal beneath.

Its bubble `on_mouse_down` (Left, and Right on macOS) classifies the press
with a pure `grab_press(button, event_modifiers, held, config, macos)`:

- a Left press whose modifiers equal the combination;
- on macOS, when the combination contains `ctrl`, a Right press whose
  modifiers equal the combination without `ctrl` while `held` has Control
  (the remapped Control-left).

It also requires an active window and the same unclaimed state as arming:
no menu, native menu, focused overlay, Licenses, close confirmation, or
other drag. On a match it focuses the pane, calls `begin_pane_drag`, and
stops propagation, so the terminal sees no press: no mouse report,
selection, link press, or context menu. The press is consumed even when
`begin_pane_drag` refuses (busy, arrangement in flight, quitting), as the
title bar does.

Overlays drawn later handle their own presses first (fact 7): a revealed
hover bar's title drags and its buttons click, a toast's buttons act, and
dividers resize.

`PaneDrag` gains `remapped: bool`. The window's release capture listener
ends a remapped drag on a Right or Left release, since Control may be let go
before the button. Title bar drags keep their Left-only release. Everything
else is the existing pane drag: threshold, targets, refusals, spring-loaded
tabs, Escape, blur, and `commit_pane_move`. Releasing the modifiers
mid-drag changes nothing.

### Drawing

- While armed, the grab layer adds the open-hand cursor. GPUI uses the
  cursor of the topmost hovered hitbox, so it wins over the terminal's
  cursor, and divider zones and later overlays win over it.
- While badges show, each grab layer centres a compact badge with
  `pane_label`, the title bar's and drag chip's text, which includes a
  custom pane name. The badge ellipsizes to the pane's width.
- While any pane drag holds its press (`pane_drag.is_some()`),
  `drag_cursor` returns `ClosedHand` instead of the arrow it returns for a
  dragging pane drag today. This covers title bar drags too, so a drag
  looks the same however it started. Tab reorders keep the arrow.
  Badges and the open hand are not drawn during a drag; the existing chip
  follows the pointer.
- Smoke state records `armed` and the badge count next to `drawn_drop`.

### Settings

A row in the Panes group, "Drag panes with", as a select, not the link
modifier chips: chips cannot step from off to `cmd-ctrl` without passing
through rejected single modifiers. Options are "Off", which removes the
key, and every combination this platform accepts that does not collide
with the current `terminal.link_modifiers`, labelled in Apple's order on
macOS ("⌃⌘ Control-Command") and with names on Linux ("Ctrl+Alt").
Settings writes `cmd` on macOS and `super` on Linux. A file value outside
the list shows as invalid, as other rows do. This needs a
`Special::DragModifiers` that builds its choices at render.

When a link-chip toggle would collide with `drag_modifier`, the existing
write validator refuses it, and the chip row shows the validator's message.
Implementation confirms that path and adds a description if the message is
unclear.

### Documentation

- `docs/configuration.md` `[panes]`: the key; off by default; `cmd-ctrl`
  recommended on macOS and `ctrl-alt` on Linux, where Super combinations
  can trigger the desktop's window move; the rejected combinations and why;
  that the chosen combination's click no longer reaches the terminal, so
  `cmd-ctrl` replaces the Cmd+Control-click context menu on macOS; single
  pane tabs; inactive windows; and that a desktop's own window-move
  modifier, such as Super on GNOME, never reaches Huterm.
- `docs/configuration.md` "Mouse, selection, and scrolling" and `README.md`'s
  pane paragraph: one sentence each, linking the key. `docs/keybindings.md`
  has no pointer gesture section and is unchanged.
- `docs/agents/desktop-input.md`: a short "Modifier pane drags" section with
  the remap, the `held` record and its sources, the root-div modifier
  listener, and the grab layer's bubble ordering.

## Behaviour summary

| Situation | Result |
| --- | --- |
| Combination held, pointer over a pane of a split tab, window active | Open hand at once; badges after 150 ms |
| Grab press, then motion past the threshold | Pane drag with the usual targets; closed hand from the press |
| Grab press without motion | Focuses the pane only |
| Modifiers released before the press | Badges hide, cursor restores |
| Modifiers released mid-drag | Drag continues |
| Window loses focus while held | Disarms; a drag cancels as today |
| Menu, dialog, palette, Licenses, or another drag open | Not armed; presses behave as today |
| Combination press on a hover bar, toast, or divider | That element handles it as today |
| Combination press on a pane's scroll pill | Starts a pane drag; the pill sits inside the terminal, below the grab layer |
| Single-pane tab | Not armed; the press reaches the terminal (on macOS `cmd-ctrl` still opens the context menu) |
| Inactive window | Not armed; the first press reaches the terminal as today |
| Key pressed, or window activated, while held | Badges hidden until the modifiers change |

## Non-goals

- Dragging a lone pane or a tab by the modifier.
- A drop that opens a new window; pane drags have none today.
- Rectangular selection or other Alt gestures.
- Suppressing link hover from GPUI's overwritten `window.modifiers()`
  after a remapped press. With `cmd` links and a still pointer, a link can
  underline until the next move. Recorded as a known limit.
- Changing title bar drags beyond the closed-hand cursor.

## Failure paths and risks

- **Stale arming frame.** The grab layer is always present and classifies
  the event, so a lagging frame affects only the cursor and badges.
- **Stale `held`.** Pointer-event refresh, activation reset, and native menu
  clearing bound it. Without them a stale Control would turn a physical
  Cmd+right-click into a drag.
- **Remapped release.** `remapped` accepts Right or Left. An unmatched
  release reaching a terminal that holds nothing is ignored there, as today.
- **System interception.** macOS's opt-in `NSWindowShouldDragOnGesture`
  default (to verify) and Linux window managers' move modifiers take the
  press before Huterm. Documented, not detected.
- **Link chord changed later.** The cross-field check runs on every load
  and Settings write.

## Verification

| Concern | Evidence |
| --- | --- |
| Tokens, the `super` spelling, rejected spellings, single-modifier, VoiceOver, and macOS `ctrl-shift` rejection, duplicates, platform split | `huterm-config` unit tests on `PaneDragModifiers::parse` for macOS and Linux |
| Link-chord collision, plain and with Shift, with a custom chord | Config test through `parse_at` and the Settings write validator |
| `LinkModifiers` unchanged | Existing link config tests stay green |
| `held` sources, arming, badge delay, key and activation suppression, release mid-drag, deactivation, native menu clearing | `pane_grab.rs` unit tests |
| Press classification, including the macOS remap and a stale-`held` Cmd+right-click | `grab_press` unit tests |
| `simulate_modifiers_change` alone arms the window | Window test (fails today: proves the root-div listener) |
| With no focused element, where no div sees `ModifiersChanged`, the next pointer move corrects `held` | Window test |
| A grab press starts a pane drag and a drop commits a move; the terminal sees no report, selection, or link press | Window test with a split tab and a reporting terminal, `pane_bar_pointer` fed as the hover tests do |
| The remapped sequence: `ModifiersChanged{cmd,ctrl}`, Right press with `cmd`, Left-pressed moves, Right release | Window test asserting the drop and that the terminal saw neither press nor release |
| Unarmed, wrong-modifier, lone-pane, and inactive-window presses reach the terminal | Window tests |
| A grab press on a hover bar button and on a toast reaches that element | Window tests |
| Badges carry pane labels, including a custom name | Window test reading smoke state |
| Real remap, release, and drop on macOS | Extend `smoke:macos-input`: add Cmd (55) and Control (59) `flagsChanged` fixtures, post them after activation, post Left mouse events with both flags, drag to another pane's edge, assert the layout changed and the terminal saw no press |
| Cursors, badge legibility, themes, hidden and hover bars | `ux-review` with Codex computer use on a scratch bundle and config |
| Settings choices and write-back | Settings test on the choice list and the written value |
| Whole repository | `mise run check`, focused `cargo test`, `mise run verify` before handback |

Linux has no remap, so window tests on the Linux test platform cover its
press path; `smoke:linux-input` is not extended.

## Decisions

Jim settled the open questions on 2026-10-30, each as recommended.

1. **Single-pane tabs are not armed.** A lone pane has no title bar, so
   there is nothing to match. A press on it reaches the terminal as today,
   including the Cmd+Control-click context menu on macOS.
2. **`cmd-ctrl` is allowed on macOS** through the remapped Right press,
   with `smoke:macos-input` coverage. The docs recommend `cmd-ctrl` on
   macOS and `ctrl-alt` on Linux.
3. **Every pane drag shows the closed hand**, title bar drags included.
