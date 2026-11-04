# Window transparency and blur

Status: implemented

The blur parts of this plan, the `background_blur` key and GPUI's `Blurred`
appearance, are superseded by
[blur with any transparency and per-window overrides](window-transparency-overrides.md),
which replaces them with a `blur` radius and blur mode.

Issue #39 (parent #16). Adds an overall window opacity, separate background
and text (foreground) opacity, and optional background blur, applied live on
reload.

## Decisions

Jim chose these from mockups on 2026-11-03:

- Chrome follows the background: the tab bar, title row, pane title bars,
  the vertical tab sidebar, the strip-style active tab and Linux's drawn
  client frame take the background opacity. Chrome text, icons and pane
  dividers stay solid. The command palette, dialogs, menus, notices, popovers
  and the Settings window stay opaque.
- Background opacity applies to the theme's default background only,
  including a default background that a program overrode with OSC 11.
  Backgrounds that programs set explicitly (SGR indexed or RGB colors, even
  one equal to the theme background, and inverse video) stay solid, as with
  Ghostty's and kitty's defaults.
- Superseded: see the note at the top. Blur is macOS only. Huterm's GPUI
  build is X11-only and GPUI has no X11
  blur. On Linux, `background_blur` is documented as unsupported, and a load
  that sets it logs one warning. Settings hides the row there, as it hides
  every foreign-platform setting the file does not set
  (`SettingsView::available`); a file that sets it shows the row read-only
  with the existing "macOS only" note. KWin's
  `_KDE_NET_WM_BLUR_BEHIND_REGION` is a follow-up.
- Settings layout S1: Appearance gets a Transparency group, ordered between
  Theme and Padding, with one row per setting.

## Configuration

```toml
[window]
opacity = 1.0             # 0.1 to 1; fades the whole window
background_opacity = 1.0  # 0 to 1
foreground_opacity = 1.0  # 0.1 to 1; text and its decorations
background_blur = false   # macOS only
```

- Effective background alpha is `opacity × background_opacity`. Effective
  text alpha is `opacity × foreground_opacity`.
- All four keys apply live (`x-huterm-apply = "live"`) to every open terminal
  window, including Quake windows, without restarting PTYs.
- The three opacities are unitless sliders with step 0.05, so Settings reads
  them as percentages. Labels: Opacity, Background opacity, Text opacity,
  Background blur. Background blur carries `x-huterm-platform = "macos"`.
  Orders 36 to 39 in group "Transparency".
- An out-of-range or non-finite value is an error that names the key, like
  `panes.inactive_dim`.
- The defaults leave every window exactly as today: Opaque appearance,
  native alpha 1, opaque paint.

## Approach

### Overall opacity: native window alpha

- `opacity` sets the native window alpha: `NSWindow.alphaValue` on macOS and
  `_NET_WM_WINDOW_OPACITY` on X11. Remove the X11 property at 1.0. The
  compositor fades everything evenly, including chrome, frame and shadow,
  with no double blending where layers overlap. Overall opacity alone does
  not require a non-opaque GPUI window.
- Apply the alpha when the window opens, on reload, and after a
  regular-to-Quake conversion or the reverse. AppKit setters run outside
  GPUI update borrows (see `docs/agents/window-presentation.md`).
