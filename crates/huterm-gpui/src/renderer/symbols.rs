//! Nerd Font icons drawn from the bundled Symbols Nerd Font when the
//! terminal font has no glyph for them. GPUI has no per-font fallback on
//! Linux, so the renderer chooses the face itself rather than relying on it.
//! Each icon is sized and aligned in its cell by the Nerd Fonts patcher's
//! rule for it, as a patched "Nerd Font Mono" family would draw it.
//!
//! The placement follows Ghostty's glyph rendering in src/font/face, copyright
//! (c) 2024 Mitchell Hashimoto, Ghostty contributors, under MIT. See
//! third-party/nerd-font-constraints/README.md.

mod constraint;

use gpui::{Font, FontId, Pixels, Point, TextSystem, font, point, px};
use skrifa::instance::{LocationRef, Size as FontSize};
use skrifa::{FontRef, MetadataProvider as _};

use self::constraint::{Constraint, GlyphBox, Metrics, Size};
use super::GridMetrics;
use crate::terminal_font::{SYMBOLS_FACE, SYMBOLS_FAMILY, TerminalFont};

/// Scalars the symbols font may draw: the Private Use Areas, where Nerd Fonts
/// places its icons, and the IEC power symbols. Standard characters the font
/// also covers, such as U+2665 and U+26A1, keep the terminal font and the
/// platform's fallback, including emoji presentation.
pub(super) fn candidate(ch: char) -> bool {
    matches!(
        ch,
        '\u{e000}'..='\u{f8ff}'
            | '\u{f0000}'..='\u{ffffd}'
            | '\u{23fb}'..='\u{23fe}'
            | '\u{2b58}'
    )
}

/// The symbols face, when it should draw `ch` instead of `configured`: the
/// configured face has no glyph for it and the symbols face has one. A
/// patched Nerd Font therefore keeps its own icons.
pub(super) fn face_for(
    text_system: &TextSystem,
    configured: &Font,
    ch: char,
    font_size: Pixels,
) -> Option<(Font, FontId)> {
    if !candidate(ch) || !crate::terminal_font::symbols_available() {
        return None;
    }
    let has_glyph = |id: FontId| text_system.advance(id, font_size, ch).is_ok();
    if has_glyph(text_system.resolve_font(configured)) {
        return None;
    }
    let symbols = font(SYMBOLS_FAMILY);
    let id = text_system.resolve_font(&symbols);
    // resolve_font substitutes GPUI's fallback stack for a family that did
    // not load; only the registered face itself may draw icons.
    let loaded = text_system
        .get_font_for_id(id)
        .is_some_and(|face| face.family == symbols.family);
    (loaded && has_glyph(id)).then_some((symbols, id))
}

/// The bundled symbols face, for glyph bounds and its character map. GPUI's
/// Linux text system reports advances, not ink bounds.
fn face() -> Option<FontRef<'static>> {
    FontRef::new(SYMBOLS_FACE).ok()
}

/// A glyph's ink box in font units, and the font's units per em.
fn glyph_bounds(face: &FontRef<'_>, ch: char) -> Option<(GlyphBox, f64)> {
    let glyph = face.charmap().map(ch)?;
    let bounds = face
        .glyph_metrics(FontSize::unscaled(), LocationRef::default())
        .bounds(glyph)?;
    let units_per_em = face
        .metrics(FontSize::unscaled(), LocationRef::default())
        .units_per_em;
    let ink = GlyphBox {
        width: f64::from(bounds.x_max - bounds.x_min),
        height: f64::from(bounds.y_max - bounds.y_min),
        x: f64::from(bounds.x_min),
        y: f64::from(bounds.y_min),
    };
    Some((ink, f64::from(units_per_em)))
}

