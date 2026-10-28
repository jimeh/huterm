//! Empty: what a list or section shows while it has nothing in it, after
//! shadcn/ui's Empty, such as a card with no global shortcuts.
//!
//! Anatomy: optional media, such as an icon, then a title and an optional
//! description, muted, centered, then any further content, such as a
//! button that adds the first entry.
//!
//! Behavior: none of its own; its buttons keep their own.
//!
//! Accessibility: a `Group` named by the title, with the description as
//! its description; its content reports its own nodes.

use gpui::{
    AnyElement, App, ElementId, IntoElement, ParentElement, RenderOnce,
    SharedString, Window, div, prelude::*, px,
};

use crate::tokens::{Tokens, size};

/// An empty state.
#[derive(IntoElement)]
pub struct Empty {
    id: ElementId,
    title: SharedString,
    tokens: Tokens,
    description: Option<SharedString>,
    media: Option<AnyElement>,
    content: Vec<AnyElement>,
}

impl std::fmt::Debug for Empty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Empty")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

impl Empty {
    /// An empty state titled `title`; add further content as children.
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
            description: None,
            media: None,
            content: Vec::new(),
        }
    }

    /// The text under the title.
    #[must_use]
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }

    /// The element above the title, such as an icon.
    #[must_use]
    pub fn media(mut self, element: impl IntoElement) -> Self {
        self.media = Some(element.into_any_element());
        self
    }
}

impl ParentElement for Empty {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.content.extend(elements);
    }
}

impl RenderOnce for Empty {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        div()
            .id(self.id)
            .role(gpui::Role::Group)
            .aria_label(self.title.clone())
            .when_some(self.description.clone(), |empty, text| {
                empty.aria_description(text)
            })
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.0))
            .px(px(16.0))
            .py(px(20.0))
            .text_center()
            .children(self.media)
            .child(
                div()
                    .text_size(size::TEXT_BODY)
                    .text_color(tokens.fg)
                    .child(self.title),
            )
            .children(self.description.map(|text| {
                div()
                    .text_size(size::TEXT_SMALL)
                    .text_color(tokens.muted)
                    .child(text)
            }))
            .when(!self.content.is_empty(), |empty| {
                empty.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .mt(px(4.0))
                        .children(self.content),
                )
            })
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use gpui::{TestAppContext, prelude::*};

    use super::Empty;
    use crate::testing::{a11y_node, host, tokens};

    #[gpui::test]
    fn an_empty_state_reports_its_title_and_description(
        cx: &mut TestAppContext,
    ) {
        let (_, cx) = host(cx, |_, _| {
            Empty::new("globals", "No global shortcuts", tokens())
                .description("A global shortcut shows or hides a window.")
                .into_any_element()
        });
        cx.update(|window, _| {
            let aria = &a11y_node(window, "globals")["aria"];
            assert_eq!(aria["role"], "Group");
            assert_eq!(aria["label"], "No global shortcuts");
            assert_eq!(
                aria["description"],
                "A global shortcut shows or hides a window."
            );
        });
    }
}
