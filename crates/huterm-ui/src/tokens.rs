//! Colors and sizes every component draws with.
//!
//! [`OverlayColors`] is the crate's color input: the few theme colors an
//! owner supplies. [`Tokens`] derives every shade components use from them.
//! Themes are arbitrary, so the derivation corrects shades that would miss
//! WCAG's minimums: text reaches 4.5:1 (secondary text and text on an accent
//! fill included), and the focus ring reaches 3:1 against the surfaces it
//! sits on. Disabled text is exempt, as WCAG exempts inactive controls, and
//! so are the borders of controls whose text labels identify them.
//! Further shades are methods rather than fields, because [`Tokens`] is passed
//! by value and must stay under Clippy's 256-byte `large_types_passed_by_value`
//! limit: its thirteen colors and [`ScrollbarColors`] take 240 bytes.

use gpui::{BoxShadow, Hsla, Rgba, hsla, point, px};

use crate::scrollbar::ScrollbarColors;

/// The colors a component's presentation derives from.
#[derive(Clone, Copy, Debug)]
pub struct OverlayColors {
    /// Text and icons.
    pub foreground: Hsla,
    /// The surface beneath everything.
    pub background: Hsla,
    /// Selected text and rows.
    pub selection: Hsla,
    /// Focus rings, selected items, and primary buttons.
    pub accent: Hsla,
    /// Error marks, stripes, and destructive buttons.
    pub danger: Hsla,
    /// Warning marks, stripes, badges, and the unknown-state tag.
    pub warning: Hsla,
    /// Scrollbar thumbs and tracks.
    pub scrollbar: ScrollbarColors,
}

/// Derived colors for one presentation.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    /// Text and icons.
    pub fg: Hsla,
    /// The surface beneath everything.
    pub bg: Hsla,
    /// The raised panel surface: the background mixed toward the foreground.
    pub surface: Hsla,
    /// Secondary text.
    pub muted: Hsla,
    /// Disabled text and placeholder hints.
    pub dim: Hsla,
    /// Hairline separators and key-cap borders.
    pub line: Hsla,
    /// Panel borders and outlined buttons.
    pub line_strong: Hsla,
    /// Hovered rows and buttons.
    pub hover: Hsla,
    /// Selected rows and segments.
    pub selection: Hsla,
    /// Focus rings, selected items, and primary buttons.
    pub accent: Hsla,
    /// Error marks and destructive buttons.
    pub danger: Hsla,
    /// Warning marks and badges.
    pub warning: Hsla,
    /// Key-cap and chip fills.
    pub chip: Hsla,
    /// Scrollbar thumbs and tracks.
    pub scrollbar: ScrollbarColors,
}

