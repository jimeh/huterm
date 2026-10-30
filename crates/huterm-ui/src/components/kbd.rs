//! Kbd: a keyboard shortcut drawn as a key cap, as menus, the palette,
//! dialog buttons, and footer hints show them.
//!
//! Anatomy: a row of [`Glyph`]s, either text or an icon with the text it
//! stands for, inside a compact chip. The owner decides the glyphs: the
//! crate has no notion of platforms or key names. [`cap`] draws the chip on
//! a static surface; [`cap_on`] draws it outlined in the text color of a
//! button or row, whose fill changes as it hovers and presses.
//!
//! Behavior: none; a key cap never takes focus or input.
//!
//! Accessibility: presentation only. Icon glyphs carry no text a screen
//! reader could read, so the control a shortcut belongs to reports it
//! through `aria_keyshortcuts`.

use gpui::{Div, Hsla, Pixels, SharedString, div, prelude::*, px, svg};

use crate::tokens::Tokens;
use crate::typography::ui;

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

/// `glyphs` as a row in `color`, icons at `size`, without a chip. Callers
/// draw text at about the icon size, so text glyphs lift by [`lift`] to sit
/// centered on the icons.
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
            Glyph::Text(text) => row.child(
                div().relative().top(-lift(text, size)).child(text.clone()),
            ),
        };
    }
    row
}

/// How far to lift `text` beside icons `size` high so it looks centered
/// on them. A line centers on its font's ascent and descent, and UI fonts
/// ascend further than they descend, so capitals and digits sit below the
/// icons' center: by 0.29pt for the macOS system font at 11.5pt, whose
/// ascent, descent, and cap height are 0.967, 0.211, and 0.705 em. A lone
/// bracket, brace, parenthesis, bar, or slash also hangs below the baseline,
/// so it lifts by the center of its whole height instead, 1.13pt there.
fn lift(text: &str, size: Pixels) -> Pixels {
    let mut chars = text.chars();
    let hangs = matches!(
        (chars.next(), chars.next()),
        (
            Some('[' | ']' | '{' | '}' | '(' | ')' | '|' | '/' | '\\'),
            None
        )
    );
    size * if hangs { 0.1 } else { 0.026 }
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

/// A key cap on a static surface, such as a footer.
#[must_use]
pub fn cap(glyphs: &[Glyph], tokens: Tokens) -> Div {
    chip(glyphs, tokens.fg)
        .bg(tokens.chip)
        .border_1()
        .border_color(tokens.line)
}

/// A key cap on a button or row, drawn in its text color `on`. It outlines
/// the keys without a fill of its own, so they keep the contrast `on` was
/// chosen for against each fill the button or row takes.
#[must_use]
pub fn cap_on(glyphs: &[Glyph], on: Hsla) -> Div {
    chip(glyphs, on).border_1().border_color(on.opacity(0.35))
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
        .text_size(ui(10.5))
        .line_height(ui(14.0))
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
