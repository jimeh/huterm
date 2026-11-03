# Bundled terminal font

Status: in progress. Tracks #370.

## Outcome

Huterm embeds DejaVu Sans Mono 2.37 and registers it with GPUI at startup, so
terminal text draws in a monospaced font on Linux without any font package
installed. Jim's decisions, 2026-11-03:

- Bundle the four DejaVu Sans Mono 2.37 faces (Book, Bold, Oblique, Bold
  Oblique) unmodified. The Bitstream Vera license forbids modified files under
  the same name. A few MB of app and AppImage size is acceptable.
- Linux's default `[font] family` becomes `DejaVu Sans Mono`. macOS keeps
  `Menlo`.
- A configured family that does not resolve draws in the platform default:
  DejaVu Sans Mono on Linux and Menlo on macOS. The startup warning and the
  Settings note name the font that draws.
- `family = "monospace"` draws the platform default on both platforms until
  #368 resolves fontconfig's alias properly. Linux configuration files that
  earlier versions generated contain `family = "monospace"`, so this fixes
  them too. The alias is not reported as "not installed".
- The interface's monospace family (`Typography::platform_mono`) becomes
  DejaVu Sans Mono on Linux, matching the terminal default.

## Which copy draws

Read from GPUI 0.3.10, not reproduced:

- macOS: `load_family` searches the in-memory source before the system
  source (`gpui-pre-apple-0.3.10/src/text_system.rs`), so the bundled faces
  draw even when the system has DejaVu Sans Mono installed.
- Linux: `add_fonts` appends the bundled faces to the cosmic-text `fontdb`
  database after the system scan. `load_family` takes every face with the
  family name, and font-kit's `find_best_match` keeps the first of equally
  good candidates, so for each requested face an installed copy with the same
  properties draws, and the bundled faces cover any the system lacks. Debian,
  Ubuntu, Fedora, and Arch package DejaVu 2.37; whether their files are
  byte-identical was not checked.
- Settings lists each family once: `TextSystem::all_font_names` sorts and
  deduplicates on both platforms.

## Approach

- **Assets.** The four TTFs live under `crates/huterm-gpui/assets/fonts/dejavu/`
  and are embedded with `include_bytes!`, as the Lucide icons are. Embedding
  avoids resolving resource paths per package format, and GPUI's macOS
  `add_fonts` takes `Cow::Borrowed` static data without copying.
  `third-party/dejavu/` holds the upstream `LICENSE` and a `README.md` with
  provenance. A new `third-party/dejavu/provenance.json`, added to
  `provenancePaths` in `scripts/licenses.ts`, pins the SHA-256 of `LICENSE`
  and the four TTFs (by relative path) to the `dejavu-fonts-ttf-2.37`
  release, so `licenses:generate` and `licenses:check` verify them. The
  component's notice uses an ordinary repository path, not `packaged:`.
- **Registration.** One function in `terminal_font.rs` registers a list of
  bundled faces through `cx.text_system().add_fonts`. Every
  `assets::application().run` site calls it before anything resolves a font:
  the desktop bootstrap, the renderer smoke and benchmark, the Quake and menu
  smokes, and the gallery. A shared helper is preferred over six copies.
  Linux GPUI's `add_fonts` returns `Ok` even when `fontdb` rejects the bytes,
  so success is the postcondition that `all_font_names` lists the bundled
  family afterwards. A failure is logged once and startup continues. The face
  list is the extension point for #371, which will add a symbols font.
- **Resolution.** `resolve_metrics` resolves the configured family to the
  drawn family and a reason: installed, the `monospace` alias, or not
  installed. The fallback for the alias and for a missing family is the
  platform default family, which must equal `Config::default().font.family`.
  The warning prints only for a missing family and names the drawn font.
  `note_terminal_font` and the Settings scan record the reason, so the
  Settings note can tell the alias from a missing family.
- **Weights.** `enumerate_weights` must see the bundled faces:
  - macOS: when the family is the bundled one, enumerate the embedded faces,
    since GPUI draws them in preference to any installed copy.
  - Linux: add the embedded faces to the lazily built `fontdb` database after
    `load_system_fonts`, which mirrors GPUI's database.
  DejaVu Sans Mono then offers Regular and Bold.
