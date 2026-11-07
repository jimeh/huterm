# Nerd Font constraint provenance

Huterm sizes and aligns each Nerd Font icon from its bundled symbols font by
porting Ghostty's Nerd Font glyph constraints, under the MIT license. The full
upstream copyright and license notice is in `LICENSE-Ghostty` alongside this
file, and accompanies the packaged application.

Reference repository: <https://github.com/ghostty-org/ghostty>

Reference commit: `83fb95624b0439ee660c2679b43b38c8b63769cd`, the same
revision Huterm builds its VT engine from (`scripts/ghostty-source.json`).

Ported from Ghostty:

- `src/font/Glyph.zig`: `RenderOptions.Constraint`, its sizing, alignment,
  padding, and scale-group rules, to
  `crates/huterm-gpui/src/renderer/symbols/constraint.rs`, and its
  `Constraints` test cases for fit, scale groups, and stretch, adapted to
  Huterm's stretch fallback, to that module's tests.
- `src/font/face/coretext.zig` and `freetype.zig`: measuring a glyph against
  the cell baseline before constraining it and centering the face's advance
  in the rounded cell afterwards, to `renderer/symbols.rs`.
- `src/font/nerd_font_codegen.py`: reading the Nerd Fonts font-patcher's
  patch sets and scale groups, measuring each icon, and mapping the
  patcher's stretch rules to constraints, to `scripts/nerd-font-constraints.py`.
- `src/font/nerd_font_tables.zig`: generated constraints per code point,
  which the script writes as `renderer/symbols/constraint/table.rs`.

Huterm's changes:

- The code is Rust, and the table is a sorted list of code point ranges
  rather than Ghostty's three-level lookup.
- GPUI scales glyphs uniformly, so `stretch` rules size like `fit` against
  the grid, and an aspect-ratio limit shrinks both axes.
- A cell's icon spans two columns only when the cell is wide. Ghostty also
  widens an icon into a following space; patched "Nerd Font Mono" families,
  which Huterm matches, keep icons to one cell.
- Plain `center` alignment is omitted because no patcher rule uses it.
- A scale group whose glyphs have different advances still scales by the
  group's combined width, as font-patcher does, through an added
  `relative_scale_width`. Ghostty scales each such glyph by its own width,
  which drew narrow Font Awesome and Weather Icons glyphs up to twice as
  large as patched fonts do.
- The generator targets Nerd Fonts 3.5.1, pins the font-patcher file by
  SHA-256, and writes no intermediate code point module.

The rules in the generated table come from the Nerd Fonts font-patcher,
copyright (c) 2014 Ryan L McIntyre, under MIT; its notice is
`third-party/nerd-fonts/LICENSE`. No Ghostty rasterizer, font stack, or
additional runtime dependency is included.