/// `ch`'s ink box at `font_size`, in logical pixels with y up from the
/// baseline and x from the pen position.
pub(super) fn ink_box(ch: char, font_size: Pixels) -> Option<GlyphBox> {
    let (ink, units_per_em) = glyph_bounds(&face()?, ch)?;
    let scale = f64::from(f32::from(font_size)) / units_per_em;
    let ink = GlyphBox {
        width: ink.width * scale,
        height: ink.height * scale,
        x: ink.x * scale,
        y: ink.y * scale,
    };
    (ink.width > 0.0 && ink.height > 0.0).then_some(ink)
}

/// The regular terminal face's cap height, which one-cell icons scale
/// against. GPUI reports zero when the font's OS/2 table predates the field,
/// as the bundled Linux default's does. Like Ghostty, measure the `H` glyph
/// instead,
/// and estimate 75% of the ascent when that fails too.
pub(super) fn cap_height(
    text_system: &TextSystem,
    terminal_font: &TerminalFont,
    metrics: GridMetrics,
) -> Pixels {
    let id = text_system.resolve_font(&terminal_font.face(false));
    let reported = text_system.cap_height(id, metrics.font_size);
    if reported > px(0.0) {
        return reported;
    }
    measured_cap_height(
        text_system,
        id,
        &terminal_font.family,
        metrics.font_size,
    )
    .filter(|height| *height > px(0.0))
    .unwrap_or(metrics.ascent * 0.75)
}

/// `H`'s ink height, from CoreText's glyph bounds.
#[cfg(target_os = "macos")]
fn measured_cap_height(
    text_system: &TextSystem,
    id: FontId,
    _family: &str,
    font_size: Pixels,
) -> Option<Pixels> {
    text_system
        .typographic_bounds(id, font_size, 'H')
        .ok()
        .map(|bounds| bounds.size.height)
}

/// `H`'s ink height in the bundled family, whose bytes Huterm embeds. GPUI's
/// Linux text system reports advances rather than ink bounds, so other
/// families fall back to an estimate.
#[cfg(not(target_os = "macos"))]
fn measured_cap_height(
    _text_system: &TextSystem,
    _id: FontId,
    family: &str,
    font_size: Pixels,
) -> Option<Pixels> {
    if family != crate::terminal_font::BUNDLED_FAMILY {
        return None;
    }
    let face =
        FontRef::new(crate::terminal_font::bundled_regular_face()).ok()?;
    let (ink, units_per_em) = glyph_bounds(&face, 'H')?;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a cap height in ems fits f32"
    )]
    Some(font_size * (ink.height / units_per_em) as f32)
}

/// The grid measurements the patcher's rules size icons against.
/// `cap_height` is the regular terminal face's.
pub(super) fn grid(metrics: GridMetrics, cap_height: Pixels) -> Metrics {
    let value = |pixels: Pixels| f64::from(f32::from(pixels));
    let face_height = value(metrics.ascent + metrics.descent);
    let cell_baseline = value(metrics.cell_height - metrics.baseline);
    Metrics {
        cell_width: value(metrics.cell_width),
        cell_height: value(metrics.cell_height),
        face_width: value(metrics.advance),
        face_height,
        face_y: cell_baseline - value(metrics.descent),
        icon_height: face_height,
        icon_height_single: (2.0 * value(cap_height) + face_height) / 3.0,
    }
}

/// Where a symbol draws: the size to lay it out at and its glyph origin,
/// the pen position on the baseline, relative to the cell's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Placement {
    pub(super) font_size: Pixels,
    pub(super) origin: Point<Pixels>,
}

