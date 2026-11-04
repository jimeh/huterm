# Blur with any transparency, blur strength, and per-window overrides

Status: implemented

Issue #399. Follows [window transparency](window-transparency.md) (#39,
PR #392). Blur applies whenever a window is translucent, its strength becomes a
radius, and each window and Quake profile can override the four transparency
values.

## Decisions

Jim made these decisions on 2026-11-04:

- Blur applies whenever a window is translucent (`opacity < 1` or
  `background_opacity < 1`) and `blur > 0`.
- `[window] background_blur` (a bool, never released) becomes `blur`, a
  radius from 0 (off) to 64, with a default of 0. There is no compatibility
  alias, so the old key fails as an unknown key.
- macOS blur uses the window server's background blur radius (the private
  `CGSSetWindowBackgroundBlurRadius`) instead of GPUI's `Blurred`
  `NSVisualEffectView`.
- **The window server's blur fades with the native alpha.** On macOS 27.0.1
  a probe measured stripe sharpness behind a clear window with radius 20.
  Alpha 1 gave 0.25 (fully blurred), alpha 0.5 gave 31.75 and alpha 0.2 gave
  50.75, against 63.75 for no blur. So alpha 0.5 leaves the backdrop half
  sharp. `...WithOpacityHint` behaved the same. The radius-0 lockout did not
  reproduce: radius 20, then 0, then 20 blurred again.
- Jim picked the **hybrid**:
  - Blur off (`blur = 0`): `opacity` stays the native window alpha, unchanged
    from #392.
  - Blur mode (`blur > 0` and any translucency): the native alpha's
    configured factor is pinned at 1, and `opacity` becomes GPUI element
    opacity on the window's root element. Every primitive drawn in the tree
    (quads, glyphs, emoji, SVGs, images, paths, shadows) multiplies by it.
  - Overlapping translucent layers blend unevenly in blur mode. This is
    accepted and documented.
  - The Quake fade stays a native-alpha factor in both modes.
- The Settings > Appearance > Transparency values become the global defaults.
- Each window can override any of the four values. Each value inherits on its
  own, and a live change to a global default reaches every window that doesn't
  override that value.
- The window panel is **mockup A**, an inspector card at the top right of the
  content area, below the tab bar or title row:
  - There's no scrim, and the terminal stays visible while it's open.
  - It's titled "Window Transparency" and has a close button.
  - It has four rows, each with a label, a source ("Default", "Profile", or a
    dot and "This window"), a slider with a readout, and an icon-only Reset.
    Reset keeps its slot while there is nothing to reset.
  - Its footer has "Reset All" and "Edit Defaults in Settings…".
- A press outside the panel, Escape, or the command again closes it, and focus
  returns to the terminal.
- `[quake.profiles.<name>]` gets optional `opacity`, `background_opacity`,
  `foreground_opacity` and `blur` keys, edited in a new Transparency card under
  the profile's card in Settings > Quake.
  - Values layer as global defaults, then the profile, then the window
    override.
  - An unset profile key describes itself as "Uses Appearance > Transparency
    (60%)".
- Overrides are runtime state in the client window model. This change adds
  no persistence.
