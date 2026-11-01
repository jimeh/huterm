# Terminal font weight and initial window size

Status: implemented in PR #367. "Revisions during review" below records
where the shipped change departs from the decisions above, chiefly that
Linux leaves the `monospace` alias as is.

## Outcome

Two settings Jim asked for on 2026-11-01:

1. `[font] weight` sets the weight of the terminal font, for families that
   ship more than one weight. Bold terminal text follows it.
2. `[window] initial_columns` and `initial_rows` set the grid size new
   windows open with.

Jim chose every recommendation below on 2026-11-01: terminal only, bold a
step heavier, names and numbers, and the `initial_*` keys with the listed
scope and limits. A Codex plan review on the same day added the font
resolution, enumeration, and window-clamp details.

## Decisions

### Font weight: config

```toml
[font]
weight = "regular" # thin, extra_light, light, regular, medium, semibold,
                   # bold, extra_bold, black, or 100 through 900 in steps of 100
```

- Applies to the terminal font only. The interface keeps its own normal
  and semibold hierarchy; `ui_weight` is a possible later extension.
- Accepts the nine names above or an integer from 100 through 900 that is
  a multiple of 100. Names map to 100 through 900 in order. Any other
  string or number, including a float, is a configuration error that
  names `font.weight` and the accepted values. Default `regular` (400), so
  today's rendering is unchanged.
- `huterm-config` stores a dependency-neutral weight (for example a
  `FontWeight(u16)` newtype) in `FontConfig`; `crates/huterm-gpui/src/config.rs`
  carries it through the raw-to-runtime conversion.
- Bold text (SGR 1) uses `max(700, weight + 300)`, capped at 900: 100
  through 400 give 700, 500 gives 800, and 600 and above give 900. Keep it
  as one small function so a later `bold_weight` override is local.
- Applies live on reload, like `family` and `size`
  (`x-huterm-apply = "live"`).

### Font weight: rendering

- Introduce one value for the terminal's font, for example
  `TerminalFont { family, weight, bold_weight }`, and pass it wherever the
  terminal font family travels today: `resolve_metrics`,
  `GridMetrics::resolve`, `TerminalRenderer::new` and `reconfigure`,
  `text_run`, `TerminalView` (`font_family`), `WorkspaceView::apply_reload`
  (`windows.rs` near 11164), `TerminalView::apply_theme`, the
  display-scale path in `resize_to_layout`, `open_window_with_profile`,
  and the renderer bench and smoke (`renderer/bench.rs`,
  `renderer/smoke.rs`), which use a fixed regular weight. Never set the
  terminal weight on a view root where interface overlays would inherit
  it.
- `GridMetrics::resolve` measures the configured weight's face, since a
  family's weights can differ in advance or vertical metrics.
- A weight-only change, even one with identical metrics and an unchanged
  snapshot, must invalidate prepared glyph layouts and update the PTY's
  cell pixel size exactly as a family change does.
- The weight must reach the family the terminal actually draws. GPUI's own
  fallback stack (`TextSystem::resolve_font_uncached`) builds its fallback
  fonts at the default weight, so Huterm must hand GPUI a family that
  loads directly:
  - A configured family that is not installed already falls back to
    Menlo or `monospace` in `resolve_metrics`; the weight applies to that
    family.
  - Linux `monospace` is a fontconfig alias that GPUI's Linux text system
    does not resolve (`load_family` matches face family names exactly), so
    today it lands in GPUI's fallback stack and loses any weight. Resolve
    `monospace` to fontconfig's preferred monospace family through
    `zed-font-kit` (`SystemSource::select_best_match` with
    `FamilyName::Monospace`) and pass that concrete family to GPUI. If that
    resolution fails, keep today's behavior. First confirm the current
    behavior in the Linux container with a focused test, and record the
    evidence; if `monospace` already loads directly, skip this step and
    say so.
- GPUI on both platforms picks the nearest face by font-kit's
  `find_best_match`. Huterm does not synthesize weights.

### Settings: weight row

- A new `Special::FontWeight` override for `font.weight`, in the Font
  group directly after Font size (order between `size` 20 and `ui_family`
  22). The schema's string-or-integer type makes the setting
  `SettingKind::Compound`, so the override owns everything the generic
  select assumes about string choices: the displayed value (a number the
  file holds shows as its name), menu highlighting, value checking before
  a write, keyboard opening, and the disabled state.
