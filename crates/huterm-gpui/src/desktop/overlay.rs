//! Presentation primitives shared by the palette, menus, dialogs, notices,
//! and terminal overlays: theme-derived colours, key caps, footer hints,
//! raised panels, and the scrim.

use gpui::{
    AnyView, App, BoxShadow, Context, Div, Hsla, Render, Rgba, SharedString,
    Window, div, hsla, point, prelude::*, px,
};

use super::scrollbar_colors;
use crate::config::Theme;
use crate::renderer::rgb_color as color;
use crate::ui::scrollbar::ScrollbarColors;

/// Theme colours an overlay derives its presentation from.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OverlayColors {
    pub(crate) foreground: Hsla,
    pub(crate) background: Hsla,
    pub(crate) selection: Hsla,
    pub(crate) accent: Hsla,
    /// Error marks, stripes, and the destructive dialog button.
    pub(crate) danger: Hsla,
    /// Warning marks, stripes, badges, and the unknown-state tag.
    pub(crate) warning: Hsla,
    pub(crate) scrollbar: ScrollbarColors,
}

impl OverlayColors {
    pub(crate) fn from_theme(theme: &Theme) -> Self {
        Self {
            foreground: color(theme.foreground),
            background: color(theme.background),
            selection: color(theme.selection),
            accent: color(theme.ansi[4]),
            danger: color(theme.ansi[1]),
            warning: color(theme.ansi[3]),
            scrollbar: scrollbar_colors(theme),
        }
    }
}

/// Derived colours for one overlay presentation.
#[derive(Clone, Copy)]
pub(crate) struct Swatch {
    pub(crate) fg: Hsla,
    pub(crate) bg: Hsla,
    /// The raised panel surface: the background mixed toward the foreground.
    pub(crate) surface: Hsla,
    pub(crate) muted: Hsla,
    pub(crate) dim: Hsla,
    /// Hairline separators and key-cap borders.
    pub(crate) line: Hsla,
    /// Panel borders and outlined buttons.
    pub(crate) line_strong: Hsla,
    /// Hovered rows and buttons.
    pub(crate) hover: Hsla,
    pub(crate) selection: Hsla,
    pub(crate) accent: Hsla,
    pub(crate) danger: Hsla,
    pub(crate) warning: Hsla,
    pub(crate) chip: Hsla,
    pub(crate) scrollbar: ScrollbarColors,
}

impl Swatch {
    pub(crate) fn new(colors: OverlayColors) -> Self {
        Self {
            fg: colors.foreground,
            bg: colors.background,
            surface: mix(colors.background, colors.foreground, 0.07),
            muted: colors.foreground.opacity(0.62),
            dim: colors.foreground.opacity(0.38),
            line: colors.foreground.opacity(0.1),
            line_strong: colors.foreground.opacity(0.22),
            hover: colors.foreground.opacity(0.08),
            selection: colors.selection.opacity(0.35),
            accent: colors.accent,
            danger: colors.danger,
            warning: colors.warning,
            chip: colors.foreground.opacity(0.1),
            scrollbar: colors.scrollbar,
        }
    }

    pub(crate) fn from_theme(theme: &Theme) -> Self {
        Self::new(OverlayColors::from_theme(theme))
    }

    /// Whether the terminal background is light, for shadow strength.
    fn light(&self) -> bool {
        self.bg.l > 0.5
    }

    /// The soft shadow the Linux client-side frame paints in its border:
    /// it must stay within the ten-point inset around the content.
    pub(crate) fn frame_shadow(&self) -> Vec<BoxShadow> {
        let strength = if self.light() { 0.28 } else { 0.55 };
        vec![BoxShadow {
            color: hsla(0.0, 0.0, 0.0, strength),
            offset: point(px(0.0), px(2.0)),
            blur_radius: px(8.0),
            spread_radius: px(0.0),
        }]
    }

    /// The two-layer drop shadow raised surfaces carry.
    pub(crate) fn shadow(&self) -> Vec<BoxShadow> {
        let (far, near) = if self.light() {
            (0.22, 0.15)
        } else {
            (0.45, 0.35)
        };
        vec![
            BoxShadow {
                color: hsla(0.0, 0.0, 0.0, far),
                offset: point(px(0.0), px(12.0)),
                blur_radius: px(32.0),
                spread_radius: px(0.0),
            },
            BoxShadow {
                color: hsla(0.0, 0.0, 0.0, near),
                offset: point(px(0.0), px(2.0)),
                blur_radius: px(6.0),
                spread_radius: px(0.0),
            },
        ]
    }

    /// A 2-point ring around a keyboard-focused control.
    pub(crate) fn focus_ring(&self) -> Vec<BoxShadow> {
        vec![BoxShadow {
            color: self.accent,
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(2.0),
        }]
    }
}

/// Mixes `from` toward `to` by `amount` in sRGB, ignoring alpha.
pub(crate) fn mix(from: Hsla, to: Hsla, amount: f32) -> Hsla {
    let from = Rgba::from(from);
    let to = Rgba::from(to);
    let channel = |from: f32, to: f32| from + (to - from) * amount;
    Hsla::from(Rgba {
        r: channel(from.r, to.r),
        g: channel(from.g, to.g),
        b: channel(from.b, to.b),
        a: 1.0,
    })
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

/// A keystroke chip, as the palette shows shortcuts and footer hints.
pub(crate) fn key_cap(key: &str, swatch: Swatch) -> impl IntoElement {
    div()
        .px(px(5.0))
        .py(px(1.0))
        .rounded(px(4.0))
        .bg(swatch.chip)
        .border_1()
        .border_color(swatch.line)
        .text_size(px(10.5))
        .text_color(swatch.fg)
        .child(key.to_owned())
}

/// The footer strip listing `(key, label)` hints.
pub(crate) fn footer_hints(
    hints: impl IntoIterator<Item = (String, String)>,
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
    div()
        .bg(swatch.surface)
        .border_1()
        .border_color(swatch.line_strong)
        .rounded(px(radius))
        .shadow(swatch.shadow())
        .overflow_hidden()
}

/// The dimming layer beneath a modal panel, filling its parent. It occludes
/// what it covers: GPUI hit-tests every hitbox under the pointer until one
/// blocks, so without this the terminal beneath still receives the presses
/// and wheel events the panel does not stop itself.
pub(crate) fn scrim(swatch: Swatch) -> Div {
    div()
        .absolute()
        .inset_0()
        .occlude()
        .bg(swatch.bg.opacity(0.6))
}

/// A one-line hover tooltip on the raised surface, for controls whose icon
/// or disabled state needs a word of explanation.
pub(crate) struct TextTooltip {
    text: SharedString,
    swatch: Swatch,
}

impl TextTooltip {
    /// The view GPUI's `tooltip` builder expects.
    pub(crate) fn view(
        text: impl Into<SharedString>,
        swatch: Swatch,
        cx: &mut App,
    ) -> AnyView {
        let text = text.into();
        cx.new(|_| Self { text, swatch }).into()
    }
}

impl Render for TextTooltip {
    fn render(
        &mut self,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let swatch = self.swatch;
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(swatch.surface)
            .border_1()
            .border_color(swatch.line_strong)
            .shadow(swatch.shadow())
            .text_size(px(11.5))
            .text_color(swatch.fg)
            .whitespace_nowrap()
            .child(self.text.clone())
    }
}

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
        let dark = Swatch::from_theme(&Theme::default());
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
        let light = Swatch::from_theme(&light);
        assert!(light.light());
        assert!(light.surface.l < light.bg.l, "light themes lower it");
        assert!((light.surface.a - 1.0).abs() < f32::EPSILON);
    }
}
