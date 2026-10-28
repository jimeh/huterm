//! Colors and sizes every component draws with.
//!
//! [`OverlayColors`] is the crate's color input: the few theme colors an
//! owner supplies. [`Tokens`] derives every shade components use from them.
//! Themes are arbitrary, so the derivation corrects shades that would miss
//! WCAG's minimums. Every enabled text color reaches 4.5:1 on the fills it
//! is drawn on: [`Tokens::muted`] on the background, the surface, chips,
//! and footers; [`Tokens::muted_on`] on a selected row's fill; [`Tokens::on`] and
//! [`Tokens::on_accent`] on solid fills, at rest and pressed; and
//! [`Tokens::accent_text`] and its siblings for colored text. The focus
//! ring reaches 3:1 against the background and the surface. Disabled text
//! and placeholders ([`Tokens::dim`]) are exempt, as WCAG exempts inactive
//! controls, and so are the borders of controls whose text labels identify
//! them.
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
    /// The raised panel surface: the background mixed toward the foreground,
    /// less where the foreground would fall below 4.5:1 on it.
    pub surface: Hsla,
    /// Secondary text, readable on the background, the surface, chips, and
    /// footers.
    pub muted: Hsla,
    /// Disabled text and placeholders only: enabled text uses `muted`.
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
        let bg = colors.background;
        // Body text keeps 4.5:1 even on the faintest fills drawn under it;
        // a theme whose foreground has too little headroom for that has it
        // moved away from the background, as little as that takes.
        let away = if luminance(bg) > 0.18 {
            hsla(0.0, 0.0, 0.0, 1.0)
        } else {
            hsla(0.0, 0.0, 1.0, 1.0)
        };
        let fg = (0..=20_u8)
            .map(|step| mix(colors.foreground, away, f32::from(step) / 20.0))
            .find(|fg| {
                let faintest = [
                    bg,
                    mix(bg, *fg, 0.01),
                    over(colors.selection.opacity(0.12), bg),
                    over(fg.opacity(0.04), bg),
                ];
                faintest
                    .iter()
                    .all(|under| contrast(*fg, *under) >= TEXT_CONTRAST)
            })
            .unwrap_or(away);
        // The surface lifts toward the foreground by 7%, or less where that
        // would leave body text below 4.5:1 on it or on the faintest fills
        // drawn over it.
        let surface = [0.07, 0.06, 0.05, 0.04, 0.03, 0.02, 0.01]
            .into_iter()
            .map(|amount| mix(bg, fg, amount))
            .find(|surface| {
                [
                    *surface,
                    over(colors.selection.opacity(0.12), *surface),
                    over(fg.opacity(0.04), *surface),
                ]
                .iter()
                .all(|under| contrast(fg, *under) >= TEXT_CONTRAST)
            })
            .unwrap_or(bg);
        // Fills under body text keep it at 4.5:1: each takes the strongest
        // of its opacities that allows that on the background and the
        // surface.
        let chip = gentlest(
            fg,
            fg,
            bg,
            surface,
            &[0.1, 0.09, 0.08, 0.07, 0.06, 0.05, 0.04],
        );
        let selection = gentlest(
            colors.selection,
            fg,
            bg,
            surface,
            &[0.35, 0.3, 0.25, 0.2, 0.15, 0.12],
        );
        let footer = footer(fg, bg, surface);
        // Secondary text at the lightest opacity that stays readable on
        // every plain fill it sits on, else the foreground itself.
        let unders = [
            bg,
            surface,
            over(chip, bg),
            over(chip, surface),
            over(footer, bg),
            over(footer, surface),
        ];
        let muted = lightest(fg, &unders);
        Self {
            fg,
            bg: colors.background,
            surface,
            muted,
            dim: colors.foreground.opacity(0.38),
            line: colors.foreground.opacity(0.1),
            line_strong: colors.foreground.opacity(0.22),
            hover: colors.foreground.opacity(0.08),
            selection,
            accent: colors.accent,
            danger: colors.danger,
            warning: colors.warning,
            chip,
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
        self.shade(self.accent, 0.2)
    }

    /// A destructive button's fill: the danger color softened toward the
    /// background. Its text is `on(danger_fill())`.
    #[must_use]
    pub fn danger_fill(&self) -> Hsla {
        mix(self.danger, self.bg, 0.16)
    }

    /// A destructive button's fill while hovered.
    #[must_use]
    pub fn danger_hover(&self) -> Hsla {
        self.shade(self.danger_fill(), 0.1)
    }

    /// A destructive button's fill while held down.
    #[must_use]
    pub fn danger_pressed(&self) -> Hsla {
        self.shade(self.danger_fill(), 0.2)
    }

    /// The faint fill under a panel's footer of key hints.
    #[must_use]
    pub fn footer(&self) -> Hsla {
        footer(self.fg, self.bg, self.surface)
    }

    /// `fill` moved by `amount` away from the text drawn on it: darker under
    /// light text and lighter under dark text, so a hovered or pressed fill
    /// only gains contrast with its text.
    #[must_use]
    pub fn shade(&self, fill: Hsla, amount: f32) -> Hsla {
        let away = if luminance(self.on(fill)) > luminance(fill) {
            hsla(0.0, 0.0, 0.0, 1.0)
        } else {
            hsla(0.0, 0.0, 1.0, 1.0)
        };
        mix(fill, away, amount)
    }

    /// Text and icons drawn on every one of the opaque `fills`, such as a
    /// button's rest, hover, and pressed fills: the background or the
    /// foreground, whichever reads better, unless neither reaches 4.5:1 on
    /// them all, when black or white does.
    #[must_use]
    pub fn on_all(&self, fills: &[Hsla]) -> Hsla {
        let worst = |text: Hsla| {
            fills
                .iter()
                .map(|fill| contrast(text, *fill))
                .fold(f32::INFINITY, f32::min)
        };
        let best = |candidates: [Hsla; 2]| {
            candidates
                .into_iter()
                .max_by(|a, b| worst(*a).total_cmp(&worst(*b)))
                .unwrap_or(self.fg)
        };
        let theme = best([self.bg, self.fg]);
        if worst(theme) >= TEXT_CONTRAST {
            theme
        } else {
            best([hsla(0.0, 0.0, 0.0, 1.0), hsla(0.0, 0.0, 1.0, 1.0)])
        }
    }

    /// Text and icons drawn on the opaque `fill`, such as a warning badge.
    #[must_use]
    pub fn on(&self, fill: Hsla) -> Hsla {
        self.on_all(&[fill])
    }

    /// Text and icons drawn on an accent fill. Its pressed shade moves away
    /// from this text, so it reads there too.
    #[must_use]
    pub fn on_accent(&self) -> Hsla {
        self.on(self.accent)
    }

    /// Secondary text on the translucent `fill`, such as a selected row's
    /// `selection`, drawn over either the background or the surface: the
    /// lightest foreground that reaches 4.5:1 on both, else whichever of the
    /// theme's colors, black, or white does.
    #[must_use]
    pub fn muted_on(&self, fill: Hsla) -> Hsla {
        let unders = [over(fill, self.bg), over(fill, self.surface)];
        let muted = lightest(self.fg, &unders);
        let reads = unders.iter().all(|under| {
            contrast(over(muted, *under), *under) >= TEXT_CONTRAST
        });
        if reads { muted } else { self.on_all(&unders) }
    }

    /// The accent as text, as in a link or an error band: moved toward the
    /// foreground until it reaches 4.5:1 on the background, the surface,
    /// and chips on either.
    #[must_use]
    pub fn accent_text(&self) -> Hsla {
        self.text_from(self.accent)
    }

    /// The warning color as text.
    #[must_use]
    pub fn warning_text(&self) -> Hsla {
        self.text_from(self.warning)
    }

    /// The danger color as text.
    #[must_use]
    pub fn danger_text(&self) -> Hsla {
        self.text_from(self.danger)
    }

    fn text_from(&self, color: Hsla) -> Hsla {
        let unders = [
            self.bg,
            self.surface,
            over(self.chip, self.bg),
            over(self.chip, self.surface),
        ];
        toward(color, self.fg, &unders, TEXT_CONTRAST)
            .unwrap_or_else(|| self.on_all(&unders))
    }

    /// The focus ring's color: the accent, moved toward the foreground in
    /// steps until it reaches 3:1 against both the background and the
    /// surface, else black or white.
    #[must_use]
    pub fn ring(&self) -> Hsla {
        let unders = [self.bg, self.surface];
        toward(self.accent, self.fg, &unders, NON_TEXT_CONTRAST)
            .unwrap_or_else(|| self.on_all(&unders))
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

/// The footer fill: a faint wash of the foreground, as faint as body text
/// on it needs.
fn footer(fg: Hsla, bg: Hsla, surface: Hsla) -> Hsla {
    gentlest(fg, fg, bg, surface, &[0.03, 0.02, 0.01])
}

/// `color` at the first of `alphas` under which `fg` reaches 4.5:1 over
/// both `bg` and `surface`, else at the last.
fn gentlest(
    color: Hsla,
    fg: Hsla,
    bg: Hsla,
    surface: Hsla,
    alphas: &[f32],
) -> Hsla {
    let last = alphas.last().copied().unwrap_or(0.0);
    alphas
        .iter()
        .map(|alpha| color.opacity(*alpha))
        .find(|fill| {
            [bg, surface]
                .iter()
                .all(|under| contrast(fg, over(*fill, *under)) >= TEXT_CONTRAST)
        })
        .unwrap_or(color.opacity(last))
}

/// `fg` at the lightest secondary-text opacity that reaches 4.5:1 on every
/// one of `unders`, else `fg` itself.
fn lightest(fg: Hsla, unders: &[Hsla]) -> Hsla {
    [0.62, 0.7, 0.78, 0.86, 0.94]
        .into_iter()
        .map(|alpha| fg.opacity(alpha))
        .find(|text| {
            unders.iter().all(|under| {
                contrast(over(*text, *under), *under) >= TEXT_CONTRAST
            })
        })
        .unwrap_or(fg)
}

/// `color` moved toward `goal` in tenths until it reaches `minimum` on
/// every one of `unders`, if any step does.
fn toward(
    color: Hsla,
    goal: Hsla,
    unders: &[Hsla],
    minimum: f32,
) -> Option<Hsla> {
    (0..=10_u8)
        .map(|step| mix(color, goal, f32::from(step) / 10.0))
        .find(|candidate| {
            unders
                .iter()
                .all(|under| contrast(*candidate, *under) >= minimum)
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
    let (a, b) = (luminance(a), luminance(b));
    let (light, dark) = if a > b { (a, b) } else { (b, a) };
    (light + 0.05) / (dark + 0.05)
}

/// WCAG 2's relative luminance of an opaque color, from 0 to 1.
fn luminance(color: Hsla) -> f32 {
    let rgb = Rgba::from(color);
    let channel = |value: f32| {
        if value <= 0.039_28 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(rgb.r) + 0.7152 * channel(rgb.g) + 0.0722 * channel(rgb.b)
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

#[cfg(test)]
mod tests {
    use gpui::{Hsla, hsla};

    use super::{
        NON_TEXT_CONTRAST, OverlayColors, TEXT_CONTRAST, Tokens, contrast, over,
    };
    use crate::scrollbar::ScrollbarColors;

    fn gray(lightness: f32) -> Hsla {
        hsla(0.0, 0.0, lightness, 1.0)
    }

    #[test]
    fn a_theme_with_almost_no_contrast_still_reads() {
        // Every color close to mid-gray: no step toward the theme's own
        // colors reaches the minimums, so the fallbacks must.
        let t = Tokens::new(OverlayColors {
            foreground: gray(0.56),
            background: gray(0.5),
            selection: gray(0.53),
            accent: gray(0.5),
            danger: gray(0.52),
            warning: gray(0.54),
            scrollbar: ScrollbarColors {
                thumb: gray(0.56),
                track: gray(0.5),
            },
        });
        for under in [t.bg, t.surface] {
            assert!(contrast(t.fg, under) >= TEXT_CONTRAST);
            assert!(contrast(over(t.muted, under), under) >= TEXT_CONTRAST);
            assert!(contrast(t.accent_text(), under) >= TEXT_CONTRAST);
            let ring = t.focus_ring()[0].color;
            assert!(contrast(ring, under) >= NON_TEXT_CONTRAST);
        }
        for fill in [t.accent, t.accent_pressed()] {
            assert!(contrast(t.on_accent(), fill) >= TEXT_CONTRAST);
        }
    }
}