- Writes use names. A number the file holds stays a number until the user
  picks a different weight; picking the weight it already has writes
  nothing.
- Options come from an enumeration of the applied terminal family's faces
  through `zed-font-kit` (add it as a macOS dependency at the pinned
  `=0.14.1-zed`; Linux already has it), mirroring GPUI's candidate set:
  skip faces GPUI skips (no `m` glyph; on macOS, faces whose weight,
  width, or symbolic traits are not numbers, which GPUI checks before
  `properties()` because `properties()` panics on them). Keep each face's
  full properties (style, weight, stretch).
- For each of the nine requests (100 through 900, normal style and
  stretch), run `find_best_match` over those properties. Round each
  matched face's native weight to the nearest hundred, clamped to 100
  through 900. The options are the distinct rounded weights, labeled by
  name and number, such as "Light (300)". Settings never writes a weight
  outside the nine values. A family with only italic faces still gets
  options this way.
- When the matched face for the configured weight rounds to a different
  weight, the select also shows the configured value and a note under
  the row names the weight that draws, for example: "Menlo has no Light
  weight, so terminals use Regular."
- A family with one option shows a disabled select, following the
  component's existing disabled convention, and the note
  "Monaco has one weight."
- Enumeration runs on the background executor, keyed by family and a
  generation. A completion for an older family or generation is dropped.
  An open weight menu closes when its options change. It runs when the
  window opens, comes forward, or follows a reload (the existing
  `scan_fonts` moments), and when the applied terminal family changes.
- Until enumeration finishes, the select shows the configured value alone
  and stays usable. Enumeration failure, including a panic inside
  fontconfig loading (its dlopen accessor unwraps) or a face's
  properties, is caught with `catch_unwind` on the background thread,
  logged once per family, and leaves the select listing all nine weights
  without a note. A face that fails to load is skipped.
- On macOS, GPUI caches a family's faces once it loads them, so a weight
  installed while Huterm runs may appear in Settings before the terminal
  can draw it. Accepted; documented as needing a restart, like fonts
  installed on Linux today.
- Reset works as for every other row and removes `weight`.

### Initial window size

```toml
[window]
initial_columns = 100 # 10 through 1000
initial_rows = 32     # 3 through 1000
```

- Applies to the first window at launch and every new window (menu,
  shortcut, Dock or application reopen). Not to Quake windows, whose
  profiles size them. No window restore exists yet; when it lands, a
  restored window's saved geometry should win.
- Padding, an always-shown tab bar, the title row, a lone pane's visible
  title bar, and client-side decorations add to the grid, as
  `initial_window_size` does today, so the grid comes out exactly
  `initial_columns` by `initial_rows` when the display has room.
- Clamp to the display: `Bounds::centered` already clamps the requested
  size to the display's visible bounds. The post-creation `window.resize`
  that corrects for scale and decorations does not, and must also be
  limited to the window's display visible bounds (`window.display(cx)`),
  less the native title bar on macOS. On macOS visible bounds are the
  work area. On Linux GPUI reports the whole X screen, so the window
  manager does the rest; per-monitor work-area clamping on Linux is a
  non-goal. A missing display skips the clamp. The corrective resize does
  not recenter, as today.
- A clamped window gets a smaller grid; `window_min_size` still applies to
  tiny grids.
- The shell's first grid: `shell_command` and a new `TerminalView`'s
  initial `requested_grid` use the configured values instead of the
  constants, and the view resizes the PTY to the grid it lays out, as it
  does today when the two differ (clamped windows, later tabs in a
  resized window, Quake windows). Splits keep overriding the grid with
  their layout. Benchmarks keep fixed constants so results stay
  comparable.
- New windows are never fullscreen. There is no cascading today; this
  change adds none.
- Reload changes only windows opened afterwards. A new Settings apply kind,
  `new_windows`, shows "Applies to new windows".
- Defaults equal today's constants (100 by 32). Values outside the limits
  are configuration errors that name the key.

### Revisions during review

- Linux leaves the `monospace` alias as is (Jim, 2026-11-01). Two attempts
  to resolve it to a concrete family failed review: font-kit's fontconfig
  source loads libfontconfig, and with it a dynamic FreeType, which the
  Linux package smoke rejects; `fontdb`'s parsed alias keeps only the last
  fontconfig alias (FreeMono on stock Debian and Ubuntu configs). With the
  default `monospace`, Linux draws exactly as before this change, and the
  weight applies once `family` names an installed font. Settings disables
  the weight select for the alias and says so. Proper Linux `monospace`
  resolution is follow-up work.