- **Settings copy.**
  - Missing family: "X is not installed, so terminals use DejaVu Sans Mono."
    on Linux, and "...use Menlo." on macOS.
  - Alias: "monospace is a font alias, so terminals use Menlo." on macOS,
    and "...use DejaVu Sans Mono." on Linux.
  - The weight row's alias-disabled state and its note are removed. With the
    alias resolved, the weight applies and the row enumerates the drawn
    family.
- **Configuration.** The default in `huterm-config`, its doc comment, the
  regenerated schema, the generated default document, `docs/configuration.md`
  (defaults, fallback, alias, which copy draws), and the renderer smoke and
  benchmark families all follow the new Linux default.
- **Licenses and packaging.** Add a `components.json` entry, `kind` `Font`,
  for all four targets, and regenerate `licenses.json`. Ship
  `third-party/dejavu` in the macOS bundle resources with `mise.toml` bundle
  assertions, and its notices in `assets/linux/package-policy.json`, as
  Lucide's are. The Linux package contract stays as it is: no dynamic
  fontconfig or FreeType.
- **Guides.** Record the bump procedure in `docs/agents/dependencies.md` and
  the font registration rule in `docs/agents/desktop-client.md`, where they
  fit.

## Failure paths

- A registration failure leaves the default unresolved. The terminal then
  draws through GPUI's own fallback stack, as it does today. The startup
  warning and the Settings note must not claim DejaVu Sans Mono draws when it
  is not registered: they name the configured family as unavailable without
  naming a font, or say the bundled font failed to load.
- Settings' family scan on Linux runs once. The bundled family is registered
  before the first window, so the first scan lists it.
- A user-installed DejaVu Sans Mono that differs from 2.37 draws on Linux.
  This is accepted and documented.
- Release-note impact: Linux terminals switch from GPUI's fallback stack to
  DejaVu Sans Mono.

## Non-goals

- Resolving fontconfig's `monospace` alias (#368).
- Nerd Font symbols or any fallback chain (#371).
- Changing macOS's default family or interface font.
- Loading fonts from resource files at runtime.
- A Linux system with no interface font. GPUI maps `.SystemUIFont` to IBM
  Plex Sans and falls back through Ubuntu, Adwaita Sans, Cantarell, Noto
  Sans, and DejaVu Sans; with none of them installed it panics, before and
  after this change. Desktop distributions install at least one.

## Verification

| Concern | Evidence |
| --- | --- |
| Embedded faces are DejaVu Sans Mono, Book and Bold, upright and oblique | A unit test parses the embedded bytes with font-kit on both platforms |
| Registration makes all four faces resolve without any system copy | A real text-system test: `TestAppContext::build_with_text_system` with Linux's `CosmicTextSystem::new_without_system_fonts` on Linux CI resolves regular, bold, oblique, and bold oblique to the bundled faces. On macOS, the host has no DejaVu Sans Mono; use the native text backend directly if it can be built off the main thread, else a native smoke. `TestAppContext::build` has a no-op text system and proves nothing about registration |
| Rejected bytes are reported | The registration postcondition test with invalid bytes, on the Linux real text system |
| Missing family and `monospace` resolve to the platform default with the right reason, and an unregistered default does not claim DejaVu | `resolve_metrics` policy tests, extending `unavailable_terminal_family_keeps_its_configured_weight` |
| The platform default matches the configuration default | An assertion against `Config::default()` |
| The Settings family note distinguishes alias from missing | Settings tier 2 tests for both notes |
| The weight row enables for `monospace` and lists Regular and Bold for DejaVu | Replace `linux_monospace_alias_disables_weight_*`; enumeration test for the bundled family on both platforms |
| Linux defaults follow | Update `a_control_shows_the_file_value_else_the_platform_default` in `desktop/settings/values.rs` and the default-document test. `scripts/check-presentation-queries.ts` keeps `monospace` on Linux as alias coverage, with a comment saying so |
| Licenses, provenance, and schema are current | `mise run license`, `licenses:check`, `schema:check` |
| macOS bundle ships the notices | `mise run package:macos` assertions |
| Linux packages ship the notices and keep the FreeType and fontconfig ban | CI Linux package jobs and smokes. These install `fonts-dejavu-core`, so they do not prove the bundled copy draws |
| Rendering on macOS unchanged; Linux draws an even grid without system DejaVu | `smoke:renderer` on macOS. A Docker Xvfb screenshot of the default configuration in a throwaway container with `fonts-dejavu-core` removed |
| Settings notes in the running app | UX review on a macOS scratch bundle |
| Whole repository | `mise run check`, `mise run test`, then `mise run verify` |

## Open questions

None.
