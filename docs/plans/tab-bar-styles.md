# Tab bar styles

Status: planned on 2026-09-15. Not yet implemented.

Visual reference: the interactive
[tab bar mock-up](https://plans.jimeh.dev/7fdnlj2dvbgvnmvnse75i52aty/huterm-tabs-workspaces-sessions.html),
section 1. Its workspace sidebar and session switcher are out of scope here.

## Outcome

Replace the current plain tab bar with two themed horizontal styles, Strip and
Pill, and one matching vertical style. Tabs can stretch to fill the bar or fit
their titles within configurable bounds. Tab settings move to a new `[tabs]`
config table, themes gain UI colors, and the chrome gains Lucide icons.
Single-tab windows with a hidden tab bar extend the terminal background into
the macOS titlebar. Everything ships in one PR.

## Settled decisions

- **Config table.** Tab settings move from `[window]` to `[tabs]`. This is a
  breaking change. The old keys produce an error that names the new key.

  ```toml
  [tabs]
  position = "top"   # top, bottom, left, or right
  always_show = false
  auto_hide_in_fullscreen = false
  style = "strip"    # strip or pill; top and bottom only
  width = "fill"     # fill or fit; top and bottom only
  min_width = 96.0   # logical points; fit only
  max_width = 240.0  # logical points; fit only
  ```

- **Defaults.** Strip style with Fill width, which stays closest to today's
  layout.
- **Bar height.** Both styles keep the existing 32-point `TAB_HEIGHT`, so
  `ChromeLayout`, PTY sizing, and existing layout tests keep their geometry.
- **Vertical tabs.** Left and right placement always use the Strip-like row: a
  status slot, the title, and an accent bar inside the rounded active row. A
  secondary directory line waits for directory metadata. `style`, `width`,
  `min_width`, and `max_width` do not affect vertical tabs.
- **Status.** Only exited terminals get an indicator in this work.
- **Theme colors.** Themes gain flat, optional UI color keys that merge through
  `extends` like existing keys. Missing values derive from the theme's
  foreground, background, and ANSI colors. Every bundled theme also gets
  hand-picked values.
- **Icons.** Use Lucide SVGs extracted from its npm package. Confirm that its
  license file permits bundling before committing any icon.
- **Single-tab titlebar.** When the tab bar is hidden because the window has one
  tab and `always_show` is false, the titlebar area uses the terminal background.
  When the bar is visible, the titlebar uses the tab bar background.

## Non-goals

- The workspace sidebar, session switcher, and workspace colors.
- Bell, unseen-output, running-job, and working-directory indicators. The
  protocol has `Bell(TerminalId)`, but huterm-gpui does not consume it yet.
- Changes to titlebar content such as the static `Huterm` label.
- Tab drag between windows or any core structural change.

## Current implementation

- `WindowConfig` in `crates/huterm-config/src/lib.rs` holds `tab_position`,
  `always_show_tab_bar`, and `auto_hide_tab_bar_in_fullscreen`. It uses
  `deny_unknown_fields`.
- `WorkspaceView::render` in `crates/huterm-gpui/src/desktop/windows.rs` draws
  the whole bar inline: absolutely positioned tabs, `×` and `+` text glyphs, a
  `foreground.opacity(0.12)` active background, and no hover states.
- `TabStrip` in `windows/tab_strip.rs` owns tab geometry with one shared
  `extent`. Rendering, wheel scrolling, reveal, reorder slots, drop markers, drag
  previews, and autoscroll all position tabs at `extent * index`.
- `TabView::title` appends a `· exited` suffix for exited terminals.
- `Theme` holds foreground, background, cursor, selection, an optional
  selection foreground, and 16 ANSI colors. `ThemeDefinition` mirrors these as
  optional strings with `extends`. The repository bundles 13 theme files in
  `crates/huterm-gpui/themes` plus the built-in `huterm-dark` theme.
- huterm-gpui registers no GPUI `AssetSource`. The vendored GPUI 0.2.2 provides
  `svg()`, rendered through resvg, and `Application::with_assets`.
- `titlebar_inset` reserves a titlebar area only on macOS outside fullscreen.
  The titlebar div has no background of its own.

## Design

### Config migration

Add a `TabsConfig` to `huterm-config` and remove the three tab keys from
`WindowConfig`. Validate that `min_width` is at least 48, `max_width` is at most
600, and `min_width <= max_width`. Width bounds are accepted in Fill mode but
have no effect there.

Detect the three legacy `[window]` keys before strict deserialization and report
a config error such as `window.tab_position moved to tabs.position`. Startup and
reload then follow the existing invalid-setting fallback, which preserves the
clipboard policy and other diagnostics.

Config reload must apply style, width, and bound changes to open windows. A
change that alters tab geometry must re-clamp the tab scroll offset.

### Theme UI colors

Add these flat optional keys to `ThemeDefinition` and resolved fields to
`Theme`:

| Key | Use | Derived default |
| --- | --- | --- |
| `tab_bar_background` | Horizontal bar, vertical column, titlebar with a visible bar | Background mixed toward black, or toward the foreground for light themes |
| `tab_active_background` | Pill active tab and vertical active row | Background mixed toward the foreground |
| `tab_foreground` | Active tab text and icons | `foreground` |
| `tab_inactive_foreground` | Inactive tab text and icons | Foreground mixed toward the bar background |
| `tab_border` | Separators and bar edge | Foreground at low opacity over the bar background |
| `tab_accent` | Strip accent line, Pill index badge, vertical accent bar | `ansi[4]` |

The Strip active tab always uses the terminal `background`, so it merges with
the terminal below it. Hover backgrounds derive from the foreground at low
opacity and are not themed.

Derivation needs real color mixing because the bar can be darker than the
terminal background; opacity overlays cannot darken. Put mixing and light-theme
detection in one tested helper. Tune the mix amounts against the Tokyo Night
mock-up during implementation.

Give every bundled theme explicit values. Prefer a surface color that the
upstream palette defines, such as Tokyo Night's `#16161e`; otherwise choose by
hand. Keep the existing upstream license notices. Update the theme schema,
theme fixtures, and `crates/huterm-gpui/themes/README.md`.

### Icons

Add `lucide-static` as a pinned Bun devDependency, subject to the three-day
release-age policy in `bunfig.toml`. Add a script with two Mise tasks. Use a
name distinct from the existing app-icon `icons:*` tasks, for example:

- `ui-icons:generate` copies an explicit list of SVGs into
  `crates/huterm-gpui/assets/icons/`.
- `ui-icons:check` compares the committed bytes with the package and fails on
  drift. Include it in `mise run check`.

The initial list is the close, plus, chevron left, chevron right, chevron up,
chevron down, and one exited-status icon. Embed the files with `include_bytes!`
behind an `AssetSource` registered at application startup, so packaging and
runtime paths do not change.

Lucide files sit outside Cargo's license audit. Add a notice under
`third-party/`, include any upstream notice the license file carries, and ship
the notice with the other packaged resource licenses.

Verify early that GPUI tints monochrome SVGs with the element's text color. If
it does not, choose the fallback before building the tab styles on icons.

### Tab rendering

Move tab item rendering out of the inline `render` block into a style-aware
helper beside `TabStrip`. Keep `TabStrip` as the only source of pixel geometry.

- **Strip.** The bar uses `tab_bar_background` with a `tab_border` edge. The
  active tab uses the terminal background with a 2-point `tab_accent` line on the
  edge that touches the terminal. Inactive tabs are separated by short
  `tab_border` dividers.
- **Pill.** Tabs render as 26-point rounded pills centered in the 32-point bar.
  Each of the first nine tabs shows an index badge; the active badge uses
  `tab_accent`. The active pill uses `tab_active_background`.
- **Vertical.** Rows use `tab_active_background` when active, with a 3-point
  `tab_accent` bar inset inside the rounded row.
- **Shared.** The close button appears on hover and on the active tab. Its slot
  stays reserved so hovering never changes tab width. New-tab and overflow
  controls use the icons. Exited tabs show the exited icon and a dimmed title
  instead of the `· exited` suffix. Keep `TabView::title` unchanged for the drag
  preview and other title consumers.

### Fit width

In Fit mode, a horizontal tab's width is its measured title plus fixed chrome
(padding, status slot, close slot, and the Pill index badge), clamped to
`[min_width, max_width]`.

- Measure titles with `window.text_system().shape_line` using the chrome font
  and size. Cache widths by title and style, and invalidate the cache when the
  chrome font, size, or config changes.
- Replace the single `extent` with per-tab start offsets for horizontal strips.
  `slot` searches the offsets; `reveal`, `marker`, `preview`, `max_offset`, and
  autoscroll use them. Vertical strips keep uniform rows.
- Set horizontal `strip.bounds.width` to the smaller of content width and
  viewport. The new-tab button then follows the last tab, and the existing
  fullscreen smoke's click at the bar's right edge still targets it.
- Fill mode keeps today's behavior: an equal share with the existing 120-point
  minimum and scrolling beyond it.

### Top chrome background

The top chrome region is the area above the terminal and tab bar that Huterm
paints itself. It is the macOS titlebar inset when windowed, or the display
safe-area inset above a notch in non-native fullscreen (`ChromeLayout`
`with_safe_area`). Today the window root background fills both, so they always
match the terminal.

Choose the region's background from the same effective tab-bar visibility that
already drives `ChromeLayout` presentation:

| Presentation | Tab bar | Top chrome background |
| --- | --- | --- |
| Reserved | Visible | `tab_bar_background` |
| Reserved | Hidden: one tab, `always_show` false | Terminal background |
| Overlay (fullscreen auto-hide) | Revealed | `tab_bar_background` |
| Overlay (fullscreen auto-hide) | Dismissed | Terminal background |

The color changes when the window gains a second tab or returns to one. For the
overlay, switch while the reveal is in progress so the notch strip and the
sliding bar read as one surface.

Native macOS fullscreen lets AppKit position content inside the safe area, so
Huterm may not own the notch strip there. Record what native fullscreen shows on
a notched display instead of assuming it matches.

## Implementation sequence

1. **Move tab config.** Add `TabsConfig` with validation and the legacy-key
   error. Update the bundled config template, `README.md`,
   `docs/agents/development.md`, schemas, `schemas/fixtures.json`, GPUI config
   tests, and the smoke configs in `scripts/check-fullscreen.ts` and
   `scripts/check-quake.ts`. Run `mise run schema:generate` and the config tests.
2. **Add theme UI colors.** Add the keys, derivation helper, resolved fields, and
   hand-picked values for all bundled themes. Update theme schema, fixtures, and
   the themes README. Test derivation for dark and light themes and `extends`
   merging.
3. **Add the icon pipeline.** Confirm the Lucide license, add the package,
   script, Mise tasks, committed SVGs, notices, and `AssetSource`. Prove that one
   tinted icon renders before continuing.
4. **Restyle the tab bar.** Implement Strip, Pill, and vertical rendering,
   hover close buttons, icon controls, the exited indicator, and the single-tab
   titlebar color. Check both styles at all four placements with Fill width.
5. **Add Fit width.** Implement title measurement, the width cache, variable
   horizontal geometry, and bar bounds. Extend `TabStrip` tests before wiring the
   renderer to the new geometry.
6. **Verify and document.** Run the verification below, take native screenshots,
   and record any durable, non-obvious findings in the narrowest agent guidance.

## Verification

Automated tests:

- **Config.** Defaults, overrides, validation bounds, `min_width > max_width`
  rejection, each legacy key's moved message, and reload of style and width.
- **Themes.** Derived defaults for a dark and a light theme, explicit overrides,
  `extends` inheritance of UI keys, and that every bundled theme defines every UI
  key.
- **`TabStrip` Fit geometry.** Offsets for mixed title widths, min and max
  clamping, `slot` across unequal tabs including pointer positions past either
  end, `reveal` of a wide tab, `marker` and `preview` positions, and bounds that
  shrink to content. Existing Fill tests must keep passing unchanged.
- **Window layout.** Existing placement, reorder, safe-area, and tiny-window
  tests in `windows.rs`, updated only where the config move requires it.
- **Titlebar color.** The chosen background for one tab, two tabs, `always_show`,
  and fullscreen presentations, asserted through the visibility decision rather
  than rendered pixels.

Checks and smokes:

- `mise run check`, including the new `ui-icons:check`, `schema:check`, and
  `check:scripts`.
- `mise run test`.
- `mise run license` after adding the npm package.
- `mise run smoke:linux-fullscreen` for tab visibility, reveal, and the new-tab
  click in both width modes. Run `mise run smoke:macos-fullscreen` on macOS.
- `mise run lint:docs:files` for changed Markdown.

Native evidence: macOS screenshots of Strip and Pill in Fill and Fit, both
horizontal edges, vertical tabs, an exited tab, overflow scrolling, hover close
buttons, and the single-tab titlebar before and after opening a second tab.
Check at least one light bundled theme. Linux screenshots cover the X11 path,
where no titlebar area exists.

## Risks

- GPUI SVG tinting or resvg stroke rendering may not match expectations. Step 3
  proves this before the styles depend on it.
- Title measurement runs from event handlers as well as render. An uncached or
  incorrectly invalidated measurement can cost frame time or leave stale widths
  after a font change.
- Variable offsets change reorder hit testing, the most interaction-heavy tab
  geometry. The `TabStrip` tests must cover unequal widths before the renderer
  switches.
- The config move breaks existing user configs by design; the moved message is
  the only mitigation.

## Unresolved questions

No product decisions block implementation. The derived color mix amounts and
the exact icon names are tuning choices to settle during implementation.
