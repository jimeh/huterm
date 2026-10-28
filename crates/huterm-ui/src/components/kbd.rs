//! Kbd: a keyboard shortcut drawn as a key cap, as menus, the palette,
//! dialog buttons, and footer hints show them.
//!
//! Anatomy: a row of [`Glyph`]s, either text or an icon with the text it
//! stands for, inside a compact chip. The owner decides the glyphs: the
//! crate has no notion of platforms or key names. [`cap`] draws the chip on
//! a plain surface; [`cap_on`] draws it on a filled button in that button's
//! text color.
//!
//! Behavior: none; a key cap never takes focus or input.
//!
//! Accessibility: presentation only. Icon glyphs carry no text a screen
//! reader could read, so the control a shortcut belongs to reports it
//! through `aria_keyshortcuts`.

use gpui::{Div, Hsla, Pixels, SharedString, div, prelude::*, px, svg};

use crate::tokens::Tokens;

/// One drawn piece of a shortcut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Glyph {
    /// Text, such as a key name or a modifier word.
    Text(SharedString),
    /// An SVG asset, and the text it stands for.
    Icon {
        /// The asset path the app's asset source resolves.
        path: SharedString,
        /// The text the icon stands for, such as `⌘`.
        text: SharedString,
    },
}

/// `glyphs` as a row in `color`, icons at `size`, without a chip.
#[must_use]
pub fn glyphs(glyphs: &[Glyph], size: Pixels, color: Hsla) -> Div {
    let mut row = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(1.0))
        .text_color(color);
    for glyph in glyphs {
        row = match glyph {
            Glyph::Icon { path, .. } => row.child(
                svg()
                    .path(path.clone())
                    .size(size)
                    .flex_none()
                    .text_color(color),
            ),
            Glyph::Text(text) => row.child(text.clone()),
        };
    }
    row
}

/// The shortcut as plain text, icons replaced by the text they stand for.
#[must_use]
pub fn text(glyphs: &[Glyph]) -> String {
    glyphs
        .iter()
        .map(|glyph| match glyph {
            Glyph::Text(text) | Glyph::Icon { text, .. } => text.as_ref(),
        })
        .collect()
}

/// A key cap on a plain surface.
#[must_use]
pub fn cap(glyphs: &[Glyph], tokens: Tokens) -> Div {
    chip(glyphs, tokens.fg)
        .bg(tokens.chip)
        .border_1()
        .border_color(tokens.line)
}

/// A key cap on a filled button, drawn in the button's text color `on`.
#[must_use]
pub fn cap_on(glyphs: &[Glyph], on: Hsla) -> Div {
    chip(glyphs, on)
        .bg(on.opacity(0.18))
        .border_1()
        .border_color(on.opacity(0.35))
}

/// The chip's own line height keeps it compact inside taller buttons.
fn chip(content: &[Glyph], color: Hsla) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .px(px(5.0))
        .py(px(1.0))
        .rounded(px(4.0))
        .text_size(px(10.5))
        .line_height(px(14.0))
        .text_color(color)
        .child(glyphs(content, px(10.0), color))
}

#[cfg(test)]
mod tests {
    use super::{Glyph, text};

    #[test]
    fn text_spells_icons_as_the_text_they_stand_for() {
        let glyphs = [
            Glyph::Icon {
                path: "icons/arrow-big-up.svg".into(),
                text: "⇧".into(),
            },
            Glyph::Icon {
                path: "icons/command.svg".into(),
                text: "⌘".into(),
            },
            Glyph::Text("P".into()),
        ];
        assert_eq!(text(&glyphs), "⇧⌘P");
    }
}
