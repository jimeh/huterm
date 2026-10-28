//! Item: a row that names one thing and offers its actions, after
//! shadcn/ui's Item, such as a keybinding, a global shortcut, or a profile.
//!
//! Anatomy: optional media at the leading edge, such as an icon or a
//! [`crate::components::collapsible::chevron`]; the content (a title line
//! that may carry elements before and after the title, and a description
//! truncated to one line, muted); and trailing actions, such as key caps
//! and buttons. It pads itself as a row of a
//! [`crate::components::card::Card`]; an owner restyles it through
//! `Styled`.
//!
//! Behavior: none of its own; its actions keep their own. An item that
//! expands is the header of a
//! [`crate::components::collapsible::Collapsible`] (through its `item`):
//! the collapsible's trigger covers the media, title, and description, and
//! the actions sit beside the trigger, outside it, so a press on an action
//! never toggles the row.
//!
//! Accessibility: a `Group` named by the title, with the description as
//! its description; the actions report their own nodes. As a collapsible's
//! header, the item reports no node of its own: the trigger carries its
//! title and description.

use gpui::{
    AnyElement, App, Div, ElementId, IntoElement, RenderOnce, SharedString,
    StyleRefinement, Window, div, prelude::*, px,
};

use crate::tokens::{Tokens, size};

/// An item.
#[derive(IntoElement)]
pub struct Item {
    id: ElementId,
    title: SharedString,
    tokens: Tokens,
    description: Option<SharedString>,
    media: Option<AnyElement>,
    before: Vec<AnyElement>,
    after: Vec<AnyElement>,
    actions: Vec<AnyElement>,
    style: StyleRefinement,
}

impl std::fmt::Debug for Item {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Item")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

impl Item {
    /// An item titled `title`.
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
            before: Vec::new(),
            after: Vec::new(),
            actions: Vec::new(),
            style: StyleRefinement::default(),
        }
    }

    /// One line under the title, truncated to fit.
    #[must_use]
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }

    /// The element at the leading edge, such as an icon.
    #[must_use]
    pub fn media(mut self, element: impl IntoElement) -> Self {
        self.media = Some(element.into_any_element());
        self
    }

    /// An element before the title on its line, such as a changed marker.
    #[must_use]
    pub fn before_title(mut self, element: impl IntoElement) -> Self {
        self.before.push(element.into_any_element());
        self
    }

    /// An element after the title on its line, such as a tag.
    #[must_use]
    pub fn after_title(mut self, element: impl IntoElement) -> Self {
        self.after.push(element.into_any_element());
        self
    }

    /// An element at the trailing edge; each call adds one after the last.
    #[must_use]
    pub fn action(mut self, element: impl IntoElement) -> Self {
        self.actions.push(element.into_any_element());
        self
    }
}

impl Styled for Item {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// An item taken apart, for a collapsible's header.
pub(crate) struct Parts {
    pub(crate) title: SharedString,
    pub(crate) description: Option<SharedString>,
    /// The media and content, in a row that grows to fill the item.
    pub(crate) lead: Div,
    /// The actions, at the trailing edge, if any.
    pub(crate) actions: Option<Div>,
    pub(crate) style: StyleRefinement,
}

impl Item {
    /// The item's parts, which [`Item::render`] lays out in a padded row.
    pub(crate) fn into_parts(self) -> Parts {
        let tokens = self.tokens;
        let title = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(size::TEXT_BODY)
            .text_color(tokens.fg)
            .children(self.before)
            .child(self.title.clone())
            .children(self.after);
        let content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(2.0))
            .child(title)
            .children(self.description.clone().map(|text| {
                div()
                    .text_size(size::TEXT_SMALL)
                    .text_color(tokens.muted)
                    .truncate()
                    .child(text)
            }));
        let lead = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(GAP))
            .children(self.media)
            .child(content);
        let actions = (!self.actions.is_empty()).then(|| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.0))
                .children(self.actions)
        });
        Parts {
            title: self.title,
            description: self.description,
            lead,
            actions,
            style: self.style,
        }
    }
}

/// The space between an item's media, content, and actions.
pub(crate) const GAP: f32 = 10.0;
/// An item's horizontal padding, as a row of a card.
pub(crate) const PADDING_X: f32 = 14.0;
/// An item's vertical padding, as a row of a card.
pub(crate) const PADDING_Y: f32 = 9.0;

impl RenderOnce for Item {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let id = self.id.clone();
        let parts = self.into_parts();
        let mut item = div()
            .id(id)
            .role(gpui::Role::Group)
            .aria_label(parts.title)
            .when_some(parts.description, |item, text| {
                item.aria_description(text)
            })
            .flex()
            .items_center()
            .gap(px(GAP))
            .px(px(PADDING_X))
            .py(px(PADDING_Y))
            .child(parts.lead)
            .children(parts.actions);
        item.style().refine(&parts.style);
        item
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use gpui::{TestAppContext, div, prelude::*, px};

    use super::Item;
    use crate::testing::{a11y_node, host, tokens};

    #[gpui::test]
    fn an_item_reports_its_title_and_description(cx: &mut TestAppContext) {
        let (_, cx) = host(cx, |_, _| {
            Item::new("copy", "Copy", tokens())
                .description("Copy the selection to the clipboard.")
                .action(
                    div()
                        .id("reset")
                        .role(gpui::Role::Button)
                        .aria_label("Reset"),
                )
                .into_any_element()
        });
        cx.update(|window, _| {
            let aria = &a11y_node(window, "copy")["aria"];
            assert_eq!(aria["role"], "Group");
            assert_eq!(aria["label"], "Copy");
            assert_eq!(
                aria["description"],
                "Copy the selection to the clipboard."
            );
            assert_eq!(a11y_node(window, "reset")["aria"]["label"], "Reset");
        });
    }

    #[gpui::test]
    fn actions_sit_at_the_trailing_edge_after_a_long_description(
        cx: &mut TestAppContext,
    ) {
        let (_, cx) = host(cx, |_, _| {
            div()
                .w(px(240.0))
                .child(
                    Item::new("copy", "Copy", tokens())
                        .description(
                            "A description far longer than the row is wide, \
                             which truncates rather than pushing the actions",
                        )
                        .action(
                            div()
                                .size(px(30.0))
                                .debug_selector(|| "cap".into()),
                        ),
                )
                .into_any_element()
        });
        let cap = cx.debug_bounds("cap").expect("drawn");
        assert_eq!(cap.right(), px(226.0));
    }
}
