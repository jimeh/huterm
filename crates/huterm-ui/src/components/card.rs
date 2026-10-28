//! Card: a bordered panel that groups related rows, after shadcn/ui's Card,
//! such as a group of settings or a list of shortcuts.
//!
//! Anatomy: an optional title above the panel, muted; then the panel, on
//! the surface fill with a hairline border, whose rows are separated by
//! hairlines. Rows are usually [`crate::components::field::Field`]s or
//! [`crate::components::item::Item`]s.
//!
//! Behavior: none; a card never takes focus or input.
//!
//! Focus: the panel clips its rows to its rounded corners, so a full-width
//! row that takes focus, such as a
//! [`crate::components::collapsible::Collapsible`]'s trigger, draws its
//! ring inside its edge ([`crate::focus::ring_inset`]) and rounds its fill
//! and ring to [`row_corners`].
//!
//! Accessibility: a titled card is a `Group` named by its title; an
//! untitled one reports no node, and its rows report their own.

use gpui::{
    AnyElement, App, Corners, ElementId, IntoElement, ParentElement, Pixels,
    RenderOnce, SharedString, Window, div, prelude::*, px,
};

use crate::tokens::{Tokens, size};

/// The panel's corner radius.
const RADIUS: f32 = 8.0;

/// The corner radii of row `index` of a card's `count` rows, for a row that
/// draws a fill or a ring to its edges: the radius inside the panel's
/// border where the row meets the panel's rounded corners, and square
/// elsewhere.
#[must_use]
pub fn row_corners(index: usize, count: usize) -> Corners<Pixels> {
    // Inside the one-point border.
    let inner = px(RADIUS - 1.0);
    let (top, bottom) = (index == 0, index + 1 == count);
    let radius = |at: bool| if at { inner } else { Pixels::ZERO };
    Corners {
        top_left: radius(top),
        top_right: radius(top),
        bottom_right: radius(bottom),
        bottom_left: radius(bottom),
    }
}

/// A card.
#[derive(IntoElement)]
pub struct Card {
    id: ElementId,
    tokens: Tokens,
    title: Option<SharedString>,
    rows: Vec<AnyElement>,
}

impl std::fmt::Debug for Card {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Card")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("rows", &self.rows.len())
            .finish_non_exhaustive()
    }
}

impl Card {
    /// An untitled card; add its rows as children.
    #[must_use]
    pub fn new(id: impl Into<ElementId>, tokens: Tokens) -> Self {
        Self {
            id: id.into(),
            tokens,
            title: None,
            rows: Vec::new(),
        }
    }

    /// The title drawn above the panel, as written.
    #[must_use]
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }
}

impl ParentElement for Card {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.rows.extend(elements);
    }
}

impl RenderOnce for Card {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let tokens = self.tokens;
        let rows = self.rows.into_iter().enumerate().map(|(index, row)| {
            div()
                .when(index > 0, |row| {
                    row.border_t_1().border_color(tokens.line)
                })
                .child(row)
        });
        let panel = div()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(RADIUS))
            .border_1()
            .border_color(tokens.line)
            .bg(tokens.surface)
            .children(rows);
        div()
            .id(self.id)
            .when_some(self.title.clone(), |card, title| {
                card.role(gpui::Role::Group).aria_label(title)
            })
            .flex()
            .flex_col()
            .gap(px(6.0))
            .children(self.title.map(|title| {
                div()
                    .text_size(size::TEXT_SMALL)
                    .text_color(tokens.muted)
                    .child(title)
            }))
            .child(panel)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use gpui::{TestAppContext, div, prelude::*, px};

    use super::Card;
    use crate::testing::{a11y_node, a11y_nodes, host, tokens};

    #[gpui::test]
    fn a_titled_card_is_a_named_group(cx: &mut TestAppContext) {
        let (_, cx) = host(cx, |_, _| {
            div()
                .child(
                    Card::new("window", tokens())
                        .title("WINDOW")
                        .child(div().h(px(20.0))),
                )
                .child(Card::new("plain", tokens()).child(div().h(px(20.0))))
                .into_any_element()
        });
        cx.update(|window, _| {
            let aria = &a11y_node(window, "window")["aria"];
            assert_eq!(aria["role"], "Group");
            assert_eq!(aria["label"], "WINDOW");
            let spelled =
                format!("{:?}", gpui::ElementId::Name("plain".into()));
            assert!(
                a11y_nodes(window).iter().all(|node| {
                    node["element_id"].as_str() != Some(spelled.as_str())
                }),
                "an untitled card reports no node"
            );
        });
    }

    #[gpui::test]
    fn rows_after_the_first_draw_a_hairline_above(cx: &mut TestAppContext) {
        let (_, cx) = host(cx, |_, _| {
            Card::new("rows", tokens())
                .child(div().h(px(20.0)).debug_selector(|| "first".into()))
                .child(div().h(px(20.0)).debug_selector(|| "second".into()))
                .into_any_element()
        });
        let first = cx.debug_bounds("first").expect("drawn");
        let second = cx.debug_bounds("second").expect("drawn");
        // One point of the card's border, then one of the hairline.
        assert_eq!(first.top(), px(1.0));
        assert_eq!(second.top(), first.bottom() + px(1.0));
    }
}