- One owner writes a window's native alpha: `configured_opacity ×
  fade_factor`, where the Quake fade (`desktop/quake_windows*`,
  `quake/macos.rs`, `quake/x11.rs`) keeps its progress normalized to 0..1.
  Every path that restores alpha today restores `1.0`, including `recover`
  and `cleanup` in `quake_windows.rs`; each restores the configured opacity
  instead. A reload changes the configured factor only. It invalidates
  captured stale effects through the existing generation check without
  resetting animation progress, activation or deadlines.

### Background and text alpha: paint time

- Alpha is applied at paint time, never baked into `PreparedRow`. An opacity
  change must not clear prepared rows or glyph caches, and must not force a
  new snapshot. `TerminalRenderer::reconfigure` keeps its early return for
  unchanged font, theme and metrics; opacity reaches the renderer through a
  separate setter that only triggers a repaint.
- Exactly one painter fills each region's background. Today the
  `WorkspaceView` root or Linux client frame panel (`desktop/windows.rs`),
  the `TerminalView` root (`desktop.rs`) and the renderer's full-grid quad
  (`renderer.rs`) all paint `theme.background` under the grid, which
  compounds to `1-(1-a)^3`. The `TerminalView` root becomes the only
  default-background painter for padding and grid, so a pane draws its
  background before its first snapshot (the renderer paints nothing
  without one). The title row fill under a merged tab bar
  (`windows.rs` title row and tab bar) must not double either.
- Chrome that takes its own layout space follows background alpha: the
  tab bar, title row, sidebar, pinned pane title bars and the Linux client
  frame panel. The strip-style active tab joins the terminal, so the bar
  fill must not paint under it and its composite alpha must equal the
  terminal's. Pill tabs are a raised layer and may compound over the bar.
  Chrome that floats over terminal content stays opaque, like the palette:
  hover-revealed pane title bars, the fullscreen tab reveal, the scroll
  pill, the size indicator, and link popups.
- Unfocused-pane dimming (`dim_layers`): at background alpha 1, keep the
  existing overlays unchanged. Below 1, an overlay would raise the pane's
  alpha (`a + d(1-a)`), so apply `inactive_dim` and
  `inactive_dim_background` as paint-time tints of the default background
  and glyph colors that preserve alpha. Emoji are not dimmed there; say so
  in the docs. The visual bell keeps its overlay; its brief flash may raise
  alpha.
- Default-background cells are distinguished by their `CellColor`, not by
  comparing resolved RGB with the theme:
  - `prepare_backgrounds` paints a run as a solid quad unless its cells are
    `CellColor::DefaultBackground`. Explicit colors equal to the theme
    background, which merge into the default today, become solid runs.
    Update `renderer/tests.rs`'s equality test accordingly.
  - OSC 11 makes the engine emit `CellColor::Rgb(override)` for default
    cells (`huterm-core/src/engine/ghostty.rs`, `Resolver::color`). Keep
    that. Publish the active override in the snapshot as
    `default_background: Option<Rgb>`, next to `cursor_color`. While it is
    set, cells equal to `Rgb(override)` count as default; an explicit SGR
    color equal to the override is the accepted edge case. The default
    background painter uses the override when set, so the padding matches
    the grid, which changes today's theme-colored padding under OSC 11.
    `same_presentation` (`desktop.rs`) compares the new field.
  - Making the default symbolic in core was rejected. Inverse video makes
    `DefaultBackground` a glyph foreground, and prepared glyphs and
    decorations bake resolved colors.
- Text alpha covers font glyphs, built-in box and powerline geometry,
  underlines, strikethrough and the hovered-link underline. Emoji follow it
  if GPUI element opacity can reach `paint_emoji` without a vendor patch.
  Otherwise emoji stay solid, and the docs say so.
- The cursor (including the 0.4 block fill and the hollow outline) and the
  selection highlight stay solid apart from overall opacity. The selection
  foreground stays solid.

### Window background appearance

- One pure function chooses the GPUI `WindowBackgroundAppearance` from the
  platform, granted decorations, Linux transparency support,
  `background_opacity` and blur. Overall opacity is the compositor's
  business and never makes the surface non-opaque.
  - Opaque when `background_opacity` is 1, unless Linux client decorations
    need Transparent (today's `sync_frame` rule, unchanged).
  - On macOS, Blurred when blur is on and `background_opacity` is below 1.
  - Otherwise Transparent.
  - On Linux without transparency support, Opaque (or Transparent for
    client decorations, as today), with background and text paint alpha
    forced to 1 and no native alpha.
- Set it in `WindowOptions.window_background` at open, so the window never
  flashes opaque. Apply it on reload and on decoration changes, and call
  `window.refresh()` after a change: GPUI's macOS and X11 setters do not
  redraw.
- Linux transparency support means both a `_NET_WM_CM_S<screen>` selection
  owner and a 32-bit TrueColor visual on the screen. GPUI picks that visual
  when one exists. It is checked at window open and on reload, so a
  compositor that stops and is followed by a reload falls back. Live
  tracking without a reload is a non-goal. GPUI's wgpu surface can silently
  fall back to an opaque alpha mode, which Huterm cannot observe; that is a
  documented residual risk. Decoration negotiation is unchanged.
- Without transparency support, Huterm logs a warning once per load when
  any opacity is below 1, naming the missing piece. Settings shows a note
  under Opacity and Background opacity, such as "No compositing manager is
  running, so windows stay opaque."
- A non-opaque window disables GPUI's subpixel antialiasing on Linux. Note
  it in the docs.

### Fullscreen and Quake

Transparency and blur stay on in every mode: non-native and native
fullscreen, and Quake windows. Nothing special happens on entry or exit.
Verify that non-native fullscreen's `hasShadow` save and restore and the
native Space transitions still behave.

## Files

- `crates/huterm-config/src/lib.rs`: `WindowConfig` fields, schema metadata,
  validation, tests. Run `mise run schema:generate`; update
  `schemas/fixtures.json` if it needs a case.
- `crates/huterm-gpui/src/renderer.rs` and tests: paint-time alpha, default
  background classification, opacity setter.
- `crates/huterm-gpui/src/desktop.rs`: `TerminalView` background layers,
  bell, link underline.
- `crates/huterm-gpui/src/desktop/windows.rs`, `windows/tab_bar.rs`,
  `windows/client_frame.rs`: chrome alpha, appearance, reload plumbing in
  `apply_reload` and `apply_theme`, native alpha.
- `crates/huterm-gpui/src/desktop/quake_windows*`, `quake/macos.rs`,
  `quake/x11.rs`: fade target.
- Settings: `desktop/windows/settings_window*.rs` for the no-compositor note,
  and the settings coverage test.
- `huterm-core` engine and `huterm-protocol` snapshot: publish the OSC 11
  `default_background` override.
- `crates/huterm-gpui/src/renderer/bench.rs` and the benchmark scripts:
  alpha and appearance inputs.
- Docs: `docs/configuration.md` (new "Transparency" section),
  `docs/agents/window-presentation.md` and `docs/agents/desktop-client.md`
  for any non-obvious rule learned, and README's feature list if it claims
  transparency as missing.

## Observable success

- At the defaults, windows look and measure exactly as before.
- At background 60%, the desktop shows through the terminal and chrome
  backgrounds while text, cursor, selection and program-set backgrounds stay
  solid.
- At opacity 70%, the whole window fades evenly. Combined values multiply.
- With blur on, macOS blurs what shows through.
- A config save updates every open window, Quake windows included, without
  restarting a shell or losing scrollback.
- On Linux without a compositor, windows stay opaque, Settings says why, and
  the log has a warning.

## Failure paths and risks

| Risk | Handling |
| --- | --- |
| Stacked background layers compound alpha | One painter per pixel; renderer and window tests assert it |
| Prepared rows bake colors, so opacity goes stale or rows rebuild | Paint-time alpha; a test asserts that rows survive an opacity change |
| A Quake fade or recovery overwrites the configured opacity | One alpha owner, `configured × fade`; policy tests |
| A reload during a Quake animation | Change the configured factor only; generation check drops stale effects |
| OSC 11 change suppressed by `same_presentation` | Compare `default_background`; view-level test |
| Dimming raises a translucent pane's alpha | Alpha-preserving tints below 1; window test of composed alpha |
| GPUI's macOS and X11 appearance setters don't redraw | Explicit `refresh()` |
| No compositor or no ARGB visual on X11 leaves undefined alpha | Keep Opaque, force alpha 1, warn |
| wgpu falls back to an opaque alpha mode silently | Residual risk, documented |
| Blur on Linux claims success silently | Settings note, docs, and a log warning |
| Box-drawing overlaps double-blend at text alpha below 1 | Accept and document; it is visible only at reduced text opacity |
| Render or scroll regression at the defaults | Opaque path unchanged; benchmarks before and after |

## Non-goals

- A blur radius, and Linux blur (follow-up issue).
- Per-theme or per-profile opacity, and opacity for the Settings window.
- Live tracking of an X11 compositor that starts or stops.
- An option that applies background opacity to program-set backgrounds; it
  can be added later.
- Wayland: GPUI is built X11-only.

## Verification

| Concern | Evidence |
| --- | --- |
| Ranges, defaults, errors naming the key | `huterm-config` unit tests |
| Schema, Settings rows, coverage | `mise run schema:check`, settings coverage test, `settings_model` tests |
| Composition (`opacity ×` component) | Unit test of the effective-alpha helper |
| Default vs program-set background, OSC 11 override and reset (OSC 111), inverse, SGR equal to the theme background | Renderer tests; a core test that the snapshot publishes and clears `default_background` |
| OSC 11 change with every row `Arc` retained redraws | View-level test through `apply_snapshot` |
| Text, decoration and link alpha; cursor and selection solid | Renderer paint tests |
| Opacity change keeps prepared rows; theme reload still refreshes colors | Renderer tests |
| One background painter per region, including before the first snapshot, merged title row, strip active tab, chrome alpha, floating chrome opaque | Window tests of the drawn scene or element colors |
| Dimming keeps composed alpha below 1 and is unchanged at 1 | Window tests |
| Appearance choice per platform, decorations, transparency support, blur; overall opacity alone stays Opaque | Unit tests of the pure function |
| Live reload of appearance and native alpha, both directions | Window test, plus smoke state that reads native alpha back (`alphaValue`, `_NET_WM_WINDOW_OPACITY`) and the applied appearance, asserted after reloads to and from transparency in an existing macOS and Linux smoke |
| Quake fade, recovery, cleanup and reload mid-animation | Policy tests; `smoke:macos-quake` and `smoke:linux-quake` read native alpha back |
| Blur row: hidden on Linux when unset, read-only when set | Settings test |
| Fullscreen unaffected | `smoke:macos-fullscreen`; `smoke:linux-fullscreen` in Docker |
| Renderer paint | `smoke:renderer` |
| No meaningful scroll or render regression | Add background and text alpha inputs (and the window appearance) to the renderer benchmarks, record them in reports, and drop the opaque fixture underlay for transparent runs. Run `bench:renderer-scenarios` compare, `bench:scroll` and `bench:renderer` at the defaults and at background 0.8 against the budgets in `docs/agents/desktop-smokes.md#benchmarks` |
| Native evidence | macOS screenshots: light and dark themes, solid text over a clear background, a uniform fade, combined values, and blur. Linux screenshots under `xcompmgr`, plus the no-compositor fallback. Crop them; no private desktop content |
| Whole gate | `mise run check`, `mise run test`, `mise run verify` |

## Open questions

None.
