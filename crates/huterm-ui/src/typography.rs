//! The fonts and text scale components draw with.
//!
//! Sizes: text sizes, line heights, and the heights of controls and rows
//! that hold a line of text are design sizes in points written with
//! [`ui`], which turns them into rems, so they follow the window's rem
//! size. A window root draws at its interface scale by calling
//! [`Typography::apply`] before anything lays out; at scale 1 the rem is
//! GPUI's default 16 points, so every [`ui`] size draws at its design
//! size. Spacing, radii, and icons stay in points. Code that measures text
//! or positions rows itself converts a [`ui`] size with the window's rem
//! size (`Rems::to_pixels(window.rem_size())`).
//!
//! Families: the owner sets [`Typography`] as a GPUI global; without one,
//! components draw in the system interface font and the platform's
//! monospace family. Window roots apply the family to their tree with
//! `font_family`, and text inherits it from there, so a component names a
//! family only where its text leaves that tree, as GPUI's tooltips do, or
//! where it draws code in the monospace family.

use gpui::{App, Global, Pixels, Rems, SharedString, Window, px};

/// The family GPUI resolves to the platform's interface font.
pub const SYSTEM_FAMILY: &str = ".SystemUIFont";

/// GPUI's default rem size, at which a [`ui`] size draws at its design
/// size.
pub const DESIGN_REM: Pixels = px(16.0);

/// A design size in points that scales with the interface: `points` at
/// scale 1, in proportion at any other.
#[must_use]
pub const fn ui(points: f32) -> Rems {
    Rems(points / 16.0)
}

/// The fonts and text scale of a desktop client's own interface.
#[derive(Clone, Debug, PartialEq)]
pub struct Typography {
    /// Interface text.
    pub family: SharedString,
    /// Code, paths, process names, and other text that lines up by column.
    pub mono: SharedString,
    /// The factor every [`ui`] size draws at.
    pub scale: f32,
}

impl Global for Typography {}

impl Default for Typography {
    fn default() -> Self {
        Self {
            family: SYSTEM_FAMILY.into(),
            mono: Self::platform_mono(),
            scale: 1.0,
        }
    }
}

impl Typography {
    /// The platform's default monospace family: Menlo on macOS, and the
    /// fontconfig alias elsewhere.
    #[must_use]
    pub fn platform_mono() -> SharedString {
        if cfg!(target_os = "macos") {
            "Menlo".into()
        } else {
            "monospace".into()
        }
    }

    /// The typography the owner set, else the default.
    #[must_use]
    pub fn of(cx: &App) -> Self {
        cx.try_global::<Self>().cloned().unwrap_or_default()
    }

    /// The rem size that draws [`ui`] sizes at this scale.
    #[must_use]
    pub fn rem_size(&self) -> Pixels {
        DESIGN_REM * self.scale
    }

    /// Draws `window` at this scale from its next layout on. A window root
    /// calls it at the start of every render, so a change applies at the
    /// next frame.
    pub fn apply(&self, window: &mut Window) {
        window.set_rem_size(self.rem_size());
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::{Typography, ui};

    #[test]
    fn ui_sizes_draw_at_their_design_size_at_scale_one() {
        let design = Typography::default().rem_size();
        for points in [9.5, 10.5, 11.5, 12.5, 13.0, 13.5, 24.0, 28.0] {
            assert_eq!(ui(points).to_pixels(design), px(points), "{points}");
        }
        let large = Typography {
            scale: 16.0 / 13.0,
            ..Typography::default()
        };
        let text = ui(13.0).to_pixels(large.rem_size());
        assert!((f32::from(text) - 16.0).abs() < 1e-4, "{text:?}");
    }
}