- Linux Settings weight enumeration uses `fontdb`, the pure-Rust database
  GPUI's Linux text system uses, built lazily on the background executor;
  macOS keeps CoreText through font-kit. This also removes the
  fontconfig-versus-fontdb mismatch accepted above.
- A new tab in a window that already shows a terminal starts at that
  window's active view grid; only a window's first terminal starts at the
  configured initial grid.

## Scope

- `huterm-config`: `RawFont::weight`, `FontConfig::weight`, the weight
  parser and bold derivation, `WindowConfig::initial_columns` and
  `initial_rows`, validation, schema metadata, the `new_windows` apply
  kind, and `schemas/fixtures.json` cases. Regenerate both schema files
  with `mise run schema:generate`.
- `huterm-gpui`: the `TerminalFont` plumbing at every site listed above,
  `initial_window_size` and the
  post-creation clamp, `shell_command` and `TerminalView` initial grids,
  the Settings weight row and its enumeration, and the default config
  template (it lists `[font]` and `[window]`).
- Dependencies: `zed-font-kit` for macOS and `fontdb` for Linux in
  `crates/huterm-gpui/Cargo.toml`; run `mise run license` and `mise run licenses:generate`.
- Docs: `docs/configuration.md` (Fonts and themes, and an initial window
  size passage near the window settings) and the desktop client guide's
  rendering or configuration rules where this change teaches something
  non-obvious (the fallback stack drops weight; Settings mirrors GPUI's
  candidate filter).
- One PR, squash title naming both features.

## Non-goals

- An interface font weight, a separate bold weight, and variable-font
  weight axes (GPUI matches static faces only).
- Synthetic weights.
- Window cascading, remembered window sizes, and session restore.
- Per-monitor work-area clamping on Linux.
- Changing Quake window sizing.
- Resizing open windows on reload.

## Failure paths and risks

- Weight missing from the family: nearest face, Settings note; no error.
- Family missing: existing fallback. On macOS the weight applies to Menlo;
  on Linux the fallback is the `monospace` alias, which draws at its
  default weight.
- Enumeration panic or failure: caught, logged, nine plain options.
- Out-of-order enumeration: dropped by family and generation.
- Requested window larger than the display: clamped, smaller grid.
- Weight-only reload with identical metrics: still invalidates glyphs.

## Verification

| Concern | Evidence |
| --- | --- |
| Weight names, numbers, default, and rejected values (350, 0, 1000, 1.5, "heavy") with the key named | `huterm-config` unit tests |
| Bold derivation for 100, 400, 500, 600, 900 | unit test |
| Option list and note from face properties: non-hundred native weights (450), mixed stretch, italic-only family, one weight, missing weight lighter and heavier | unit tests over property lists, independent of installed fonts |
| Enumeration failure and stale completion | unit or tier 2 tests with a test seam for the enumerator, like `test_families` |
| Initial columns and rows: parse, default, limits with the key named | `huterm-config` unit tests |
| Schema and Settings metadata agree with parsing | `schemas/fixtures.json` cases, `mise run schema:check`, the settings coverage test |
| Window size from cells, chrome, and padding; clamp to a visible area; minimum size | unit tests on `initial_window_size` and the clamp helper |
| Weight-only reload invalidates prepared glyphs with unchanged metrics and snapshot; PTY cell pixel size follows a metrics change without a grid change | renderer unit tests next to the cache-eviction test in `renderer/tests.rs`, and a view or tier 2 test for the PTY cell size |
| Settings weight row: numeric value shows as its name, menu highlight, a pick writes a name, the same pick writes nothing, missing-weight note, one-weight disabled state, reset | tier 2 Settings tests in the `window_tests/settings_lists.rs` style |
| Linux `monospace` alias in Settings | a tier 2 test that the weight select is disabled with its note and enumerates nothing |
| Real rendering on macOS | `mise run smoke:renderer`, extended to draw a non-regular weight if cheap; then a running-app check with a scratch config: a multi-weight family at Light and at Medium, bold text visibly heavier, and the Settings row |
| Real window size on macOS | running-app check: `initial_columns = 120`, `initial_rows = 40`; the size indicator or `stty size` in the first window and a new window reads 120x40; an oversized value opens within the screen |
| Repository gates | `mise run verify` before handoff; CI |

## Open questions

None. Jim settled the product questions on 2026-11-01.