- Out of scope: Linux blur (Refs #400), and named appearance profiles.

## Approach

### Configuration (`huterm-config`)

- `WindowConfig::blur: u8`:
  - schema range 0 to 64, step 1, slider control, unit `pt`, zero-label
    "Off", platform `macos`, apply `live`, order 39, label "Blur";
  - its description says it blurs what shows through whenever the window is
    translucent;
  - `validate_opacity` checks the range.
- `quake::Profile` gets `opacity`, `background_opacity`, `foreground_opacity`
  (`Option<f32>`) and `blur` (`Option<u8>`).
  - Each is `#[serde(default, skip_serializing_if = "Option::is_none")]`.
  - `Profile::validate` checks the same ranges as `WindowConfig`.
  - Schema metadata: section `quake`, group "Transparency", the same labels,
    steps and controls as the window keys, and a new `x-huterm-inherits`
    key naming `window.<key>`.
- Settings model (`settings.rs`):
  - `kind` accepts a `type` array of one type plus `"null"` as that type.
  - `Setting` gains `inherits: Option<String>`, parsed from
    `x-huterm-inherits` and added to the known `KEYS`. The model refuses a
    reference that doesn't name a setting of the same kind.
  - A unit test covers the parse and the refusal.
- Regenerate `schemas/` (`mise run schema:generate`), then update
  `schemas/fixtures.json`, the commented template in
  `crates/huterm-gpui/src/config.rs`, and `scripts/check-palette.ts` wherever
  they name `background_blur`.

### Resolution (`huterm-gpui/src/transparency.rs`)

- `Values { opacity, background, foreground, blur }` come from
  `WindowConfig`. `Layer` holds the same four fields as `Option`s, with
  `fn over(self, Values) -> Values`. A Quake profile converts to a `Layer`,
  and window overrides are a `Layer`. `Source { Default, Profile, Window }`
  reports where each effective value came from, for the panel.
- A pure `resolve(values, support, platform, decorated) -> Effective` returns
  `native_alpha: f64`, `root_opacity: f32`, `background: f32`,
  `foreground: f32`, `translucent: bool`, `blur_radius: u8` and
  `appearance`:
  - Without support it matches today's opaque fallback: no translucency, and
    a radius of 0.
  - Blur mode is macOS, `blur > 0`, and `opacity < 1` or `background < 1`.
    It gives `native_alpha = 1`, `root_opacity = opacity`,
    `blur_radius = blur`, `translucent = true` and a `Transparent`
    appearance.
  - Otherwise `native_alpha = opacity` and `root_opacity = 1`, with a radius
    of 0, and the appearance follows today's rule without `Blurred`.
    `translucent` is `background < 1`.
- `translucent` replaces every structural `background_alpha < 1.0` check.
  `background` stays the paint alpha. Both reach:
  - the renderer: `set_opacity` takes `translucent`, and the branches at
    `renderer.rs` ~480, `solid_color` ~543 and `paint_background_layer` ~563
    use it to tint explicit background runs, selection and cursor colors;
  - the window: the root and frame underlay (`windows.rs` ~12154, ~12261),
    the two chrome-fill branches (~12632, ~12877), the
    tint-versus-overlay-dim choice in `configure_pane_paint` and
    `dim_layers`, and `tab_colors`.

  The opaque fast paths stay as they are when `translucent` is false. The flag
  prevents structural stacking. Primitives that overlap still blend
  unevenly, which the decisions accept.
- `Support::paint` and `appearance` give way to `resolve`.
  `WindowBackgroundAppearance::Blurred` is no longer produced. The bench's
  `HUTERM_BENCH_APPEARANCE=blurred` keeps passing GPUI's appearance directly.

### Applying effective values, and mode-switch ordering

- `Alpha` stays the only native-alpha owner. Its configured factor becomes
  `effective.native_alpha`, and the Quake fade multiplies it in both modes.
- `Alpha::sync` writes on a later foreground turn. So that the painted root
  opacity follows the native alpha, the view keeps the root opacity it paints
  (`painted_root_opacity`) separate from the resolved one. This orders the
  state, not the screen: the native alpha write and the GPUI frame present
  separately, so a mode switch can show at most one frame at the old
  combination (`opacity²`, or opacity 1).
  - Entering blur mode, the native alpha goes to 1 first, and the root opacity
    is published only when `sync`'s write lands. The continuation updates the
    view and notifies. A failed write publishes nothing, so the root keeps
    its old opacity.
  - Leaving blur mode, the root opacity drops to 1 in the same continuation
    that writes the native alpha.
  - Without a native window, as in tests or after a failed handle lookup, both
    apply at once.
- The switch keeps the Quake effect generation, progress and deadlines.

### Native blur (macOS)

- `native_appearance::macos::NativeWindow::set_blur_radius(u8)` calls
  `CGSSetWindowBackgroundBlurRadius(CGSMainConnectionID(), windowNumber,
  radius)`. Both symbols are resolved once with `dlsym`, and the
  `windowNumber` is read from the retained `NSWindow` on the main thread at
  the time of the write.
- A per-window `Blur` owner sits next to `Alpha`, with `desired`, `applied`
  (`Option<u8>`, where `None` means unknown) and a pending flag:
  - It writes on a later foreground turn, outside GPUI update borrows, and
    reads `desired` live when it runs.
  - Only a successful write sets `applied`. A failure sets it to `None` and
    logs once per window. The next change, reload or style change retries
    once, with no timer loop.
  - It never writes 0 when `applied` is `Some(0)` or the window never had
    blur.
  - A missing symbol logs once per process and leaves blur unused. The
    write reports blur unsupported, so `applied` stays unknown. Translucency
    still works.
  - Closing the window drops the pending work.
- A Quake style change, conversion to or from regular, or native fullscreen
  marks `applied` unknown, so the next sync writes the desired radius again.
  Quake already reasserts state there.

### Window state

- `WindowRecord` gains `transparency: Layer`, the window overrides.
  `WindowModel::set_transparency` returns whether the layer changed.
  `WindowRestore` gains the same field, with no writer, for #42.
- `WorkspaceView` resolves its effective values from three sources:
  - the global `Config.window`;
  - the layer of the profile its Quake association names. That follows the
    window through temporary regular presentation and takes a renamed
    profile's new name (`quake_windows.rs` ~1134). The layer drops on the
    reload that removes the profile, because the profile's values are no
    longer available, even while the window still holds the association;
  - its record's overrides.
- It re-resolves at open, on reload, on a profile rename or removal, on
  conversion and recovery, on an override change, and on a decoration
  change. None of these restarts an existing fade.
- A change to a profile's transparency alone must not restart Quake
  presentation. Compare `Profile` without the transparency fields wherever
  `quake_windows.rs` ~564 compares whole profiles.
- `pane_view` (`windows.rs` ~8148) and `TerminalView::new` (`desktop.rs`
  ~744) take the destination window's resolved paint state (`background`,
  `foreground`, `translucent`), so a new pane never paints global values
  first.
  - Cross-window transfers install destination views asynchronously
    (`cross_window.rs` ~830). Installation applies the destination's state as
    it stands at installation, so an override that changes while a transfer
    is pending is honored.
  - A torn-off tab's new window starts with no overrides.
