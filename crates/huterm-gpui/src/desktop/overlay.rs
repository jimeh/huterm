//! Presentation primitives shared by the palette, menus, dialogs, notices,
//! and terminal overlays: theme-derived colours, key caps, footer hints,
//! raised panels, and the scrim.

use gpui::{Div, Hsla, Pixels, SharedString, div, prelude::*, px};
pub(crate) use huterm_ui::tokens::{OverlayColors, Tokens as Swatch};

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
        .bg(swatch.footer())
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
        .text_color(swatch.on(fill))
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

#[cfg(test)]
mod contrast {
    use gpui::Hsla;
    use huterm_ui::tokens::{NON_TEXT_CONTRAST, TEXT_CONTRAST, contrast, over};

    use super::Swatch;

    use super::tokens;
    use crate::config::Theme;

    /// Every bundled theme, and the built-in default.
    fn bundled() -> Vec<(String, Theme)> {
        let directory = std::path::Path::new("");
        crate::themes::names(None)
            .into_iter()
            .map(|name| {
                let theme = if name == "huterm-dark" {
                    Theme::huterm_dark()
                } else {
                    crate::themes::preview(&name, directory)
                        .unwrap_or_else(|| panic!("{name} resolves"))
                };
                (name, theme)
            })
            .collect()
    }

    /// Every enabled text color with each fill it is drawn on, and the
    /// focus ring with the surfaces it is drawn against, for one theme.
    fn drawn_pairs(t: &Swatch) -> Vec<(&'static str, Hsla, Hsla, f32)> {
        let selection = [over(t.selection, t.bg), over(t.selection, t.surface)];
        let plain = [t.bg, t.surface];
        let chips = [over(t.chip, t.bg), over(t.chip, t.surface)];
        let footers = [over(t.footer(), t.bg), over(t.footer(), t.surface)];
        let primary = [t.accent, t.accent_pressed()];
        let danger = [t.danger_fill(), t.danger_hover(), t.danger_pressed()];
        let close = t.on(t.danger_fill());
        let band = over(t.accent.opacity(0.05), t.surface);
        let mut pairs = Vec::new();
        let mut text = |name, color: Hsla, unders: &[Hsla]| {
            for under in unders {
                pairs.push((name, color, *under, TEXT_CONTRAST));
            }
        };
        text("fg", t.fg, &plain);
        text("fg on selection", t.fg, &selection);
        text("muted", t.muted, &plain);
        text("muted on chips", t.muted, &chips);
        text("muted on footers", t.muted, &footers);
        text("muted on selection", t.muted_on(t.selection), &selection);
        text("on_accent", t.on_accent(), &primary);
        text("on warning", t.on(t.warning), &[t.warning]);
        text("on danger", t.on(t.danger), &[t.danger]);
        text("close button", close, &danger);
        for (name, color) in [
            ("accent text", t.accent_text()),
            ("warning text", t.warning_text()),
            ("danger text", t.danger_text()),
        ] {
            text(name, color, &plain);
            text(name, color, &chips);
        }
        text("error band", t.accent_text(), &[band]);
        let ring = t.focus_ring()[0].color;
        for under in plain {
            pairs.push(("focus ring", ring, under, NON_TEXT_CONTRAST));
        }
        pairs
    }

    #[test]
    fn bundled_themes_meet_the_contrast_minimums() {
        let themes = bundled();
        assert!(themes.len() > 10, "{} themes", themes.len());
        let mut failures = Vec::new();
        for (name, theme) in themes {
            let t = tokens(&theme);
            for (pair, color, under, minimum) in drawn_pairs(&t) {
                // Translucent text shows composited over what it sits on.
                let ratio = contrast(over(color, under), under);
                if ratio < minimum {
                    failures.push(format!("{name} {pair}: {ratio:.2}:1"));
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
