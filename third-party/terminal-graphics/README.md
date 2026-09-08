# Terminal graphics provenance

Huterm's `crates/huterm-gpui/src/renderer/builtin` adapts Ghostty's Block
Elements, Box Drawing, Powerline, and Geometric Shapes implementations under
the MIT license. The full
upstream copyright and license notice is in `LICENSE-Ghostty` alongside this
file, and accompanies the packaged application.

Reference repository: <https://github.com/ghostty-org/ghostty>

Reference commit: `448062571c5edf010b7490d06869b88b5ebf8f80`

Source files under `src/font/sprite/draw`:

- `block.zig`: codepoints, block fractions, quadrant masks, and shade levels.
- `box.zig`: codepoints, mixed-weight/double-line junction extents, dash
  placement, diagonal overshoot, and rounded-corner curves.

- `powerline.zig`: triangular, rounded, diagonal, and split separators.
- `geometric_shapes.zig`: filled and outlined corner triangles.

Huterm ports these rules to Rust and paints through GPUI. It uses its own
font-size-based stroke thickness, clips geometry to cells, clamps corner radii
for small cells, and caches geometry independently of font glyph layouts.
Shades use foreground opacity over the displayed cell background. No Ghostty
rasterizer, font stack, or additional dependency is included.

Coverage is U+2500–U+259F, U+E0B0–U+E0BF, U+E0D2, U+E0D4,
U+25E2–U+25E5, U+25F8–U+25FA, and U+25FF for standalone Unicode scalars.
Rounded outlines use concentric arcs; triangle outlines use inset edges,
with a solid fallback when the cell is too small for a hollow center.
Combining sequences and all other characters retain font rendering.
The terminal engines, text selection, and copied Unicode text are unchanged.

Ghostty's other sprite groups are a reference for future coverage, not an
implicit promise to override their entire Unicode blocks. Add each supported
range explicitly and verify against the Unicode character charts:
<https://www.unicode.org/charts/>.