impl Tokens {
    /// Derives every shade from `colors`.
    #[must_use]
    pub fn new(colors: OverlayColors) -> Self {
        let surface = mix(colors.background, colors.foreground, 0.07);
        // Secondary text at the lightest opacity that stays readable on both
        // surfaces it sits on, else the foreground itself.
        let muted = [0.62, 0.7, 0.78, 0.86, 0.94]
            .into_iter()
            .map(|alpha| colors.foreground.opacity(alpha))
            .find(|muted| {
                [colors.background, surface].iter().all(|under| {
                    contrast(over(*muted, *under), *under) >= TEXT_CONTRAST
                })
            })
            .unwrap_or(colors.foreground);
        Self {
            fg: colors.foreground,
            bg: colors.background,
            surface,
            muted,
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

    /// Buttons while the pointer holds them down: twice the hover overlay.
    #[must_use]
    pub fn pressed(&self) -> Hsla {
        Hsla {
            a: (self.hover.a * 2.0).min(1.0),
            ..self.hover
        }
    }

    /// A selected row while the pointer holds it down.
    #[must_use]
    pub fn selection_pressed(&self) -> Hsla {
        Hsla {
            a: (self.selection.a * 1.6).min(1.0),
            ..self.selection
        }
    }

    /// Accent-filled buttons and rows while held down.
    #[must_use]
    pub fn accent_pressed(&self) -> Hsla {
        mix(self.accent, self.bg, 0.22)
    }

    /// Text and icons drawn on an accent fill: the background, or the
    /// foreground, whichever reads better, unless neither reaches 4.5:1,
    /// when black or white does.
    #[must_use]
    pub fn on_accent(&self) -> Hsla {
        let best = |candidates: [Hsla; 2]| {
            candidates.into_iter().max_by(|a, b| {
                contrast(*a, self.accent).total_cmp(&contrast(*b, self.accent))
            })
        };
        best([self.bg, self.fg])
            .filter(|color| contrast(*color, self.accent) >= TEXT_CONTRAST)
            .or_else(|| {
                best([hsla(0.0, 0.0, 0.0, 1.0), hsla(0.0, 0.0, 1.0, 1.0)])
            })
            .unwrap_or(self.bg)
    }

    /// The focus ring's color: the accent, moved toward the foreground in
    /// steps until it reaches 3:1 against both the background and the
    /// surface.
    #[must_use]
    pub fn ring(&self) -> Hsla {
        (0..=10_u8)
            .map(|step| mix(self.accent, self.fg, f32::from(step) / 10.0))
            .find(|ring| {
                [self.bg, self.surface]
                    .iter()
                    .all(|under| contrast(*ring, *under) >= NON_TEXT_CONTRAST)
            })
            .unwrap_or(self.fg)
    }

    /// The dimming layer beneath a modal surface.
    #[must_use]
    pub fn scrim(&self) -> Hsla {
        self.bg.opacity(0.6)
    }

    /// Whether the background is light, for shadow strength.
    #[must_use]
    pub fn is_light(&self) -> bool {
        self.bg.l > 0.5
    }

    /// The soft shadow the Linux client-side frame paints in its border:
    /// it must stay within the ten-point inset around the content.
    #[must_use]
    pub fn frame_shadow(&self) -> Vec<BoxShadow> {
        let strength = if self.is_light() { 0.28 } else { 0.55 };
        vec![BoxShadow {
            color: hsla(0.0, 0.0, 0.0, strength),
            offset: point(px(0.0), px(2.0)),
            blur_radius: px(8.0),
            spread_radius: px(0.0),
            inset: false,
        }]
    }

    /// The two-layer drop shadow raised surfaces carry.
    #[must_use]
    pub fn shadow(&self) -> Vec<BoxShadow> {
        let (far, near) = if self.is_light() {
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
                inset: false,
            },
            BoxShadow {
                color: hsla(0.0, 0.0, 0.0, near),
                offset: point(px(0.0), px(2.0)),
                blur_radius: px(6.0),
                spread_radius: px(0.0),
                inset: false,
            },
        ]
    }

    /// A solid ring of [`Self::ring`], [`size::FOCUS_RING`] wide, around a
    /// focused control. Components draw it through [`crate::focus::ring`],
    /// which shows it only for keyboard focus.
    #[must_use]
    pub fn focus_ring(&self) -> Vec<BoxShadow> {
        vec![BoxShadow {
            color: self.ring(),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: size::FOCUS_RING,
            inset: false,
        }]
    }
}

/// Mixes `from` toward `to` by `amount` in sRGB, ignoring alpha.
#[must_use]
pub fn mix(from: Hsla, to: Hsla, amount: f32) -> Hsla {
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

/// WCAG 2's minimum contrast for text.
pub const TEXT_CONTRAST: f32 = 4.5;
/// WCAG 2.1's minimum contrast for focus indicators and other meaningful
/// graphics (1.4.11).
pub const NON_TEXT_CONTRAST: f32 = 3.0;

/// `color` drawn over the opaque `background`, as the screen shows it.
#[must_use]
pub fn over(color: Hsla, background: Hsla) -> Hsla {
    let alpha = color.a;
    let mixed = mix(background, Hsla { a: 1.0, ..color }, alpha);
    Hsla { a: 1.0, ..mixed }
}

/// The WCAG 2 contrast ratio between two opaque colors, from 1 to 21.
#[must_use]
pub fn contrast(a: Hsla, b: Hsla) -> f32 {
    fn luminance(color: Hsla) -> f32 {
        let rgb = Rgba::from(color);
        let channel = |value: f32| {
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(rgb.r)
            + 0.7152 * channel(rgb.g)
            + 0.0722 * channel(rgb.b)
    }
    let (a, b) = (luminance(a), luminance(b));
    let (light, dark) = if a > b { (a, b) } else { (b, a) };
    (light + 0.05) / (dark + 0.05)
}

/// The sizes components share, in points.
pub mod size {
    use gpui::{Pixels, px};

    /// A control's height: buttons, fields, selects, and segments.
    pub const CONTROL_HEIGHT: Pixels = px(24.0);
    /// A menu or list row's height.
    pub const ROW_HEIGHT: Pixels = px(28.0);
    /// Corner radius of controls.
    pub const CONTROL_RADIUS: f32 = 6.0;
    /// Corner radius of raised panels such as menus.
    pub const PANEL_RADIUS: f32 = 9.0;
    /// The focus ring's spread.
    pub const FOCUS_RING: Pixels = px(2.0);
    /// Small labels: badges, key caps, and notes.
    pub const TEXT_SMALL: Pixels = px(11.5);
    /// Control labels.
    pub const TEXT_CONTROL: Pixels = px(12.0);
    /// Body text in menus and rows.
    pub const TEXT_BODY: Pixels = px(12.5);
}