- Every remaining read of `config.window.{opacity, background_opacity,
  foreground_opacity, background_blur}` reads the resolved values instead,
  including `warn_transparency`.
- The root element carries `.opacity(painted_root_opacity)` when that is
  below 1. Popovers, menus and tooltips paint outside the root, so they stay
  solid in blur mode. Document this with the overlays that already stay
  opaque.

### Panel

- Catalog command `window_transparency`:
  - title "Window Transparency…", Window scope, no default shortcut;
  - in the View menu, in the group with Toggle Fullscreen, before it;
  - listed by the palette;
  - it toggles the panel;
  - its availability goes in `WorkspaceView::command_availability`.
- Panel keys use a new `Panel` key context and a small `panel_*` family of
  Window-scope commands gated on it, with defaults identical on both
  platforms:

  | Command | Keys |
  | --- | --- |
  | `panel_close` | Escape |
  | `panel_focus_next` | Tab |
  | `panel_focus_previous` | Shift-Tab |
  | `panel_increase` | Up, Right |
  | `panel_decrease` | Down, Left |
  | `panel_page_up`, `panel_page_down` | Page Up, Page Down (on a slider) |
  | `panel_first`, `panel_last` | Home, End (on a slider) |
  | `panel_press` | Enter and Space (on a button) |
  | `panel_reset` | Cmd-Backspace (macOS), Ctrl-Backspace (Linux), on a slider |

  - These follow the menu and dialog families: each binding is conditional, so
    none reserves a terminal key.
  - Settings' commands and contexts are not reused, because `Settings` would
    bring `settings_search` and Settings entries into the palette.
  - The stepping math is `huterm_ui::steps::Steps`, as in Settings.
  - `docs/keybindings.md` lists the family.
- The panel has its **own** `huterm_ui` `Popover` and guard, separate from
  the shared menu popover:
  - `close_menu` no longer closes it. Opening a menu, the palette or a dialog
    closes the panel first, and opening the panel closes an open menu.
  - `run_command` routes `panel_*` to the panel before its generic menu
    dismissal.
  - `overlay_holds_focus` includes the panel.
  - A reload keeps it open and refreshes its rows.
  - Mount its guard after the window-level listeners, as for the menu popover.
