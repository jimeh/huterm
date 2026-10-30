//! Alert: a banner that tells the user about a state of the app, after
//! shadcn/ui's Alert, such as an invalid configuration file or a conflict.
//!
//! Anatomy: a bordered band in one of three [`Tone`]s with a title, any
//! actions at the end of the title's row, an optional description under
//! it, and any further content, such as the error text and action buttons.
//! A compact alert ([`Alert::compact`]) is one short row, such as a warning
//! whose details wait behind a button.
//!
//! Behavior: none of its own; its buttons keep their own. The owner shows
//! and removes it with the state it reports.
//!
//! Colors: the band is a faint wash of the tone over whatever it sits on,
//! its border the tone itself; the title draws in the foreground where that
//! reaches 4.5:1 on the wash over the background and the surface, else in
//! whichever text color does, and the description in muted text corrected
//! for the wash, so both reach 4.5:1.
//!
//! Accessibility: a `Status` node named by the title, with the description
//! as its description; its content reports its own nodes.

use gpui::{
    AnyElement, App, ElementId, Hsla, IntoElement, ParentElement, RenderOnce,
    SharedString, Window, div, prelude::*, px,
};

use crate::tokens::{TEXT_CONTRAST, Tokens, contrast, over, size};

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
        let unders = [over(fill, tokens.bg), over(fill, tokens.surface)];
        let fg_reads = unders.iter().all(|under| {
            contrast(over(tokens.fg, *under), *under) >= TEXT_CONTRAST
        });
        Colors {
            fill,
            border,
            title: if fg_reads {
                tokens.fg
            } else {
                tokens.on_all(&unders)
            },
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
    actions: Vec<AnyElement>,
    compact: bool,
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
            actions: Vec::new(),
            compact: false,
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

    /// An element at the end of the title's row, such as a button that
    /// shows the details or retries.
    #[must_use]
    pub fn action(mut self, element: impl IntoElement) -> Self {
        self.actions.push(element.into_any_element());
        self
    }

    /// Draws the alert as one short row, with less padding.
    #[must_use]
    pub fn compact(mut self) -> Self {
        self.compact = true;
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
            .map(|alert| {
                if self.compact {
                    alert.px(px(12.0)).py(px(6.0))
                } else {
                    alert.p(px(12.0))
                }
            })
            .rounded(px(8.0))
            .border_1()
            .border_color(colors.border)
            .bg(colors.fill)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(size::TEXT_BODY)
                            .text_color(colors.title)
                            .child(self.title),
                    )
                    .children(self.actions),
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
    use crate::scrollbar::ScrollbarColors;
    use crate::testing::{a11y_node, host, tokens};
    use crate::tokens::{OverlayColors, TEXT_CONTRAST, Tokens, contrast, over};

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
        reads_on_every_wash(tokens());
    }

    #[test]
    fn a_title_reads_where_the_foreground_would_not() {
        let rgb = |hex: u32| gpui::Hsla::from(gpui::rgb(hex));
        // Its foreground reaches only 4.44:1 on the warning wash.
        let dim = Tokens::new(OverlayColors {
            foreground: rgb(0x0078_7878),
            background: rgb(0x0000_0000),
            selection: rgb(0x0033_4466),
            accent: rgb(0x0066_99ff),
            danger: rgb(0x00ff_5555),
            warning: rgb(0x00ff_cc00),
            scrollbar: ScrollbarColors {
                thumb: rgb(0x0078_7878),
                track: rgb(0x0000_0000),
            },
        });
        reads_on_every_wash(dim);
    }

    fn reads_on_every_wash(t: Tokens) {
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
