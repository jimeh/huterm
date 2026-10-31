# Pane sync and zoom indicators

Status: in progress.

Replaces the indicator part of [pane input sync](pane-input-sync.md) (#353).

## Outcome

A tab that syncs input, or shows a zoomed pane, says so through one shape
drawn around its panes. That shape follows the window's rounded corners and
never doubles up where panes meet. The tab bar marks a zoomed tab the way it
marks a syncing one. Each state's look is configurable: its style,
thickness, strength, and pattern, or no shape at all with the marks kept.

Jim picked approaches A to E from the mockup
(`/tmp/huterm-indicators-mockup/mockup.html`, approach A is the default) on
2026-10-31. He rejected the background tint (F).

## Decisions

### Config

Two tables under `[panes]`, each with the same four keys and its own defaults:

```toml
[panes.sync_indicator]
style = "outline"   # outline, dividers, glow, edge, title_bar, or off
width = 2           # 1 to 6 points
opacity = 0.9       # 0.1 to 1
pattern = "dashed"  # solid, dashed, or dotted

[panes.zoom_indicator]
style = "outline"
width = 2
opacity = 0.8
pattern = "solid"
```

- Both tables are separate structs (`SyncIndicatorConfig` and
  `ZoomIndicatorConfig`) with identical field types. The tables need their
  own defaults, so that a partial table still fills from the right ones, and
  their own Settings labels and group. The structs share an
  `IndicatorStyle` and `LinePattern` enum and convert to one plain
  `IndicatorLook { style, width, opacity, pattern }` for the client.
- `width` is an integer from 1 to 6. `opacity` is an `f32` from 0.1 to 1.
  `PanesConfig::validate` rejects values outside those ranges with the key
  path, as `focus_accent` does.
- `off` draws no shape, but the tab bar and title bar marks stay.
- Settings: two new groups in the Panes section, "Input sync indicator" and
  "Zoom indicator", ordered after the existing Panes rows and before Pane
  layouts. Each group has style, width, opacity, and pattern rows. `width`
  shows unless the style is `off` or `title_bar`. `pattern` shows for
  `outline`, `dividers`, and `edge`. `opacity` shows unless the style is
  `off`. Descriptions are one sentence with inline code in backticks.
  Everything applies live.

### Which state draws, and in which colour

- The sync look applies while the tab syncs and its focused pane is live,
  the same condition under which #353 draws frames today
  (`input_sync::receives` for the focused pane).
- The zoom look applies while this window shows a zoomed pane
  (`shown_zoom`).
- Both at once: typing reaches only the zoomed pane, so sync has no visible
  effect. The zoom look wins. The tab bar shows both marks, and the sync
  mark, in the tab and in the zoomed pane's title bar, draws at 40%
  opacity.
- Sync uses the theme's tab accent. Zoom uses the theme's ANSI magenta.
  `TabColors` gains a `zoom` colour.

### Corner geometry

One pure function decides each corner's radius of the pane area, the
`layout.terminal` bounds `pane_elements` draws into. Pane bounds and
`ChromeLayout::content` share window coordinates; compare edges with a small
tolerance.

- Inputs: the window's content bounds, the pane area's bounds, the window's
  top and bottom corner radii, and the radius of Huterm's own rounded
  terminal corner beside a vertical tab bar (`terminal_corner`) when that
  patch draws: `Presentation::Reserved`, a titlebar above the column, and no
  flush active Strip row.
- A pane area corner that coincides with a window corner takes that window
  corner's radius. The corner Huterm rounds beside a vertical tab bar takes
  that radius. Every other corner is square. In practice only a macOS
  windowed bottom corner coincides with a window corner: the titlebar strip
  always sits above the terminal.
- Window radii are zero while `chrome_hidden()` holds (native and
  non-native fullscreen, and a Quake presentation), on Linux with server
  decorations, and wherever a Linux client frame is not `decorated()`. A
  Quake window shown as a regular window keeps its rounded corners. A Linux
  client frame rounds only its top corners (`FRAME_RADIUS`), and its title
  row keeps panes away from them.
- macOS radius source: `NSWindow`'s private `_topCornerRadius` and
  `_bottomCornerRadius`, guarded by `respondsToSelector:`, read once when the
  window opens, in `native_titlebar.rs`, for every window. The read sits
  outside the traffic-light block's `Titlebar && !quake` gate and runs before
  a Quake attach strips the style. On macOS 27 both report 16 points for
  Huterm's window style; this was probed on 2026-10-31. When the selectors
  are missing, a version table answers: 16 points from macOS 26, and 10
  before that. A window without an AppKit handle, as in GPUI tests, is its
  own source and logs nothing; tests set the radius through a `#[cfg(test)]`
  override beside `decorations_override`.
- Smoke state publishes the window's resolved radii and their source
  (`appkit`, `table`, or `none`).

### Drawing order

Two existing elements would paint over the indicator, so the order changes:

- `pane_elements` pushes the divider lines after the marks today, and the
  vertical tab bar's corner patch is drawn after `pane_elements`, from
  `flush_active`, which needs the tab strip computed later in render.
- The indicator and the focus accent therefore draw after the divider lines
  and after the corner patch, and before every overlay that already draws
  above the panes: the revealed fullscreen tab bar, pane drag previews, grab
  badges, toasts, menus, and dialogs. Render computes `flush_active` and the
  strip before the indicator needs them.
- The indicator carries no listeners, cursor, hover style, or `occlude`, so
  GPUI gives it no hitbox; divider grab zones and every pane keep their
  input. The inactive dim stays inside each pane, below the indicator.

### Styles

All shapes draw inside the pane area. They use the look's opacity and
colour.

- `outline`: one stroked path inset by half its width, with the corner
  radii above less that inset. Under the sync look, every divider of the
  tab also draws at the same width and pattern, centred on its 1-point line,
  so every edge has one line. The zoom look has no dividers to draw, since
  one pane shows.
- `dividers`: the dividers alone, at the look's width and pattern. A zoomed
  tab has none, so the zoom look draws nothing, and only the marks show.
- `glow`: a soft band inside the outline path, about `width × 4` points
  deep and tuned against the mockup, drawn as stacked 1-point strokes whose
  opacity falls off inward. Under the sync look, each divider glows on both
  sides the same way.
- `edge`: one bar `width` points thick along the top of the pane area, as
  the mockup shows for every tab bar position. Its ends follow the top
  corners' radii, which are non-zero only at Huterm's own vertical tab bar
  corner.
- `title_bar`: no lines. Each receiving pane's title bar, or the zoomed
  pane's, is tinted with the colour at the look's opacity, scaled to 25%.
  Where a pane's bar is not on screen (hidden, or hover before it reveals),
  a small state chip with the state's mark sits at the pane's title-bar
  edge, inset from its trailing corner and clear of the focus accent. The
  chip has no listeners or tooltip. "Chip" keeps it apart from the existing
  grab badges.

Lines never overlap. GPUI blends overlapping path coverage, so an overlap
would draw darker. Each divider stops at the outline's inner edge, and a
divider that meets another at a T-junction stops at that divider's edge.

The divider rule under sync covers every divider of the tab, including one
beside an exited pane. An exited pane receives nothing and still loses its
title-bar mark, as in #353, but a shape that skips segments would bring
back broken lines.

### Patterns and paths

- `dashed`: dashes of `3 × width` with gaps of `2 × width`, through
  `PathBuilder::dash_array`. A closed outline starts on the middle of a
  straight edge so the seam never lands on a corner. A line shorter than
  one dash and one gap draws solid.
- `dotted`: round dots of diameter `width` every `3 × width`. GPUI does not
  expose lyon's `LineCap`, so Huterm places filled circles itself along the
  same straight segments and arcs, in one fill path per chunk.
- Each built path stays under GPUI's `u16` vertex index limit: long dotted
  or dashed outlines split into several paths. A path whose build fails is
  skipped and logged once.
- Stroke edges snap to physical pixels at the window's scale, as the
  rendering guide requires for built-in graphics.
- Paths are tessellated once per geometry and look, not on every paint. The
  view keeps a single-slot cache keyed by pane area bounds, divider lines,
  corner radii, scale, and look; the colour stays out of the key because
  `paint_path` takes it separately. Paint moves clones of the cached paths
  into a `canvas`.

### Focus accent

The #342 focus cue uses the tab accent, as the sync look does, so it must
stay distinguishable. Where the cue's edge or ends meet a line the active
look draws (outline, edge, or a divider), the cue is inset by the look's
width, so a strip of terminal background separates them. A window test
asserts the cue's bounds.

### Marks

- The tab bar gains a zoom mark (`Icon::Maximize`, in the zoom colour)
  before the sync mark, for a tab whose pane this window shows zoomed.
  `fit_tab_width` counts both marks.
- Pane title bars keep the sync mark, dimmed under both states, and their
  existing zoom button.

### What the window records

`FocusMarks::synced` becomes an `indicator` record of what the frame
painted: the look (state and style), the pane-area corner radii, the cache
key of the painted paths, the divider segments drawn, the panes that
receive synced input (whatever the style), and the panes with a tinted
title bar or a chip. Window tests read it; smoke state reads the radii and
their source.

### Config details

- Enums use `rename_all = "snake_case"`, and every variant has an
  `x-huterm-label`.
- Settings orders 41 to 48 follow the existing Panes rows (31 to 40) and
  precede Pane layouts (50).
- `width` is a slider with `x-huterm-step = 1` and `x-huterm-unit = "pt"`.
  `opacity` is a slider with step 0.05.
- Visibility: under sync, `width` shows for `outline`, `dividers`, `glow`,
  and `edge`; `pattern` for `outline`, `dividers`, and `edge`; `opacity` for
  everything but `off`. Under zoom, `dividers` draws nothing, so it shows no
  rows.

### Docs

Update `docs/configuration.md` (the new tables), `docs/keybindings.md`
(the `toggle_pane_input_sync` row describes the dashed frame),
`docs/agents/desktop-input.md` (the sync frame paragraph), README if it
describes the frame, and mark `pane-input-sync.md`'s indicator section as
replaced by this plan.

## Non-goals

- The background tint style (F).
- Per-state colour settings. The colours come from the theme.
- Changing who receives synced input, or zoom behaviour.
- An indicator for a tab that is not active.

## Failure paths and risks

- A private AppKit selector disappears: the version table takes over, and a
  log line names the fallback once per window.
- A window's fullscreen or Quake state changes: the radii come from the
  current `chrome_hidden()` and frame state on every frame, not from the
  value read at open.
- Thick lines cover chrome inside the pane area. At 6 points the outline
  covers part of a top title bar's close button and the scrollbar thumb's
  edge. This is visual only and accepted; the defaults use 2 points.
- Tessellation cost: the cache limits tessellation to geometry or look
  changes. A divider drag changes geometry every frame while it lasts, which
  is accepted. A rendering benchmark with the indicator on checks the
  steady state.

## Verification

- Pure functions, unit tests:
  - Corner radii: windowed macOS with top, titlebar, bottom, left, and right
    tab bars, and a hidden tab bar; fullscreen and Quake (`chrome_hidden`);
    a regular Quake window; Linux server and client decorations; the
    vertical tab bar's inner corner with and without a flush active row.
  - The look resolver: sync, zoom, both, an exited focused pane, and `off`.
  - Divider trimming at the outline and at T-junctions leaves no overlapping
    segments.
  - Dash arrays and dot placement per width; the short-line fallback; the
    dash seam on a straight edge.
  - Path chunking keeps a dotted width-1 outline around a 4K-sized pane
    area under the vertex limit.
  - The macOS version table.
  - `fit_tab_width` with the zoom mark and with both marks.
- Config: defaults, partial tables keep their own table's defaults, every
  enum value, and out-of-range width and opacity. The Settings model loads
  the new rows with their visibility conditions, and the coverage test
  passes.
- Window tests (tier 2), migrating the five `marks.synced` assertions:
  - The record follows sync on and off, zoom in and out, sync with zoom, the
    focused pane exiting, a pane moved out, and a style change by config
    reload (asserting the cache key, so a stale cache fails).
  - The tab bar marks follow zoom and sync, with the dimmed sync mark under
    both.
  - The focus cue's bounds are inset under a line-drawing sync look.
  - `title_bar` style under hover bars shows the chip before the bar
    reveals and the tint after.
  - A left tab bar with a titlebar and a non-flush active tab records the
    inner corner's radius.
- Smoke: an existing macOS smoke asserts radius source `appkit` and a
  radius above 0 for a windowed window, and the fullscreen smoke asserts
  zero radii while fullscreen.
- `mise run schema:generate` updates the committed schemas, and
  `schema:check` passes.
- A `bench:*` rendering run with the sync indicator on, compared with off.
- `mise run check`, focused tests, and `mise run verify`.
- UX review in a scratch app, compared with the mockup's approach A
  screenshot: each style, for sync, zoom, and both, windowed with rounded
  corners and in fullscreen, in dark and light themes.
- Before and after screenshots from a scratch app, cropped.

## Open questions

None. Jim settled the styles, the defaults, the zoom colour, and the
both-states rule on 2026-10-31.
