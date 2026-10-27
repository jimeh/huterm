# UI components

Status: proposed. Follows [the settings plan](settings-ui.md), whose PR 3
built a control kit inside the Settings window, and folds in the open
Settings follow-ups [#276][issue-276] and [#277][issue-277]. Codex and
Claude reviewed it against the source in four rounds; the last found no
significant issues. The running-app checks are still owed (see Defects).

The Settings series (#269, #274, #275) built its controls one at a time,
inside the window that uses them. Each works on its happy path, but the kit
has no shared contract for dismissal, focus, disabled states, or
accessibility, and the same pattern is implemented twice with different
behavior. The visible result: a Settings dropdown closes only when one of its
items is clicked.

This plan proposes a small in-house component layer for Huterm's GPUI client,
in the spirit of shadcn/ui: a dozen components, one per file, each with a
written behavior contract and GPUI tests for that contract. Settings, and
later the terminal window's overlays, are built from them. Each component
can then be perfected on its own against the native controls on macOS and
GNOME and against Radix and the WAI-ARIA Authoring Practices.

## Current state

Observed on `main` at `f0b0174c`. GPUI is `gpui-pre` 0.3.9.

### Inventory

"Shared" means more than one caller uses the implementation. Every
interactive element below has no accessibility role or label: no file in
`crates/huterm-gpui` calls GPUI's `role` or any `aria_*` method. Hover means
a background change unless the row says otherwise; scrolling applies only to
the menu, list, and Settings pane rows.

The tab strip, title-row buttons, pane dividers and drags, palette, toasts,
Licenses view, and scroll pill have contracts of their own that this plan
does not migrate; they adopt tokens, `Button`, and `Popover` only where that
removes duplication.

| Kind | Where | Shared | Keyboard | Pointer | Focus ring | Dismissal | Disabled or read-only |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Button (Settings) | `settings_window.rs` `button`, `render.rs` `actionable` | Settings | Space, Return (`settings_toggle`), plus GPUI's implicit Enter and Space click (D16) | Hover and pressed styles | `.focus()` ring, also on click | n/a | Opacity 0.5 |
| Dialog buttons | `close_dialog.rs`, `about.rs` | Terminal window | `dialog_*` commands under `confirming`: Return, Escape, Tab, Shift-Tab, Left, Right | Hover, pressed | Accent fill on the focused button | n/a | n/a |
| Tab strip | `tab_strip.rs`, `tab_bar.rs` | Terminal window | Catalog tab commands; tabs are not tab stops | Click activates, drag reorders with edge auto-scroll, the close button closes, right press opens the tab menu | None | n/a | n/a |
| Title-row and tab-bar buttons (menu, new tab) | `windows.rs` (the menu button, `new-tab`, `window_controls_element`) | Terminal window | `open_menu` and `new_tab` commands; not tab stops | `TabColors` hover and pressed, tooltips | None | Menu button toggles its menu | n/a |
| Pane dividers and pane drags | `pane_geometry.rs`, `pane_drag.rs`, `windows.rs` | Terminal window | Resize and move commands | Divider drag resizes; pane-title drag moves a pane | None | Window deactivation cancels | n/a |
| Link | `settings_window.rs` `link` | Settings | Space, Return | Accent text | Ring | n/a | None |
| Switch | `controls.rs` `render_switch` | Settings | Space, Return toggle; Up and Right on, Down and Left off | Click toggles | Ring, also on click | n/a | Opacity, no tab stop, no reason |
| Tri-state (Default, On, Off) | `controls.rs` `render_optional_bool` | Settings | One tab stop; arrows step | Click a segment | Ring on group | n/a | Opacity |
| Segmented control | `controls.rs` `render_segments` | Settings | One tab stop; arrows step and write | Click a segment | Ring on group | n/a | Opacity |
| Select | `controls.rs` `render_select`, `with_menu` | Settings, Quake, keybinding arguments | Space, Return open; arrows step and write without opening | Click opens; a second click opens a new menu | Ring | Escape, Tab, a pick; not Shift-Tab, an outside press, a trigger press, or deactivation | Opacity |
| Menu (listbox) | `desktop/menu.rs` `Menu`, `MenuModel` | Window menu, tab and terminal context menus, every select | Arrows, Home, End, Return, Space, Escape, Tab, type-ahead | Moving pointer selects; click picks | n/a | Emits `Dismissed`; each host closes it | Disabled rows with a hint |
| Menu host | `windows.rs` (`toggle_menu`, capture-phase press), `settings_window.rs` (`show_menu`, `close_menu`) | Two hosts | n/a | Terminal window: outside press closes, button toggles. Settings: neither | n/a | See defects D1 to D4 | n/a |
| Font picker | `controls.rs` `render_font` | Settings | Field plus an "Installed" select | Menu of every family | Ring | As select | Opacity |
| Text field | `ui/text_field.rs` `TextField` | Palette, Licenses, Settings | Catalog text commands under `Palette`; IME | Click, drag, double-click word | None (caret only) | Escape restores in Settings | None in the field itself |
| Number field | `render_field` plus `stepped` in `settings_window.rs` | Settings, Quake | Up and Down add or subtract 1 | None | None | n/a | Opacity |
| Size field | `render_field` (`Special::Size`) | Settings | Text | None | None | n/a | Opacity |
| Condition completion (combobox) | `keybindings.rs` `render_completion` | Keybindings | Up, Down, Return, Tab, Escape | Hover highlight; mouse-down accepts | n/a | Tab, Escape, blur | n/a |
| Key recorder | `keybindings.rs` `record`, window keystroke interceptor | Keybindings, global shortcuts | Escape cancels, Return saves, chord timeout | Click starts | Ring | Blur, deactivation, outside click cancel | Opacity |
| Modifier chips (toggle buttons) | `controls.rs` `render_modifiers` | Settings | One tab stop per chip, Space toggles | Click | Ring | n/a | Opacity, last chip refuses |
| Theme cards (radio cards) | `controls.rs` `render_themes` | Settings | One tab stop per card | Click | `.focus()` style set, but #276 reports none visible | n/a | Opacity |
| Sidebar (navigation list) | `render.rs` `render_nav` | Settings | Tab stops; no arrows (#276) | Click | Ring | n/a | n/a |
| Inline confirm | `quake.rs` `render_delete`, `escape_delete` | Quake | Escape matched literally in the keystroke interceptor, only while a prompt button has focus (D17) | Buttons | Ring | Cancel, Escape | n/a |
| Modal dialog | `close_dialog.rs`, About in `about.rs` | Terminal window | `dialog_*` and `confirming` commands | Buttons, scrim | Accent buttons | Escape, Cancel | n/a |
| Command palette | `palette/` | Terminal window | Full catalog grammar | Hover, click | n/a | Escape, outside press | n/a |
| Licenses view | `licenses.rs` | About panel | Search field and list under `Palette`; arrows select, PageUp and PageDown scroll the text, Tab and Shift-Tab move between the search and the buttons, Enter presses the focused button | Click selects, wheel scrolls, list scrollbar | Rings on the search field and buttons | Escape clears the search, then returns to About | n/a |
| Scroll pill | `desktop.rs` `render_scroll_pill` | Terminal window | Scroll-to-bottom command | Click scrolls to bottom; stops presses, never releases | n/a | Hides at the bottom | n/a |
| Toasts | `notices.rs` | Terminal window | `notice_*` commands | Hover pauses | n/a | Escape, Delete, timeout | n/a |
| Tooltip | `overlay.rs` `TextTooltip` | Menu buttons, tab bar | None | GPUI hover delay | n/a | Pointer leaves | n/a |
| Scrollbars | `ui/scrollbar.rs` (terminal), `ui/list_scrollbar.rs` (lists) | Shared | n/a | Track press, drag, hover widen | n/a | n/a | n/a |
| Key cap | `key_hint.rs` `KeyHint`, `overlay.rs` `key_cap` | Shared | n/a | n/a | n/a | n/a | n/a |
| Form row and card | `render.rs` `render_row`, `keybindings.rs` `card`, `group_block` | Settings | n/a | Reset button | n/a | n/a | Read-only note for other platforms |

What already works and should be kept: `MenuModel` selection and
`place_menu` placement are pure and tested; a select opens on its current
value; only a moving pointer selects a row (the GPUI hover hazard in
[the desktop client guide](../agents/desktop-client.md)); menus have
type-ahead and an overflow indicator; `TextField` handles IME composition and
grapheme editing; nearly every key is a catalog command under a key context;
and the Settings window recovers focus left on a control that disappeared.
The exceptions to the catalog rule are the key recorder's interceptor, which
must see raw keys by design, the Quake delete prompt's Escape (D17), and
GPUI's implicit Enter and Space click (D16).

### Defects and quirks

Evidence is from source on `f0b0174c` unless marked. The macOS session was
locked during this investigation (`CGSSessionScreenIsLocked=Yes`), which stops
frames, so the Settings smoke timed out at startup twice and the running-app
review could not run. [The smokes guide](../agents/desktop-smokes.md)
documents this failure mode. Items marked "runtime check pending" are inferred
from code and need the running-app review in PR 1. The 139 existing Settings
and menu GPUI tests pass; none sends a pointer event to a Settings menu.

| # | Defect | Evidence | Severity |
| --- | --- | --- | --- |
| D1 | A press outside a Settings menu does not close it. | `with_menu` (`controls.rs:269-292`) mounts the menu with `deferred(anchored())` and no outside handler; `Menu` stops only presses inside itself. The terminal window closes its menu from a capture-phase press listener (`windows.rs:10549`). Settings has no equivalent. | High |
| D2 | Pressing a select's trigger while its menu is open opens a fresh menu instead of closing it. | The trigger's `on_click` calls `open_select` (`controls.rs:258-263`), and `show_menu` (`settings_window.rs:835`) always builds a new `Menu`. The terminal window's `toggle_menu` (`windows.rs:11835`) closes first. | High |
| D3 | A menu taller than the window, such as Installed fonts, cannot scroll; rows past the window edge are unreachable by pointer (keyboard selection and type-ahead still pick them, unseen), and the menu slides up over its trigger. | Settings never calls `Menu::set_max_height` (`menu.rs:551`); `snap_to_window_with_margin` only shifts the origin; the menu is offset by a fixed `mt(px(28.0))` and never opens upward. Runtime check pending. | High |
| D4 | No menu closes when its window deactivates. | Settings' activation observer only cancels recording (`settings_window.rs:345`); the terminal window's cancels drags (`windows.rs:2347`). AppKit menus and GTK popovers close on deactivation. | Medium |
| D5 | After D1, the clicked control acts and takes focus while the stale menu stays drawn; its keys are dead because `menu` bindings match only on the focus path, and a later close returns focus to the original trigger. | Inference from D1, `close_menu` (`settings_window.rs:902`), and the key-path rule in the desktop client guide. Runtime check pending. | Medium |
| D6 | The condition completion is a second, different listbox: it shows separate hover and keyboard highlights, caps at eight matches with no scrolling, accepts on mouse-down, and lays out in the row's flow, so it pushes the row down. | `keybindings.rs:2242-2296` (`.hover` at 2275, `take(8)` at 2264); placed in a column under the field at 2092-2099. | Medium |
| D7 | Setting and Quake number stepping ignores range and scale: Up and Down add 1 and do not clamp, so a Quake `width` or `height` (greater than 0, at most 1) always steps out of range into an error. Keybinding arguments already clamp. | `stepped` (`settings_window.rs:1264-1297`); schema `exclusiveMinimum: 0, maximum: 1`; argument stepping clamps at `keybindings.rs:1404-1409`. No PageUp, PageDown, Home, End, or stepper buttons. | Medium |
| D8 | Arrow keys on a select or a switch write a new value without opening or confirming. Every arrow press while moving through the form saves. | `settings_defaults` binds the arrows to `settings_increase` and `settings_decrease` (`keymap.rs:973`); `stepped` steps a `Choice` and sets a `Bool`. Radix Select, AppKit pop-up buttons, and GTK drop-downs open the list on Down or Up. | Medium |
| D9 | The focus ring also shows after a pointer click. | Every control uses `.focus()` (`controls.rs:129, 417, 467, 573, 614`; `render.rs:37, 119`). GPUI 0.3.9 has `focus_visible`, but it rechecks the input mode every frame, so it alone is not the fix (see the API conventions). | Low |
| D10 | Read-only is opacity only. A read-only text field still takes focus from a click and accepts typing, which is then dropped; no disabled control says why. | `render_field` sets `opacity(0.5)` and clears the tab stop only; `TextField` has no read-only state; the write is refused in `saving.rs:416`. Runtime check pending. | Medium |
| D11 | Nothing is exposed to VoiceOver or Orca. | No `role` or `aria_*` calls; GPUI 0.3.9 already builds an AccessKit tree, and `accesskit_macos` and `accesskit_unix` are in `Cargo.lock`. | High |
| D12 | `TextField` never scrolls horizontally, and has no Cut or Undo. Long text in a fixed-width Settings field (90, 120, or 180 points) runs past it, and the caret can leave view. | No offset in `text_field.rs` render (552-660); `run` (433-462) and the protocol have no cut or undo commands. Overflow is a runtime check pending. A future field Undo needs its own command id: #256 adds a generic Window-scope `undo` on `cmd-z` (`layout-undo.md`). | Medium |
| D13 | The two menu hosts duplicate anchoring, outside-press handling, toggling, and focus return, and Settings implements half of it. | `windows.rs` (`menu_bounds`, `menu_button_bounds`, `toggle_menu`, `render_menu`) versus `settings_window.rs` (`show_menu`, `close_menu`) and `controls.rs` (`with_menu`). This is the root cause of D1 to D4. | Structural |
| D14 | `Swatch::focus_ring` carries two doc comments, the first stale. | `overlay.rs:155-156` | Trivial |
| D15 | Shift-Tab in an open Settings menu moves focus behind it and leaves the menu open, the stale state of D5, reached from the keyboard alone. Smoke state hides it: it reports `focus=menu` whenever a menu exists. | `menu_defaults` binds `tab` but not `shift-tab` (`keymap.rs:950-951`), so `settings_focus_previous` runs `window.focus_prev` (`settings_window.rs:946`); `smoke_state` at `settings_window.rs:1182`. | Medium |
| D16 | Removing or rebinding the Enter or Space binding of a Settings control does not stop those keys from activating it. | GPUI turns Enter or Space on a focused element with `on_click` into a `ClickEvent::Keyboard` (`gpui-pre-0.3.9/src/elements/div.rs:3016-3045`); Settings click handlers ignore the event kind (for example `controls.rs:139`). | Medium |
| D17 | The Quake delete prompt's Escape cannot be rebound and is missing from the keybinding editor. | `escape_delete` matches the literal key in the window's keystroke interceptor (`quake.rs:378-395`, wired at `settings_window.rs:282-291`). | Low |

From [#276][issue-276], by owning component: Tab does not scroll the focused
control into view (scroll area); the sidebar has no arrow keys (navigation
list); theme cards show no focus despite the style at `controls.rs:467`,
cause not yet diagnosed (radio cards and the focus ring token); a held key
records a repeat and recording drops a shortcut's drafts (key recorder). The
"Reloaded" footer after removing an `unbind` is Settings status logic, not a
component.

From [#277][issue-277]: renaming a profile needs an inline rename (inline
edit with confirm), and the delete prompt becomes the inline confirm
component. Quoted profile names, keybindings that name a deleted profile, and
the global shortcut check are not component work.

Linux uses the same code paths. Its evidence here is code reading and the
`linux-settings` CI smoke, which covers keyboard paths only.

## Reference behavior

Each component contract cites one primary reference and notes where the
platforms differ:

- [Radix Primitives][radix] keyboard tables and its dismissable layer, and
  the [WAI-ARIA Authoring Practices][apg] patterns (button, switch, radio
  group, listbox, select-only combobox, combobox, menu button, spinbutton,
  dialog, alert dialog, tooltip).
- macOS: AppKit `NSPopUpButton`, `NSSwitch`, `NSSegmentedControl`,
  `NSTextField` with `NSStepper`, and the [Human Interface
  Guidelines][hig].
- GNOME: GTK 4 `GtkDropDown` and `GtkPopover`, libadwaita `AdwComboRow`,
  `AdwSwitchRow`, `AdwSpinRow`, and `AdwEntryRow`, and the [GNOME
  HIG][gnome-hig].

For example, all three agree that an open select or menu closes on Escape,
on a press outside it, on a second press of its trigger, and when its window
deactivates, and that focus returns to the trigger. They also agree that Down
or Up on a closed select opens it. Huterm's Settings select does none of
these except Escape.

## Options

### (a) In-house component layer

A `crates/huterm-gpui/src/ui/components/` module: one component per file,
each opening with its behavior contract; a shared popover primitive for
anchoring, layering, dismissal, and focus return; theme tokens and a focus
ring; and GPUI tests for each contract.

- For: it keeps Huterm's rules intact. Every key stays a catalog command the
  user can rebind; colors derive from the terminal theme; the GPUI hazards
  already recorded (resting-pointer hover, focus listeners inside a draw,
  presses against a stale frame) are handled once, inside each component.
  It adds no dependency to the GPUI upgrade path, and most components
  already exist and need extracting and finishing, not writing from scratch.
- Against: Huterm owns every behavior and every bug. Components Huterm lacks
  (slider, virtualized list, date picker) cost real work if they are ever
  needed.

### (b) Depend on `gpui-component`

[`gpui-component`][gpui-component] (Longbridge, Apache-2.0, with its
`gpui-base` and `gpui-kit-assets` siblings also Apache-2.0) is the most
complete GPUI library: about 108,000 lines covering select, popover, switch,
slider, settings pages, dock, tables, and an editor. Its maintainer also
publishes `gpui-pre`, it ships weekly, and 0.8.0 pins `gpui-pre =0.3.9`,
Huterm's exact version. Its declared licenses are allowed by Huterm's
policy; a full `mise run license` run over its dependency tree was not
done.

It does not fit Huterm's rules:

- **Key bindings.** Its `init` registers 57 hardcoded `KeyBinding`s
  across 12 `cx.bind_keys` calls (more may come from `gpui_base::init`, which
  was not read). Huterm requires every UI key to be a catalog command in the
  user's keymap, reserves terminal keys from the compiled keymap, and lists
  every binding in the keybinding editor. Bypassing that breaks rebinding,
  the editor, and reserved-key derivation.
- **Upgrade lockstep.** Each release pins one exact `gpui-pre` (0.8.1, from
  2026-10-26, already requires 0.3.10). Huterm vendors and patches
  `gpui-pre` (`third-party/vendor/`) and upgrades it deliberately, as
  [the dependencies guide](../agents/dependencies.md) describes. Every GPUI
  upgrade would then wait for a matching component release, and every
  component fix would require a GPUI upgrade.
- **Weight.** It has non-optional dependencies Huterm does not have:
  lsp-types, markdown, ropey, and rust-i18n (notify, schemars, chrono, and
  uuid are already in Huterm's lockfile). They add compile time and
  license-audit surface.
- **Theming and structure.** It reads its own global `Theme` and needs its
  `Root` view around each window for dialogs and notifications. Huterm
  derives colors from the terminal theme on every reload and has its own
  overlay, scrim, and focus-return rules.
- **Accessibility.** Its `Select` sets no roles; 29 of its files use the
  AccessKit API at all.

### (c) Hybrid

Build (a), and use `gpui-component` as a design reference. Because it is
Apache-2.0, Huterm may adapt a specific piece of its code, such as anchoring
math, with attribution in the license notices. Huterm takes no dependency on
it. Zed's `ui` crate is GPL-covered and stays out entirely: its public docs
may inform behavior, never code.

## Recommendation

Choose (c): build an in-house component layer and treat `gpui-component` as a
reference that Huterm may adapt with attribution, never a dependency. The
deciding facts are the binding model and upgrade lockstep in (b). Either one
alone would cost more than writing the dozen components Huterm needs, most of
which already exist and need a contract, a shared popover, and tests.

Keep the layer as a module in `huterm-gpui`, not a new crate. The text client
will not use GPUI, so a crate boundary buys nothing today. A
`mise run architecture` check that `ui/` never imports `crate::desktop` keeps
the boundary honest at the cost of one `rg` rule. It lands in PR 5, when
`Menu` and `MenuModel` move into `ui/`. PR 1 moves the pieces `Popover` and
tokens need into `ui/`: `place_menu` becomes `ui::components::popover`'s
placement, and `Swatch` and `OverlayColors` move to `ui/theme.rs`, with
`desktop::overlay` re-exporting them so callers do not change.
`OverlayColors::from_theme` calls `desktop::scrollbar_colors`
(`desktop.rs:3517`), so that moves too, as `ScrollbarColors::from_theme` in
`ui/scrollbar.rs`. Until PR 5,
only `ui/components/select.rs` may use `desktop::menu`; that rule is review
discipline, not a check.

## Component set

| Component | Built from | Replaces |
| --- | --- | --- |
| Tokens, focus ring | `Swatch` | Per-call styling |
| `Popover` (with dismissal and focus return) | `place_menu`, `deferred`, `on_mouse_down_out` | Both menu hosts, `with_menu` |
| `Listbox` (model, selection, type-ahead, scroll) | `MenuModel`, `ListScrollbar` | `Menu` internals, completion list |
| `Menu` | `Listbox` in a `Popover` | `desktop/menu.rs` view |
| `Select` | Trigger plus `Menu` | `render_select`, Quake and argument selects, font picker |
| `Combobox` | `TextField` plus `Listbox` in a non-modal `Popover` | Condition completion |
| `Button` (default, accent, danger, ghost, link, icon) | | `button`, `link`, `actionable`; later dialog buttons |
| `Switch` | | `render_switch` |
| `ToggleChip` | | Modifier chips |
| `SegmentedControl` (radio group) | | Segments, tri-state |
| `RadioCards` | | Theme cards |
| `TextField` | Existing | Hardened in place |
| `NumberField` | `TextField` | `render_field` number path, `stepped` |
| `KeyRecorder` | Keystroke interceptor | Recorder in `keybindings.rs` |
| `NavList` | | Settings sidebar |
| `InlineConfirm`, `InlineEdit` | `Button`, `TextField` | Quake delete prompt, New profile, later rename |
| `ScrollArea` | `ListScrollbar` | Settings pane scrolling, focus-into-view |
| `Tooltip` | `TextTooltip` on GPUI's tooltip layer, not `Popover` | Tooltip calls |
| `FormRow`, `Card` | | `render_row`, `card`, `group_block` |

## Component API conventions

- **One file per component** under `ui/components/`, opening with its
  contract: purpose, anatomy, keyboard table, pointer behavior, focus,
  dismissal where it applies, disabled and read-only states, accessibility
  role and properties, and the reference it follows. Tests live after the
  production items, or in an adjacent file when they dominate.
- **Controlled by default.** A control receives its value and reports
  changes; the owner holds the truth. Settings values come from the file and
  a pending queue, so components never cache them. Builders are `RenderOnce`
  values: `Switch::new(id, label).on(value).disabled(reason).on_change(f)`.
- **Entities only for transient state.** `TextField`, `Listbox` (selection,
  scroll, type-ahead), `Popover` (open state, anchor bounds), and
  `KeyRecorder` are entities. Everything else is a stateless builder.
- **Stable identity.** Every component takes an `ElementId`. GPUI derives
  accessibility node identity and element state from it, so it must survive
  re-renders and be unique within the window.
- **A label is required.** Constructors take an accessible name even when
  the visible label sits in a `FormRow`, so no control ships unnamed.
- **States are explicit.** `disabled(Option<SharedString>)` takes the
  reason, shows it as a tooltip, keeps the focus handle tracked but drops the
  tab stop, calls `prevent_default` on mouse-down so a press does not focus
  it (AppKit and GTK do not focus disabled controls either), and attaches no
  click listener, so neither the pointer nor an accessibility Click can act.
  Keyboard activation arrives as catalog commands routed to the component's
  methods (`toggle`, `step`, `confirm`), so each method checks the disabled
  and read-only state itself: a control can turn disabled while it has focus,
  as every Settings control does when the file turns invalid. `read_only()`
  keeps a control focusable and selectable but refuses edits.
  `invalid(message)` marks the control and shows its message under the row.
- **Owner-supplied focus.** Components accept a `FocusHandle` from their
  owner, so the Settings window's focus recovery and tab order keep working.
- **Keys stay catalog commands.** A component declares its key context and
  exposes methods (`select_next`, `confirm`, `step`, `toggle`); the owner's
  command handler routes catalog invocations to the focused component, as
  `Menu` and `TextField` already do. Components bind no keys, and their
  click handlers ignore `ClickEvent::Keyboard`, GPUI's implicit Enter and
  Space click (D16), so removing a binding really removes the key. Pointer
  clicks and accessibility Click actions still act. Existing contexts and
  command ids (`menu`, `Palette`, `SettingsControl`, `settings_*`) stay as
  they are. A generic `Control` context waits for a second window that uses
  these controls.
- **Pointer rules from the guide.** Rows select on `on_mouse_move`, never
  `on_hover`. Pressable elements pair hover with an `.active` pressed style.
  Popovers and dialogs occlude what they cover.
- **Focus visibility is decided when focus moves.** GPUI's `focus_visible`
  rechecks the input mode every frame, and any mouse move switches it to
  mouse (`window.rs:5603-5617`), so a ring set that way vanishes when the
  pointer is nudged after Tab. Each window records whether the last input
  was the keyboard at the moment its focused handle changes, and a pointer
  press anywhere in the window clears the record, because pressing the
  already-focused control does not change focus (`Window::focus` returns
  early). The ring follows that record. A click shows no ring (D9), even on
  the control Tab just reached; Tab shows one that a pointer move does not
  remove.

## Overlay, dismissal, and focus model

### Popover primitive

`Popover` owns one open surface per window: its anchor (trigger bounds or a
pointer position), its placement, its layer, its focus policy, and its
dismissal. Tooltips are separate: GPUI draws them in its own layer, and
showing one never closes a popover.

- **Placement.** Generalize the pure `place_menu` into `place_popover`: open
  below the trigger, or above it from the lower half; align to the trigger's
  left edge, or its right edge from the right half; clamp to the window
  margin; and return a `max_height` that the content scrolls within. A select
  menu is at least as wide as its trigger. The trigger's bounds come from the
  last paint, as `menu_bounds` does today.
- **Layer.** Content renders through `deferred` with one priority for every
  popover. Deferred content paints and hit-tests above everything drawn in
  normal flow, including the terminal window's dialogs, palette, and toasts,
  which draw in flow. So opening a modal dialog or the palette closes any
  open popover first, as the close confirmation already closes the menu
  (`windows.rs:8968`). Opening a popover closes any other open popover in the
  window. Submenus are out of scope.
- **Modality.** A modal popover (menu, select) draws a transparent occluding
  backdrop under its content, covering the window. A non-modal popover
  (combobox list) draws none and uses `on_mouse_down_out` instead.
- **Focus policy.** The owner chooses where focus goes on open and on close.
  A modal popover takes focus into its listbox, selecting the trigger's
  current value. A non-modal popover leaves focus in its field, so typing
  continues. On close, focus goes to the owner's return target: the trigger
  for Settings selects, and the active terminal for terminal-window menus,
  which must never keep focus on chrome
  ([desktop client guide](../agents/desktop-client.md)).

### Dismissal

Every dismissal has a reason, so the owner can choose focus and refuse where
needed:

| Reason | Modal popover | Non-modal popover | Focus afterwards |
| --- | --- | --- | --- |
| Escape | Closes | Closes; a second Escape reaches the field | Return target; the field for non-modal |
| Tab, Shift-Tab | Closes, as Escape does | Closes; focus moves on | Return target; the next or previous stop for non-modal |
| Pick | Closes after the owner applies it | Closes after the owner applies it | Return target; the field for non-modal |
| Press outside | Closes; the backdrop consumes the press | Closes; the press proceeds | Return target, or what the press focused for non-modal |
| Press on the trigger | Closes (toggle) | Owner decides | Return target |
| Focus leaves the popover and its trigger | Closes | Closes | Unchanged |
| Window deactivates | Closes | Closes | Return target, so focus is valid when the window returns |
| Opening a dialog, the palette, or another popover | Closes | Closes | The new surface |
| Owner state change (reload, row removed) | Closes | Closes | Return target, or the owner's fallback |

The backdrop answers D1, D2, and D5 at once. A press anywhere outside the
content, including on the trigger, lands on the backdrop, which closes the
popover and stops the press. That matches AppKit, GTK's autohide popovers, and
Radix's modal menus. The terminal window's menus today let the press proceed;
see the open questions. Shift-Tab gets a `menu_close` binding under `menu`,
as Tab has (D15). `menu_close` carries no direction, so in a modal popover
Tab and Shift-Tab only dismiss and return focus to the target, which matches
what Tab does today; a further Tab moves on. Radix Select and AppKit menus do
not move focus on Tab either.

Window deactivation must set focus itself. GPUI leaves focus on the closed
listbox's handle, which the Settings window then treats as orphaned and moves
to the window root, not the trigger. Blur-driven dismissal runs inside a
draw, so it goes through `cx.defer_in`, as the guide requires for work whose
result must show.

A press that arrives before the popover's first frame is handled by what
that earlier frame drew, which has no backdrop, as the guide records for
dialogs. A click-handler guard would come too late: GPUI moves focus on
mouse-down, in the bubble phase, and runs `on_click` on mouse-up, and the
terminal sends a mouse report on mouse-down. So the owner registers a
capture-phase mouse-down listener on every frame, whether or not a popover
is open, as the terminal window's menu host does today (`windows.rs:10549`).
It reads the live popover state, not the drawn frame. When a modal popover
is open and the press falls outside its last painted content bounds (or no
content has been painted yet; the bounds clear on every open and close, as
`menu_bounds` does today), it dismisses the popover and calls
`stop_propagation` and `prevent_default`. Non-modal popovers let the press
proceed, as their dismissal row says, and presses inside the content reach
its rows. So a second press on the trigger closes the popover
rather than opening another one, and nothing under where the popover will
draw takes focus or input. It consumes presses only: releases still reach
the gesture that owned them before the popover opened. Smokes wait for the
drawn-state field (`menu_rect`) before pressing.

### Focus

Focus never stays on an element that left the tree: the Settings window's
drawn-controls check stays, and `Popover` reports its own focus handle to it.

### Scroll

A modal popover's backdrop also takes wheel events, so the page does not
scroll away from an open select, as in AppKit and Radix. Content scrolls
within `max_height` with the existing list scrollbar. `ScrollArea` gains
`reveal(bounds)`, which scrolls the smallest distance that shows a focused
control fully; Tab and Shift-Tab call it (#276).

## Accessibility

Accessibility is part of every component's contract, not a later pass. GPUI
0.3.9 builds an AccessKit tree for macOS (VoiceOver) and Linux AT-SPI (Orca)
from elements that have an `id` and a `role`, while an assistive technology
is connected. It adds a Click action to elements with click listeners and a
Focus action to focusable ones. Huterm already links the adapters, so this
adds no dependency.

GPUI's setters cover label, description, value, placeholder, selected,
expanded, toggled, numeric value, minimum, maximum, step, orientation, level,
and set and grid positions. It has none for disabled, read-only, invalid, or
live regions, and no public way to name another element's node, so relations
such as "controls" or "error message" cannot be expressed. The convention:
set disabled, read-only, and invalid on the element's own node through
`a11y_synthetic_children` with `A11ySubtreeBuilder::parent_node`, one shared
helper in `ui/components/a11y.rs`. Every row below also exposes disabled.

| Component | Role | Properties | Actions |
| --- | --- | --- | --- |
| Button, link | `Button`, `Link` | label, description (disabled reason) | Click |
| Switch | `Switch` | label, `aria_toggled` | Click |
| ToggleChip | `Button` (AccessKit has no toggle-button role) | label, `aria_toggled` | Click |
| SegmentedControl, RadioCards | `RadioGroup` with `RadioButton` items | `aria_toggled` per item (AccessKit's macOS and Linux adapters read radio state from `toggled`, not `selected`), position and size in set | Click per item |
| Select trigger | `ComboBox` (select-only) | value label, `aria_expanded` | Click |
| Listbox, menu | `ListBox` and `ListBoxOption`; `Menu` and `MenuItem` | `aria_selected`, `aria_active_descendant`, size in set | Click per row |
| Combobox | `TextInput` on the field | value, `aria_expanded`; the highlighted suggestion as the field's description | Focus |
| TextField | `TextInput` | value, placeholder, read-only and invalid through the helper | Focus |
| NumberField | `SpinButton` | numeric value, min, max, step | Increment, Decrement through `on_a11y_action` |
| KeyRecorder | `Button` | value (the shortcut), description while recording | Click |
| NavList | `TabList` and `Tab` | `aria_selected` (tabs read `selected` on macOS) | Click |
| InlineConfirm | `AlertDialog` group | label, description | Buttons |
| Tooltip | none | becomes the target's description | |

Open risks, to settle in PR 1:

- GPUI honors an active descendant only under the focused node, and a
  combobox's list renders deferred, outside the field. VoiceOver may not
  announce the highlighted suggestion; the description fallback above is the
  plan, and a gap is accepted if it does not announce.
- GPUI exposes no caret or selection ranges, so VoiceOver can read a field's
  value but may not move by character.
- Footer status ("Saved") needs a live region, which only the raw-node helper
  can set; it is unverified that the adapters announce it.

The terminal grid itself is not part of this plan.

## Theming tokens

`Swatch` becomes `ui::theme::Tokens`, still derived from the terminal theme on
every reload and still passed by value. It has 13 `Hsla` fields plus
`ScrollbarColors` (two more), 240 of Clippy's 256 bytes, so it has room for
one more field at most:

- **Fields:** `fg`, `bg`, `surface`, `muted`, `dim`, `line`, `line_strong`,
  `hover`, `selection`, `accent`, `danger`, `warning`, `chip`, and
  `scrollbar`, as today.
- **Methods:** `pressed`, `selection_pressed`, `accent_pressed`, `on_accent`
  (the text color on an accent fill, `bg` today), `scrim`, `focus_ring`, and
  the shadows. Derived shades stay methods, as the guide requires, and a
  contrast correction is applied inside a method, not stored.
- **Sizes:** control height (24), menu row (28), radii (6 for controls, 9
  for raised panels), the focus ring (2-point spread), and the type scale
  (11.5, 12, 12.5). These are constants in one place, not literals in every
  builder.
- **Contrast:** the colors are computed from arbitrary user themes, so a
  test over the bundled themes checks body text and `on_accent` against
  WCAG's 4.5:1, and the focus ring and control borders against 1.4.11's 3:1.
  `line_strong` at 22% opacity is the likely failure. A theme that fails
  gets an adjusted token, not an exception.

## Testing strategy

- **Pure models.** Placement, listbox navigation, type-ahead, number stepping
  and clamping, and the dismissal table are functions with unit tests, as
  `MenuModel` and `place_menu` already are.
- **One GPUI test per contract row.** Each component mounts in a small host
  view under `TestAppContext` and is driven by real events: simulated clicks,
  mouse moves, keystrokes routed through the catalog keymap,
  `deactivate_window`, and `Window::dispatch_event` for the stale-frame
  contract. Each test asserts an observable outcome, including what must not
  happen:
  - an outside press on a switch closes the menu and writes and queues
    nothing;
  - a keyboard click from a removed Enter or Space binding does nothing,
    while the rebound key acts; the test sends key-down and key-up, because
    GPUI's implicit click fires on key-up and `simulate_keystrokes` sends
    only key-down;
  - after a terminal-window menu closes, a typed key reaches the terminal;
  - typing continues in a combobox while its list is open, and Escape first
    closes the list, then reaches the field, without saving early;
  - Tab, then a mouse move, keeps the focus ring; a click on that same
    control removes it. The control's click must not move focus, or the test
    passes over a focus-change-only record: PR 1 uses a focusable element in
    the test host that draws the ring from the window's record, since a
    select trigger's click moves focus into its listbox; PR 3 adds a switch;
  - Shift-Tab from an open menu closes it with focus on the trigger, and
    deactivation returns focus to the trigger;
  - the stale-frame guard: open a popover without drawing, dispatch a
    mouse-down over a switch or the terminal, and assert at once that the
    popover closed, focus is on the return target, and nothing was written,
    queued, or sent to the terminal; the terminal variant enables mouse
    reporting first and also asserts that no selection started; then draw,
    dispatch the mouse-up, and assert no delayed activation;
  - a disabled or read-only control refuses pointer, keyboard, and
    accessibility activation, including a control disabled while it has
    focus and then sent its catalog keys; the test first asserts that the
    control still has focus, so a key reaching the window root cannot pass
    it;
  - a focus-ring test asserts what the control draws, through a test-visible
    flag set where the ring is painted, not the window's keyboard record, so
    a control still styled with `.focus()` fails it. A lint in PR 1 rejects
    `Swatch::focus_ring()` outside the flag-gated ring painter (a plain
    `.focus(` search would also match `FocusHandle::focus` calls), and each
    migration PR extends it to the callers it moves, so a leftover
    `.focus()` beside a correct flag cannot still draw a ring after a click.
- **Accessibility tests need a GPUI patch.** GPUI's test window never
  activates accessibility: `TestWindow` keeps the platform's no-op
  `a11y_init`, so the tree is never built and `debug_a11y_tree_json` returns
  `None`. PR 1 adds a vendored `gpui-pre` patch, following
  [the dependencies guide](../agents/dependencies.md), that activates
  accessibility in test windows and lets a test send an AccessKit action.
  A canary test asserts that the tree exists and holds a known role, and
  every accessibility assertion fails on `None` instead of skipping. Tests
  check state transitions (a radio item's `toggled` flips) and action
  effects (an accessibility Click on a switch writes), not only that a
  property appears.
- **Regression tests first.** D1, D2, D3, D4, D5, D15, and D16 get Settings
  window tests that fail on `main` before PR 1 fixes them; D9 gets the focus
  ring test above. D7 gets its test in PR 4.
- **Smokes.** `scripts/check-settings.ts` gains a pointer step on macOS
  (through `hid-pointer`) and X11 (through `xdotool`): open a select by
  click, close it by an outside click and by a second trigger click, and
  check focus. Smoke state reports the focused control and `settings.menu`
  as separate fields, plus the menu's drawn rect, so a stale open menu can
  no longer read as focus.
- **Visual check.** A `component_gallery` example binary renders every
  component in every static state (default, focused, selected, disabled,
  read-only, invalid, open) in the bundled light and dark themes. It gives
  `ux-review` and screenshots one place per component, and needs no product
  surface or keymap entry.
- **Running app.** Each component PR gets a `ux-review` through Codex
  computer use on a scratch bundle and config. Codex drives the app partly
  through the accessibility tree, so missing roles may show up as
  unreachable controls. Add a VoiceOver pass on macOS, and an Orca pass in
  `vm:linux:dev`, for the main path.

## Migration order

Each step perfects one component against its references, then moves its
callers to it.

1. **Popover and Select in Settings.** Tokens, focus visibility, the GPUI
   accessibility test patch, the shared accessibility helper, `Popover` with
   the dismissal model, and `Select` on the existing `Menu`. This fixes D1
   to D5, D9, D15, and D16 for every Settings select, including Quake,
   keybinding arguments, and the font picker. PR 1's D16 test uses a select
   trigger; switches, buttons, links, and segments keep D16 until step 3.
2. **Terminal window menus** move to `Popover`, removing the second host
   (D13), with terminal focus as their return target.
3. **Buttons and toggles:** `Button`, `Switch`, `ToggleChip`,
   `SegmentedControl`, with roles, disabled reasons, the arrow-key decision,
   and keyboard-click filtering on every one (D8, D10, D16).
4. **TextField and NumberField:** read-only state, horizontal scrolling, Cut,
   and an accessible value (D12); `NumberField` with `x-huterm-step` schema
   metadata, clamping, PageUp, PageDown, Home, and End (D7). Field Undo stays
   out until it has a command id distinct from #256's window `undo`.
5. **Listbox and Combobox:** one listbox for menus and completion, moved into
   `ui/` with the architecture check, and the condition completion as a
   non-modal popover (D6).
6. **Navigation and scrolling:** `NavList` with arrow keys, `ScrollArea`
   reveal on focus, and `RadioCards` with a visible focus ring (#276).
7. **KeyRecorder:** repeat detection and draft preservation (#276).
8. **InlineConfirm and InlineEdit:** the delete prompt, with its Escape as a
   catalog command under a context of its own (D17), and, with #277, profile
   rename. Not `confirming`: it also binds Tab and Shift-Tab to
   `dialog_focus_*`, which would trap focus between the prompt's two buttons
   or do nothing in Settings. A D17 test checks that the rebound Escape
   cancels and that Tab still leaves the prompt.

## PR split

| PR | Contents | Depends on |
| --- | --- | --- |
| 1 | `ui/components/` skeleton, tokens, focus visibility, `Popover`, `Select`; the shared accessibility helper; GPUI test-window accessibility patch and canary; Settings selects migrated; regression tests for D1 to D5, D9, D15, D16; the `shift-tab` `menu_close` binding; smoke pointer step and separate focus and menu state; gallery example | Rebased on #256 if merged; its `keymap.rs` change is one line in `menu_defaults` |
| 2 | Terminal window menus on `Popover` | PR 1, #256 merged |
| 3 | `Button`, `Switch`, `ToggleChip`, `SegmentedControl`; D16 regression tests for each | PR 1 |
| 4 | `TextField` hardening, `NumberField`, `x-huterm-step` | PR 1 |
| 5 | `Listbox` and `Menu` moved into `ui/`, `Combobox`, architecture check | PR 2 |
| 6 | `NavList`, `ScrollArea` reveal, `RadioCards` | PR 3 |
| 7 | `KeyRecorder` | PR 3 |
| 8 | `InlineConfirm`, `InlineEdit`, profile rename | PR 3, #277 scope |

PRs 3 and 4 can run in parallel after PR 1, and PR 5 after PR 2. Each PR
updates its components' contracts, the gallery, and the relevant agent guide,
and closes the #276 or #277 items it covers.

## Non-goals

Terminal grid accessibility; submenus; a slider, date picker, or virtualized
list; rewriting the palette, toasts, close dialog, tab strip, or pane
gestures; theme color editing; and any dependency on `gpui-component` or
Zed's GPL-covered `ui` crate.

## Open questions

1. **Should an outside press that closes a menu also act?** Recommendation:
   consume it everywhere, through the backdrop. For: AppKit, GTK autohide
   popovers, and Radix modal menus all consume it; a dismissing click cannot
   toggle a switch or, in a terminal, start a selection or send a mouse
   report to a TUI. Against: the terminal window lets it proceed today, so
   clicking a tab to dismiss a menu would take two clicks.
2. **Should Down and Up on a closed select open it, or step its value?**
   Recommendation: open it, and keep arrow stepping for segmented controls
   and number fields. For: Radix, AppKit, and GTK open the list, and stepping
   saves on every press. Against: stepping in place is faster for short
   lists and matches Windows and HTML `<select>`, and the settings plan's
   PR 3 contract documents arrow stepping on selects.
3. **Should switches keep arrow keys?** Recommendation: drop them; Space and
   Return toggle, as WAI-ARIA, Radix, and AppKit define. For: an arrow press
   while moving through a form should not write. Against: it removes a
   working shortcut that the PR 3 contract documents.
4. **Patch vendored GPUI for accessibility tests, or verify accessibility by
   hand?** Recommendation: patch. For: without it, every accessibility claim
   in the contracts is untested and regresses silently; the patch is small
   and test-only. Against: each GPUI upgrade must carry the patch until
   upstream adds an equivalent.
5. **Module or crate?** Recommendation: a module in `huterm-gpui` with an
   architecture check. For: no second consumer exists, and the text client
   will not use GPUI. Against: a crate enforces the boundary through Cargo
   and makes compile-time isolation possible later.
6. **Gallery: example binary, debug Settings section, or none?**
   Recommendation: an example binary. For: it shows every state in both
   themes without a product surface or keymap entry. Against: it is one more
   target to build in CI and to keep in step with the components.
7. **Adapt `gpui-component` code?** Recommendation: allow it per component
   with attribution, but default to writing Huterm's own. For: Apache-2.0
   permits it, and its anchoring and list code are mature. Against: adapted
   code carries its assumptions (global theme, `Root`, hardcoded keys) and a
   notice to maintain.

[issue-276]: https://github.com/jimeh/huterm/issues/276
[issue-277]: https://github.com/jimeh/huterm/issues/277
[radix]: https://www.radix-ui.com/primitives/docs/components/select
[apg]: https://www.w3.org/WAI/ARIA/apg/patterns/
[hig]: https://developer.apple.com/design/human-interface-guidelines/pop-up-buttons
[gnome-hig]: https://developer.gnome.org/hig/
[gpui-component]: https://github.com/longbridge/gpui-component
