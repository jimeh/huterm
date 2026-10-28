//! Badge: a small pill with a count or a short label, such as a menu's
//! notice count, a keybinding's Default or Custom tag, or a sidebar count.
//!
//! Anatomy: a rounded pill in one of four [`Tone`]s, at least as wide as it
//! is tall, so a single digit stays round.
//!
//! Behavior: none; a badge never takes focus or input. A badge that should
//! act belongs inside a button.
//!
//! Accessibility: a `Label` node named by its text, so assistive technology
//! reads the count or tag.

use gpui::{
    Div, ElementId, FontWeight, Hsla, SharedString, Stateful, div, prelude::*,
    px,
};

use crate::tokens::Tokens;

/// A badge's color scheme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    /// A quiet chip fill: tags such as Default or Custom.
    #[default]
    Neutral,
    /// The accent fill: a selected or primary state.
    Accent,
    /// The warning fill: counts that want attention, such as notices.
    Warning,
    /// The danger fill: errors.
    Danger,
}

impl Tone {
    /// The pill's fill and text colors.
    #[must_use]
    pub fn colors(self, tokens: Tokens) -> (Hsla, Hsla) {
        match self {
            Self::Neutral => (tokens.chip, tokens.fg),
            Self::Accent => (tokens.accent, tokens.on_accent()),
            Self::Warning => (tokens.warning, tokens.on(tokens.warning)),
            Self::Danger => (tokens.danger, tokens.on(tokens.danger)),
        }
    }
}

/// A badge reading `label` in `tone`.
#[must_use]
pub fn badge(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    tone: Tone,
    tokens: Tokens,
) -> Stateful<Div> {
    let label = label.into();
    let (fill, text) = tone.colors(tokens);
    div()
        .id(id)
        .role(gpui::Role::Label)
        .aria_label(label.clone())
        .flex_none()
        .min_w(px(18.0))
        .px(px(6.0))
        .rounded(px(9.0))
        .bg(fill)
        .text_size(px(10.5))
        .line_height(px(16.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(text)
        .text_center()
        .child(label)
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use gpui::{TestAppContext, prelude::*};

    use super::{Tone, badge};
    use crate::testing::{a11y_node, host, tokens};

    #[gpui::test]
    fn a_badge_reads_as_its_label(cx: &mut TestAppContext) {
        let (_, cx) = host(cx, |_, _| {
            badge("notices", "3", Tone::Warning, tokens()).into_any_element()
        });
        cx.update(|window, _| {
            let aria = &a11y_node(window, "notices")["aria"];
            assert_eq!(aria["role"], "Label");
            assert_eq!(aria["label"], "3");
        });
    }
}