- `desktop/windows/transparency_panel.rs` renders mockup A:
  - It is 288 pt wide, at the top right of the content rect, inside the client
    frame and fullscreen safe area, and below the visible tab bar or title
    row.
  - In a window too short for it, the body scrolls in a `ScrollArea::fill` and
    the footer stays visible.
  - It uses `huterm_ui` `Slider` and `Button`: an icon-only `RotateCcw` Reset
    named "Reset Opacity" and so on, with tooltip "Use the default" or "Use
    the profile value". It also uses `Separator` and link-variant footer
    buttons.
  - Steps follow the schema: 0.05 for the three opacities over their ranges,
    and 1 for blur from 0 to 64. Readouts are percents, and blur shows "Off"
    at 0.
- Focus:
  - Opening the panel focuses the first enabled slider. With every slider
    disabled, it focuses the close button.
  - Closing it returns focus to the active terminal. If a pane closed
    meanwhile, focus goes to the tab's focused pane.
  - It closes on window deactivation, on an outside press (consumed, as menus
    do, including a press against a stale frame), when the window starts
    closing, and when a menu, the palette or a dialog opens.
- Edits:
  - Dragging or stepping a slider writes the window's override at once,
    through `WindowModel::set_transparency`, and the window repaints.
  - Reset clears that value, Reset All clears all four, and Reset All is
    disabled when nothing is overridden.
  - "Edit Defaults in Settings…" closes the panel and opens Settings at
    Appearance > Transparency.
- States:
  - The panel hides the Blur row on Linux.
  - Without transparency support, the sliders are disabled with the
    `Support::note` reason, and the note shows at the top of the panel.
  - A Quake window shows "Profile" for values its profile sets.
- Accessibility: the panel is a `Group` named "Window Transparency". Sliders
  carry their labels and readouts, Reset buttons are named per row, and the
  focus ring comes from `focus::ring`.

### Settings

- The Quake section renders the profile's `group == "Transparency"`
  templates, through `profile_setting`, in a second card titled
  "Transparency" under the profile card.
- One displayed-value resolver serves rendering, slider and stepping math,
  and keyboard steps: the pending value, else the file value, else (for a
  setting with `inherits`) the inherited setting's own displayed value, else
  the platform or schema default. A pending Reset of an inheriting row shows
  the parent's value and steps from it, and a pending change to the parent
  shows at once.
  - `values::shown`, `SettingsView::value` and `default_value` share this
    resolver.
- Stored-value identity is unchanged:
  - the changed dot and Reset eligibility use `file.is_set` on the profile
    key;
  - a write conflict compares the raw profile key;
  - inherited values are never written.
- An unset inheriting row's description reads "Uses Appearance >
  Transparency (60%).", with the inherited readout.
- Search: profile rows, existing ones included, aren't search results today,
  and this change keeps that. The Appearance Transparency rows stay
  searchable, and a test asserts that the `blur` row is found as "Blur".
- Platform: the profile and Appearance blur rows keep `available_on`'s rule.
  They hide on Linux unless the file sets the key, and then they show as
  foreign.
- The Appearance blur row becomes the slider.

### Documentation

- `docs/configuration.md`: `blur`, blur mode and its uneven blending, the
  panel, layering, and the profile keys.
- `README.md`, if it lists transparency.
- `docs/keybindings.md`: `window_transparency` and the `panel_*` family.
- `docs/agents/window-presentation.md`:
  - the native-alpha owner in blur mode;
  - the probe finding;
  - the blur owner's rules and the mode-switch ordering.
- `docs/agents/desktop-client.md`: the panel's own popover, its focus and
  key rules, and the `Panel` context.
- Mark `docs/plans/window-transparency.md`'s blur section as superseded by
  this plan.

## Observable success

- macOS:
  - opacity 0.6, background 1 and blur 20 shows the desktop evenly blurred
    through the window;
  - background 0.6 with blur 20 does too, and so do both values together;
  - with blur 0, opacity 0.6 fades the window evenly through native alpha, as
    in #392.
