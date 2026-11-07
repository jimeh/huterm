//! Nerd Font icons drawn from the bundled Symbols Nerd Font Mono when the
//! terminal font has no glyph for them. GPUI has no per-font fallback on
//! Linux, so the renderer chooses the face itself rather than relying on it.

use gpui::{Font, FontId, Pixels, Point, TextSystem, font, point};

use super::GridMetrics;
use crate::terminal_font::SYMBOLS_FAMILY;

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

/// The font size that fits the font's one-em-square glyphs in one cell.
pub(super) fn font_size(metrics: GridMetrics) -> Pixels {
    metrics.cell_width.min(metrics.cell_height)
}

/// The glyph origin, relative to the cell origin, that centres a symbol of
/// `advance` and the given ascent and descent in a span of `columns` cells,
/// in whole device pixels. Descent is a distance below the baseline, of
/// either sign.
pub(super) fn origin(
    metrics: GridMetrics,
    columns: u16,
    advance: Pixels,
    ascent: Pixels,
    descent: Pixels,
) -> Point<Pixels> {
    let scale = metrics.scale_factor;
    let align = |value: Pixels| (value * scale).round() / scale;
    let height = ascent + descent.abs();
    point(
        align((metrics.cell_width * f32::from(columns) - advance) / 2.0),
        align((metrics.cell_height - height) / 2.0 + ascent),
    )
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

    #[test]
    fn symbols_fill_the_cell_width_and_centre_vertically() {
        // Menlo 12 at 2x: a cell narrower than it is tall.
        let metrics = GridMetrics::from_measurements(
            px(12.0),
            px(7.224_609_4),
            px(11.138_672),
            px(-2.830_078_1),
            2.0,
        );
        let size = font_size(metrics);
        assert_eq!(size, metrics.cell_width);
        let ascent = size * 0.8;
        let descent = size * 0.2;
        let origin = origin(metrics, 1, size, ascent, -descent);
        assert_eq!(origin.x, px(0.0));
        // Equal space above the ascent and below the descent, within the
        // half device pixel that rounding may move it.
        let above = origin.y - ascent;
        let below = metrics.cell_height - origin.y - descent;
        assert!((above - below).abs() <= px(0.5), "{above:?} {below:?}");
        assert_eq!(origin.y * 2.0, (origin.y * 2.0).round());
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
                assert_eq!(advance, size, "Mono glyphs are one em wide");
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

    #[test]
    fn symbols_in_a_short_wide_cell_take_the_cell_height() {
        let metrics = GridMetrics::fixed(20.0, 10.0);
        let size = font_size(metrics);
        assert_eq!(size, metrics.cell_height);
        let origin = origin(metrics, 1, size, size * 0.8, size * 0.2);
        assert_eq!(origin.x, px(5.0));
        assert_eq!(origin.y, size * 0.8);
    }

    #[test]
    fn wide_symbols_centre_on_whole_device_pixels() {
        // An odd device-pixel cell width at 1x puts the two-column centre
        // on a half pixel unless the shifted origin is aligned again.
        let metrics = GridMetrics::fixed(7.0, 14.0);
        let size = font_size(metrics);
        let narrow = origin(metrics, 1, size, size * 0.8, size * 0.2);
        let wide = origin(metrics, 2, size, size * 0.8, size * 0.2);
        assert_eq!(narrow.x, px(0.0));
        assert_eq!(wide.x, wide.x.round());
        assert!((wide.x - px(3.5)).abs() <= px(0.5));
        assert_eq!(wide.y, narrow.y);
    }
}
