//! Alert: a banner that tells the user about a state of the app, after
//! shadcn/ui's Alert, such as an invalid configuration file or a conflict.
//!
//! Anatomy: a bordered band in one of three [`Tone`]s with a title, an
//! optional description under it, and any further content, such as the
//! error text and action buttons.
//!
//! Behavior: none of its own; its buttons keep their own. The owner shows
//! and removes it with the state it reports.
//!
//! Colors: the band is a faint wash of the tone over whatever it sits on,
//! its border the tone itself; the title draws in the foreground, and the
//! description in muted text corrected for the wash, so both reach 4.5:1.
//!
//! Accessibility: a `Status` node named by the title, with the description
//! as its description; its content reports its own nodes.

use gpui::{
    AnyElement, App, ElementId, Hsla, IntoElement, ParentElement, RenderOnce,
    SharedString, Window, div, prelude::*, px,
};

use crate::tokens::{Tokens, size};

/// An alert's color scheme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    /// A quiet chip wash: information.
    #[default]
    Neutral,
    /// The warning color: something to look at, such as a conflict.
    Warning,
    /// The danger color: an error, such as an invalid file.
    Danger,
}

/// The colors an alert draws in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    /// The translucent band.
    pub fill: Hsla,
    /// The band's border.
    pub border: Hsla,
    /// The title.
    pub title: Hsla,
    /// The description.
    pub text: Hsla,
}

impl Tone {
    /// The colors this tone draws in with `tokens`.
    #[must_use]
    pub fn colors(self, tokens: Tokens) -> Colors {
        let (fill, border) = match self {
            Self::Neutral => (tokens.chip, tokens.line_strong),
            Self::Warning => (tokens.warning.opacity(0.08), tokens.warning),
            Self::Danger => (tokens.danger.opacity(0.08), tokens.danger),
        };
        Colors {
            fill,
            border,
            title: tokens.fg,
            text: tokens.muted_on(fill),
        }
    }
}

/// An alert.
#[derive(IntoElement)]
pub struct Alert {
    id: ElementId,
    title: SharedString,
    tokens: Tokens,
    tone: Tone,
    description: Option<SharedString>,
    content: Vec<AnyElement>,
}

impl std::fmt::Debug for Alert {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Alert")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("tone", &self.tone)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

impl Alert {
    /// A neutral alert titled `title`; add further content as children.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        title: impl Into<SharedString>,
        tokens: Tokens,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            tokens,
            tone: Tone::Neutral,
            description: None,
            content: Vec::new(),
        }
    }

    /// The alert's tone.
    #[must_use]
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// The text under the title.
    #[must_use]
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }
}

impl ParentElement for Alert {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.content.extend(elements);
    }
}

impl RenderOnce for Alert {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let colors = self.tone.colors(self.tokens);
        div()
            .id(self.id)
            .role(gpui::Role::Status)
            .aria_label(self.title.clone())
            .when_some(self.description.clone(), |alert, text| {
                alert.aria_description(text)
            })
            .flex()
            .flex_col()
            .gap(px(6.0))
            .p(px(12.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(colors.border)
            .bg(colors.fill)
            .child(
                div()
                    .text_size(size::TEXT_BODY)
                    .text_color(colors.title)
                    .child(self.title),
            )
            .children(self.description.map(|text| {
                div()
                    .text_size(size::TEXT_CONTROL)
                    .text_color(colors.text)
                    .child(text)
            }))
            .children(self.content)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use gpui::{TestAppContext, prelude::*};

    use super::{Alert, Tone};
    use crate::testing::{a11y_node, host, tokens};
    use crate::tokens::{TEXT_CONTRAST, contrast, over};

    #[gpui::test]
    fn an_alert_reports_a_named_status(cx: &mut TestAppContext) {
        let (_, cx) = host(cx, |_, _| {
            Alert::new("invalid", "config.toml has an error", tokens())
                .tone(Tone::Danger)
                .description("Settings is read-only until the file is fixed.")
                .into_any_element()
        });
        cx.update(|window, _| {
            let aria = &a11y_node(window, "invalid")["aria"];
            assert_eq!(aria["role"], "Status");
            assert_eq!(aria["label"], "config.toml has an error");
            assert_eq!(
                aria["description"],
                "Settings is read-only until the file is fixed."
            );
        });
    }

    #[test]
    fn every_tones_text_reads_on_its_wash() {
        let t = tokens();
        for tone in [Tone::Neutral, Tone::Warning, Tone::Danger] {
            let colors = tone.colors(t);
            for under in [t.bg, t.surface] {
                let band = over(colors.fill, under);
                for text in [colors.title, colors.text] {
                    let ratio = contrast(over(text, band), band);
                    assert!(ratio >= TEXT_CONTRAST, "{tone:?}: {ratio:.2}:1");
                }
            }
        }
    }
}