- Dragging the blur slider changes the radius live. 0 turns blur off, and a
  value above 0 turns it back on in the same window.
- Overriding opacity in window A leaves window B alone. Resetting brings back
  the default. A later change to the global opacity reaches B, and reaches A
  only if A doesn't override opacity.
- A Quake profile with `opacity = 0.8`:
  - its window rests at 0.8;
  - a window override of 0.5 shows 0.5;
  - the fade ends at the effective opacity in both modes.
- Linux:
  - without support, windows stay opaque and the panel's sliders are
    disabled with the note;
  - with a compositor, opacity overrides work;
  - Settings and the panel show no blur control.

## Failure paths and risks

| Risk | Handling |
| --- | --- |
| Private CGS symbol missing or failing | `dlsym` once, log once, and skip blur. Translucency still works. |
| Radius 0 locks blur off (reported elsewhere) | Write only on change, and never write 0 to a window that never blurred. A manual check covers 20, then 0, then 20. |
| Mode switch shows `opacity²` or a flash | The painted root opacity follows the native write (see ordering). At most one frame can still show the old combination, because the native write and the GPUI frame present separately. |
| The shared menu popover closes the panel | The panel has its own popover. |
| Blur mode underlay double-blends | Structural checks use `translucent`, and a window test asserts one background painter. |
| A reload or fade reenters during a native write | Keep the `Alpha` reconcile loop. The blur owner applies on a later turn and rereads live state. |
| A Quake profile rename or removal leaves a stale layer | The layer is looked up by the current profile name on each resolve. `reconcile_quake` covers renames and removals. |
| Panel focus is stranded when a pane closes or a reload happens | A reload refreshes the rows. Closing returns focus to the tab's focused pane. |
| Deferred overlays stay solid in blur mode | Accepted and documented. |
| Old `background_blur` in a file | It fails as an unknown key. The key was never released, so nobody has it. |

## Non-goals

