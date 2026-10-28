//! Presentation primitives shared by the palette, menus, dialogs, notices,
//! and terminal overlays: theme-derived colours, key caps, footer hints,
//! raised panels, and the scrim.

use gpui::{Div, Hsla, Pixels, SharedString, div, prelude::*, px};
pub(crate) use huterm_ui::tokens::{OverlayColors, Tokens as Swatch, mix};

use super::key_hint::KeyHint;
use super::scrollbar_colors;
use crate::config::Theme;
use crate::renderer::rgb_color as color;

/// The overlay colors `theme` supplies.
pub(crate) fn overlay_colors(theme: &Theme) -> OverlayColors {
    OverlayColors {
        foreground: color(theme.foreground),
        background: color(theme.background),
        selection: color(theme.selection),
        accent: color(theme.ansi[4]),
        danger: color(theme.ansi[1]),
        warning: color(theme.ansi[3]),
        scrollbar: scrollbar_colors(theme),
    }
}

/// The derived tokens for `theme`. `Swatch` is passed by value and must stay
/// under Clippy's 256-byte limit, so derive further shades through methods
/// on `huterm_ui::tokens::Tokens` rather than fields.
pub(crate) fn tokens(theme: &Theme) -> Swatch {
    Swatch::new(overlay_colors(theme))
}

/// The monospace family for process names, locations, and grid sizes,
/// matching the default terminal font on each platform.
pub(crate) fn mono_font_family() -> SharedString {
    if cfg!(target_os = "macos") {
        "Menlo".into()
    } else {
        "monospace".into()
    }
}

/// A 3-point accent bar with rounded ends, inset from the left edge of a
/// rounded surface so it never crosses its border or corners: the active
/// tab, a foreground process, and a notice's severity use it.
pub(crate) fn accent_bar(color: Hsla, left: Pixels, inset: Pixels) -> Div {
    div()
        .absolute()
        .left(left)
        .top(inset)
        .bottom(inset)
        .w(px(3.0))
        .rounded_full()
        .bg(color)
}

/// A keystroke chip, as the palette shows shortcuts and footer hints.
pub(crate) fn key_cap(hint: &KeyHint, swatch: Swatch) -> Div {
    huterm_ui::components::kbd::cap(&hint.glyphs(), swatch)
}

/// A keystroke chip on a filled button, drawn in the button's text colour
/// `on`.
pub(crate) fn key_cap_on(hint: &KeyHint, on: Hsla) -> Div {
    huterm_ui::components::kbd::cap_on(&hint.glyphs(), on)
}

/// The footer strip listing `(key, label)` hints.
pub(crate) fn footer_hints(
    hints: impl IntoIterator<Item = (KeyHint, String)>,
    swatch: Swatch,
) -> Div {
    let mut footer = div()
        .flex()
        .items_center()
        .gap(px(14.0))
        .px(px(12.0))
        .py(px(6.0))
        .border_t_1()
        .border_color(swatch.line)
        .bg(swatch.fg.opacity(0.03))
        .text_size(px(11.5))
        .text_color(swatch.muted);
    for (key, label) in hints {
        footer = footer.child(
            div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .child(key_cap(&key, swatch))
                .child(label),
        );
    }
    footer
}

/// A raised surface over terminal output: the mixed surface colour, a
/// one-point border, the theme shadow, and a `radius` corner.
pub(crate) fn raised_panel(swatch: Swatch, radius: f32) -> Div {
    huterm_ui::components::surface::surface(swatch, radius)
}

/// The dimming layer beneath a modal panel, filling its parent. It occludes
/// what it covers: GPUI hit-tests every hitbox under the pointer until one
/// blocks, so without this the terminal beneath still receives the presses
/// and wheel events the panel does not stop itself.
pub(crate) fn scrim(swatch: Swatch) -> Div {
    div().absolute().inset_0().occlude().bg(swatch.scrim())
}

/// A one-line hover tooltip on the raised surface, for controls whose icon
/// or disabled state needs a word of explanation.
pub(crate) use huterm_ui::components::tooltip::Tooltip as TextTooltip;

/// A severity mark: a filled circle with a single glyph.
pub(crate) fn severity_mark(glyph: &str, fill: Hsla, swatch: Swatch) -> Div {
    div()
        .flex_none()
        .w(px(18.0))
        .h(px(18.0))
        .rounded_full()
        .bg(fill)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(swatch.bg)
        .child(glyph.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_moves_the_background_toward_the_foreground() {
        let dark = tokens(&Theme::default());
        assert!(dark.surface.l > dark.bg.l, "dark themes raise the surface");
        let light = Theme {
            background: huterm_protocol::Rgb {
                red: 0xf0,
                green: 0xf0,
                blue: 0xf0,
            },
            foreground: huterm_protocol::Rgb {
                red: 0x20,
                green: 0x20,
                blue: 0x20,
            },
            ..Theme::default()
        };
        let light = tokens(&light);
        assert!(light.is_light());
        assert!(light.surface.l < light.bg.l, "light themes lower it");
        assert!((light.surface.a - 1.0).abs() < f32::EPSILON);
    }
}