/// Places `ch` in `columns` cells by its patcher rule, with the origin on
/// whole device pixels. `None` for a glyph without ink.
#[expect(
    clippy::cast_possible_truncation,
    reason = "cell-sized logical pixels and scale factors fit f32"
)]
pub(super) fn place(
    ch: char,
    metrics: GridMetrics,
    grid: &Metrics,
    columns: u8,
) -> Option<Placement> {
    let ink = ink_box(ch, metrics.font_size)?;
    let cell_baseline =
        grid.cell_height - f64::from(f32::from(metrics.baseline));
    let glyph = GlyphBox {
        y: ink.y + cell_baseline,
        ..ink
    };
    let constraint = Constraint::for_char(ch);
    let placed = constraint.constrain(glyph, grid, columns);
    let factor = placed.width / glyph.width;
    // Like Ghostty, centre the face's advance in the rounded cell, except
    // for stretch rules, which already align to the grid.
    let x = if constraint.size == Size::Stretch {
        placed.x
    } else {
        placed.x + (grid.cell_width - grid.face_width) / 2.0
    };
    let scale = f64::from(metrics.scale_factor);
    let align = |value: f64| px(((value * scale).round() / scale) as f32);
    Some(Placement {
        font_size: metrics.font_size * factor as f32,
        origin: point(
            align(x - ink.x * factor),
            align(grid.cell_height - placed.y + ink.y * factor),
        ),
    })
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::*;

    #[test]
    fn candidates_are_private_use_and_power_symbols_only() {
        for ch in [
            '\u{e000}',
            '\u{e0a0}',
            '\u{f07b}',
            '\u{f115}',
            '\u{f8ff}',
            '\u{f0001}',
            '\u{f1af0}',
            '\u{ffffd}',
            '\u{23fb}',
            '\u{23fe}',
            '\u{2b58}',
        ] {
            assert!(candidate(ch), "{:04X}", u32::from(ch));
        }
        for ch in [
            'a',
            '\u{2665}',
            '\u{26a1}',
            '\u{276c}',
            '\u{2630}',
            '\u{23fa}',
            '\u{23ff}',
            '\u{f900}',
            '\u{effff}',
            '\u{100000}',
        ] {
            assert!(!candidate(ch), "{:04X}", u32::from(ch));
        }
    }

    /// Registers the bundled fonts in the platform's native text system, as
    /// the application does, and checks which face draws each scalar.
    #[cfg(all(
        feature = "test-support",
        any(target_os = "macos", target_os = "linux")
    ))]
    #[test]
    fn bundled_symbols_draw_only_icons_the_terminal_font_lacks() {
        #[cfg(target_os = "macos")]
        let (platform, terminal) = (
            std::sync::Arc::new(gpui_apple::AppleTextSystem::new()),
            "Menlo",
        );
        #[cfg(target_os = "linux")]
        let (platform, terminal) = {
            let family = crate::terminal_font::BUNDLED_FAMILY;
            let platform =
                gpui_wgpu::CosmicTextSystem::new_without_system_fonts(family);
            (std::sync::Arc::new(platform), family)
        };
        let cx = gpui::TestAppContext::build_with_text_system(
            gpui::TestDispatcher::new(0),
            None,
            platform,
        );
        cx.update(|cx| {
            crate::terminal_font::register_bundled_fonts(cx);
            let text_system = cx.text_system();
            let size = px(14.0);
            let configured = font(terminal);
            for ch in [
                '\u{e0a0}',
                '\u{f07b}',
                '\u{f115}',
                '\u{f0001}',
                '\u{f1af0}',
                '\u{23fb}',
                '\u{2b58}',
            ] {
                let (face, id) = face_for(text_system, &configured, ch, size)
                    .unwrap_or_else(|| {
                        panic!("U+{:04X} must draw from symbols", u32::from(ch))
                    });
                assert_eq!(face.family.as_ref(), SYMBOLS_FAMILY);
                let advance = text_system.advance(id, size, ch).unwrap().width;
                assert!(advance > px(0.0));
            }
            // A family with its own glyph keeps it; the symbols family stands
            // in for a patched Nerd Font.
            let patched = font(SYMBOLS_FAMILY);
            assert!(
                face_for(text_system, &patched, '\u{f07b}', size).is_none()
            );
            // Text, standard symbols, and code points the symbols font lacks
            // stay on the terminal font path.
            for ch in ['a', '\u{2665}', '\u{f8ff}', '\u{e0d8}'] {
                assert!(
                    face_for(text_system, &configured, ch, size).is_none(),
                    "U+{:04X}",
                    u32::from(ch)
                );
            }
        });
    }

    /// Menlo 12 at 2x, with its cap height: a cell narrower than tall.
    fn menlo() -> (GridMetrics, Metrics) {
        let metrics = GridMetrics::from_measurements(
            px(12.0),
            px(7.224_609_4),
            px(11.138_672),
            px(-2.830_078_1),
            2.0,
        );
        (metrics, grid(metrics, px(8.748)))
    }

    /// The placed ink's edges relative to the cell's top-left corner:
    /// left, top, right, bottom.
    fn placed_ink(ch: char, at: Placement) -> (f64, f64, f64, f64) {
        let ink = ink_box(ch, at.font_size).expect("ink");
        let x = f64::from(f32::from(at.origin.x)) + ink.x;
        let baseline = f64::from(f32::from(at.origin.y));
        (
            x,
            baseline - ink.y - ink.height,
            x + ink.width,
            baseline - ink.y,
        )
    }

    #[test]
    fn tall_icons_take_the_line_height_like_a_patched_font() {
        let (metrics, grid) = menlo();
        let cell_height = f64::from(f32::from(metrics.cell_height));
        let cell_width = f64::from(f32::from(metrics.cell_width));
        // The Powerline branch scales to the full line height.
        let at = place('\u{e0a0}', metrics, &grid, 1).expect("branch");
        let (_, top, _, bottom) = placed_ink('\u{e0a0}', at);
        assert!(
            (bottom - top - grid.face_height).abs() < 0.5,
            "branch spans {top}..{bottom} of a {} line",
            grid.face_height
        );
        // A narrow Material battery grows taller than a cell is wide, which
        // fitting the em square to the cell width never allowed.
        let at = place('\u{f0084}', metrics, &grid, 1).expect("battery");
        let (left, top, right, bottom) = placed_ink('\u{f0084}', at);
        assert!(
            bottom - top > cell_width,
            "battery is {} tall",
            bottom - top
        );
        assert!(right - left < cell_width + 0.5 && bottom <= cell_height + 0.5);
        // A wide folder fills the width and stays short.
        let at = place('\u{f07b}', metrics, &grid, 1).expect("folder");
        let (left, _, right, _) = placed_ink('\u{f07b}', at);
        assert!((right - left - grid.face_width).abs() < 0.5);
    }

    #[test]
    fn every_icon_stays_inside_its_cells() {
        let (metrics, grid) = menlo();
        let cell_width = f64::from(f32::from(metrics.cell_width));
        let cell_height = f64::from(f32::from(metrics.cell_height));
        // The patcher's overlap pads let a few glyphs reach 3% of a cell
        // past each edge, and origins round by up to half a device pixel.
        let slack = 0.03 * grid.face_width + 0.5;
        let mut code_points = Vec::new();
        let face = face().expect("bundled face");
        code_points.extend(face.charmap().mappings().map(|(code, _)| code));
        let mut placed = 0;
        for ch in code_points.into_iter().filter_map(char::from_u32) {
            for columns in [1_u8, 2] {
                let Some(at) = place(ch, metrics, &grid, columns) else {
                    continue;
                };
                let span = cell_width * f64::from(columns);
                let (left, top, right, bottom) = placed_ink(ch, at);
                assert!(
                    left >= -slack
                        && top >= -slack
                        && right <= span + slack
                        && bottom <= cell_height + slack,
                    "U+{:04X} in {columns} cells spans ({left}, {top})..({right}, {bottom})",
                    u32::from(ch)
                );
                placed += 1;
            }
        }
        assert!(placed > 20_000, "placed {placed} glyphs");
    }
}

#[cfg(test)]
mod patched_reference;