- Linux blur (Refs #400), and persisting overrides (#42).
- Named appearance profiles, and opacity for the Settings window.
- Per-tab or per-pane transparency.

## Verification

| Concern | Evidence |
| --- | --- |
| Layering, sources, and per-value inheritance | Unit tests of `Layer::over` and `Source` |
| Blur mode against native alpha, unsupported fallback, Linux never blurring, `translucent` | Unit tests of `resolve` |
| Quake fade with configured 1 in blur mode; mode switches both ways during a fade keep generation, progress and deadlines | `Alpha` and Quake policy tests |
| Mode-switch ordering: the painted root opacity follows the native write, and a failed write publishes nothing | Unit tests of the painted-root-opacity handoff with a fake writer |
| Blur owner: write only on change, never 0 on a fresh window, failure marks unknown and retries once, close cancels | Unit tests with an injected writer |
| Config ranges, optional profile keys, old key rejected | `huterm-config` tests |
| Settings model `inherits` and nullable kinds | `settings.rs` tests |
| Settings resolver: pending Reset shows and steps from the parent; parent change shows; conflicts on the raw key; Use mine and Use file; foreign blur row on Linux; Blur search | Settings window tests |
| An override changes one window only; a global reload reaches non-overriding windows | Window test with two windows (`window_tests/transparency.rs`) |
| New panes, pane and whole-tab transfers between windows with different overrides, an override changed while a transfer is pending, tear-off defaults | Window tests (`cross_window` harness) |
| Emitted scene alpha in blur mode with `bg = 1`: one background painter; inactive panes, explicit backgrounds, OSC 11, selection, cursor, merged title tabs | Window or renderer tests that read painted alphas, not style flags |
| Panel: command, Escape, outside and stale-frame presses, deactivation; focus return incl. after a pane closes; real key dispatch for arrows, Page and Home/End, Tab, Enter and Space, Reset, and a user rebinding; all-disabled focus; Reset All state; Linux blur row hidden; short window | Window tests dispatching keystrokes, plus a11y-tree assertions for names and states |
| Panel keeps working across a reload; a menu or palette opening closes it | Window tests |
| View menu item and palette entry | `smoke:macos-menus` and `smoke:macos-palette` extended; menu-spec tests |
| Real blur at both alphas, live radius, 20 then 0 then 20, after Quake conversion and native fullscreen | Manual check on a scratch bundle (Codex computer use, background), with cropped screenshots for the PR. Optional: a smoke-state readback of the applied radius and native alpha. |
| Linux X11 transparency fallback | `mise run linux:smoke` in Docker (`smoke:linux-quake` and `smoke:linux-palette` cover the paths) |
| Repository gates | `mise run check`, focused cargo tests, `mise run test`, `mise run verify` before handoff |
| Fidelity to mockup A | UX review compares the running panel with `/tmp/huterm-399-mockups/mockups.png` |

## Open questions

None. A Codex (`gpt-6.1-sol`) plan review on 2026-11-04 led to the panel's own
popover, the `panel_*` commands, the renderer's `translucent` flag, the
Settings displayed-value resolver, the mode-switch ordering, and the blur
owner's applied-state rules.

## Revisions during implementation

- The panel's Blur readout reads "20", as mockup A shows, and "Off" at 0.
  Settings keeps its "20 pt" readout.
- `huterm_ui` `Slider` gained `readout_width`, which right-aligns the readout
  in a fixed column, so the panel's four tracks line up as in mockup A.
- The panel's Tab order follows its visual order: Close, then each row's
  slider and its Reset while that value is overridden, then Reset All while
  anything is overridden, then Edit Defaults in Settings, wrapping.
- Edit Defaults in Settings opens Settings at Appearance and scrolls the
  Transparency card's heading to the top of the pane, or as far as the pane
  scrolls, once the section's layout settles
  (`settings_window::open_at_group`). A new window's theme scan grows the
  gallery above the card after its first frame, so the reveal waits for the
  file read and theme scan to finish and for two frames that agree.
- Each panel slider's accessible description names its value's source:
  "Uses the default", "Set by the Quake profile", or "Set for this window".
  A disabled slider's reason replaces it, through `huterm_ui` `Slider`'s new
  `description`.
- In a panel too short for its rows, every keyboard focus move, including
  the slider that takes focus after Reset or Reset All, scrolls the body to
  show the focused control's whole row.
- The panel closes when the window starts closing through `remove_window`,
  and through `sync_dialog` when a close confirmation shows, rather than at
  the top of `request_close`. The behavior is the same, and `request_close`
  stays within Clippy's length limit.
- `warn_transparency` checks the defaults and every Quake profile's keys
  layered over them. Window overrides need no warning: without support the
  panel's sliders are disabled, and on Linux it has no Blur row.
- `WindowRestore::transparency` is filled from the window's record by the
  Quit capture, so #42 only has to persist it; nothing writes it to disk.
- A pending Reset of a setting that inherits nothing still holds no value in
  `SettingsView::value`, as before; only an inheriting row resolves a
  pending Reset to the inherited value.
- The profile transparency keys apply live, unlike the other profile keys,
  and `quake::Profile::same_presentation` compares profiles without them.
- Smoke state gained `root_opacity`, `blur_radius` (the radius last written,
  or `unknown`), `transparency_overrides`, and `transparency_panel` (the
  focused control, `unfocused`, or `closed`).
- The pane title bar's indicator tint follows `translucent`, like the other
  structural checks: a translucent window blends the tint into the bar's one
  fill, so blur mode over an opaque background no longer stacks a tint
  overlay under the root's opacity.
- Any command other than a `panel_*` command and `window_transparency`
  closes the panel first and returns focus to the terminal, as the menu
  does, so Next Tab, New Tab or a split leave no open panel behind focus.
- Without the private blur symbols, a write reports blur unsupported: the
  applied radius stays unknown, nothing writes again, and smoke state shows
  `blur_radius=unsupported`.
- `x-huterm-inherits` refuses a target that inherits another setting, so a
  chain or cycle cannot reach the Settings resolver.
- `Alpha::sync` runs its landed callback only after a successful native
  write, so a failed write keeps the old painted root opacity.
- A pending Settings group reveal cancels when the user edits the search,
  scrolls the pane with the wheel or trackpad, or presses anywhere in the
  pane, its scrollbar included. The press cancels in the capture phase,
  because a quick scrollbar click can press and release between two frames,
  before a later check could see a drag.
