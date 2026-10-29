//! Item: a row that names one thing and offers its actions, after
//! shadcn/ui's Item, such as a keybinding, a global shortcut, or a profile.
//!
//! Anatomy: optional media at the leading edge, such as an icon or a
//! [`crate::components::collapsible::chevron`]; the content (a title line
//! that may carry elements before and after the title, and may draw the
//! title as an element, such as key-cap glyphs; and a description
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
//! never toggles the row. Actions that are content, such as key caps, may
//! draw inside the trigger instead, which then spans the row.
//!
//! An item that leads somewhere, such as a search result, is one link, as
//! shadcn/ui renders an Item as an anchor ([`Item::link`]). It takes focus,
//! and the owner runs it on its activation command (Space and Return by
//! default); GPUI's implicit keyboard click is ignored. A click runs it,
//! and hover and pressed states show. Its actions draw inside the link, so
//! they should be content, such as key caps, not controls. Its focus ring
//! shows for keyboard focus alone, inside its edge
//! ([`crate::focus::ring_inset`]), where a card that clips its overflow
//! cannot hide it; an owner rounds it through `Styled`, such as with
//! [`crate::components::card::row_corners`] for a row of a card.
//!
//! Accessibility: a `Group` named by the title, with the description as
//! its description; the actions report their own nodes. A link item is a
//! `Link` named and described the same way. As a collapsible's header, the
//! item reports no node of its own: the trigger carries its title and
//! description.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Div, ElementId, FocusHandle, IntoElement, RenderOnce,
    SharedString, StyleRefinement, Window, div, prelude::*, px,
};

use crate::click::from_keyboard;
use crate::focus;
use crate::tokens::{Tokens, size};

type Activate = Rc<dyn Fn(&mut Window, &mut App)>;

/// An item.
#[derive(IntoElement)]
pub struct Item {
    id: ElementId,
    title: SharedString,
    tokens: Tokens,
    description: Option<SharedString>,
    media: Option<AnyElement>,
    title_element: Option<AnyElement>,
    before: Vec<AnyElement>,
    after: Vec<AnyElement>,
    actions: Vec<AnyElement>,
    link: Option<FocusHandle>,
    key_context: Option<SharedString>,
    on_click: Option<Activate>,
    style: StyleRefinement,
}

impl std::fmt::Debug for Item {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Item")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("description", &self.description)
            .field("link", &self.link.is_some())
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
            title_element: None,
            before: Vec::new(),
            after: Vec::new(),
            actions: Vec::new(),
            link: None,
            key_context: None,
            on_click: None,
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

    /// Draws `element` in place of the title's text, such as a shortcut's
    /// key-cap glyphs; the title still names the item.
    #[must_use]
    pub fn title_element(mut self, element: impl IntoElement) -> Self {
        self.title_element = Some(element.into_any_element());
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

    /// Makes the whole item one link that takes focus through `focus`.
    #[must_use]
    pub fn link(mut self, focus: &FocusHandle) -> Self {
        self.link = Some(focus.clone());
        self
    }

    /// The key context a link carries, such as the owner's control context
    /// that its activation command binds under.
    #[must_use]
    pub fn key_context(mut self, context: impl Into<SharedString>) -> Self {
        self.key_context = Some(context.into());
        self
    }

    /// Called for a pointer click on a link.
    #[must_use]
    pub fn on_click(
        mut self,
        action: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(action));
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
            .map(|line| match self.title_element {
                Some(element) => line.child(element),
                None => line.child(self.title.clone()),
            })
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
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.id.clone();
        let tokens = self.tokens;
        let link = self.link.take();
        let key_context = self.key_context.take();
        let on_click = self.on_click.take();
        let parts = self.into_parts();
        let mut item = div()
            .id(id)
            .role(if link.is_some() {
                gpui::Role::Link
            } else {
                gpui::Role::Group
            })
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
        let Some(focus) = link else {
            return item;
        };
        let item = item
            .track_focus(&focus)
            .when_some(key_context, |item, context| {
                item.key_context(context.as_ref())
            })
            .hover(|style| style.bg(tokens.hover))
            .active(|style| style.bg(tokens.pressed()))
            .when_some(on_click, |item, action| {
                item.on_click(move |event, window, cx| {
                    if !from_keyboard(event) {
                        action(window, cx);
                    }
                })
            });
        focus::ring_inset(item, &focus, tokens, window, cx)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{
        AccessibleAction, KeyUpEvent, Keystroke, TestAppContext, div,
        prelude::*, px,
    };

    use super::Item;
    use crate::focus::inset_ring_drawn;
    use crate::testing::{a11y_action, a11y_node, host, tokens};

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
    fn a_title_element_draws_in_place_of_the_title_that_names_the_item(
        cx: &mut TestAppContext,
    ) {
        let (_, cx) = host(cx, |_, _| {
            Item::new("unbound", "Command-K", tokens())
                .title_element(
                    div().size(px(20.0)).debug_selector(|| "glyphs".into()),
                )
                .into_any_element()
        });
        assert!(cx.debug_bounds("glyphs").is_some(), "the element drew");
        cx.update(|window, _| {
            assert_eq!(
                a11y_node(window, "unbound")["aria"]["label"],
                "Command-K"
            );
        });
    }

    #[gpui::test]
    fn a_link_item_is_one_named_link_that_a_click_runs(
        cx: &mut TestAppContext,
    ) {
        let focus = cx.update(|cx| cx.focus_handle());
        let runs = Rc::new(Cell::new(0));
        let (drawn, counted) = (focus.clone(), runs.clone());
        let (_, cx) = host(cx, move |_, _| {
            let counted = counted.clone();
            Item::new("result", "New Tab", tokens())
                .description("Open a new tab.")
                .action(div().size(px(20.0)))
                .link(&drawn)
                .on_click(move |_, _| counted.set(counted.get() + 1))
                .into_any_element()
        });
        cx.update(|window, _| {
            let aria = &a11y_node(window, "result")["aria"];
            assert_eq!(aria["role"], "Link");
            assert_eq!(aria["label"], "New Tab");
            assert_eq!(aria["description"], "Open a new tab.");
        });
        // Focus from the keyboard rings it inside its edge; GPUI's click on
        // Enter's key-up runs nothing, since the owner routes the key.
        cx.simulate_keystrokes("a");
        cx.update(|window, cx| focus.focus(window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| assert!(inset_ring_drawn(&focus, window, cx)));
        cx.simulate_keystrokes("enter");
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("enter").expect("a valid keystroke"),
        });
        cx.run_until_parked();
        assert_eq!(runs.get(), 0);
        a11y_action(cx, "result", AccessibleAction::Click);
        assert_eq!(runs.get(), 1);
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
