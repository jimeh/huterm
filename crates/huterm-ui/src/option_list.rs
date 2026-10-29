//! The list behind a combobox's suggestions and a command's items: options
//! of one line or two, a highlight that keys and a moving pointer move,
//! and scrolling that keeps the highlight in view.
//!
//! [`moved`] is the pure navigation. The crate's `OptionList` holds one
//! list's state and draws it for the component that owns it, which decides
//! what a hover and a pick do. Whether or not a moving pointer highlights,
//! the list tracks the option under the pointer until the pointer leaves
//! the options, for an owner that reports it. Focus never enters the list:
//! a press on an option prevents the default focus change, so the field
//! that drives the list keeps focus and is not blurred.

// User input picks positions here, so a stale index must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Context, Div, ElementId, MouseButton, MouseMoveEvent,
    Pixels, Point, ScrollHandle, SharedString, Window, canvas, div, prelude::*,
    px,
};

use crate::animation::AnimationSchedule;
use crate::click::from_keyboard;
use crate::list_scrollbar::ListScrollbar;
use crate::listbox::ItemId;
use crate::tokens::{Tokens, size};

/// One option: a label and an optional second line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListOption {
    /// What the owner receives when the option is highlighted or picked.
    /// Unique within its list: rows, picks, and a kept highlight follow it.
    pub id: ItemId,
    /// The visible text, which is also the option's accessible name.
    pub label: SharedString,
    /// Secondary text under the label.
    pub description: Option<SharedString>,
}

impl ListOption {
    /// An option `label` named `id`.
    #[must_use]
    pub fn new(id: impl Into<ItemId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
        }
    }

    /// The option with `text` under its label.
    #[must_use]
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }
}

/// Draws an option's content in place of its label and description, such
/// as a title with its matched characters marked. It receives the option
/// and whether it is highlighted; the list still draws the row's fill,
/// height, and accessibility node.
pub type RowContent =
    Rc<dyn Fn(&ListOption, bool, &mut Window, &mut App) -> AnyElement>;

/// A move of the highlight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// The next option.
    Next,
    /// The previous option.
    Previous,
    /// One page of options down.
    PageDown,
    /// One page of options up.
    PageUp,
    /// The first option.
    First,
    /// The last option.
    Last,
}

/// Where a highlight at `current` lands after `step` in a list of `count`
/// options showing `page` of them at once. Next and Previous wrap at the
/// ends when `wrap` is set and stop there otherwise; pages always stop.
/// With no highlight, Next and Page Down start at the first option, and
/// Previous and Page Up at the last. `None` for an empty list.
#[must_use]
pub fn moved(
    current: Option<usize>,
    step: Move,
    count: usize,
    page: usize,
    wrap: bool,
) -> Option<usize> {
    let last = count.checked_sub(1)?;
    let page = page.max(1);
    Some(match (step, current.map(|current| current.min(last))) {
        (Move::First, _) | (Move::Next | Move::PageDown, None) => 0,
        (Move::Last, _) | (Move::Previous | Move::PageUp, None) => last,
        (Move::Next, Some(current)) if current == last => {
            if wrap {
                0
            } else {
                last
            }
        }
        (Move::Next, Some(current)) => current + 1,
        (Move::Previous, Some(0)) => {
            if wrap {
                last
            } else {
                0
            }
        }
        (Move::Previous, Some(current)) => current - 1,
        (Move::PageDown, Some(current)) => (current + page).min(last),
        (Move::PageUp, Some(current)) => current.saturating_sub(page),
    })
}

/// Where the option highlighted at `highlight` in `old` sits in `new`: on
/// the same option, not the same position, since filtering moves options.
#[must_use]
pub fn remap(
    old: &[ListOption],
    highlight: Option<usize>,
    new: &[ListOption],
) -> Option<usize> {
    let id = &old.get(highlight?)?.id;
    new.iter().position(|option| &option.id == id)
}

/// A one-line option's height.
const ROW_HEIGHT: Pixels = size::ROW_HEIGHT;
/// An option with a description's height.
const DESCRIBED_ROW_HEIGHT: Pixels = px(40.0);
/// The space between the list's edge and its options.
pub(crate) const PADDING: f32 = 4.0;

/// What a component that owns an [`OptionList`] does with it.
pub(crate) trait Owner: Sized + 'static {
    /// The list.
    fn option_list(&mut self) -> &mut OptionList;
    /// A moving pointer reached the option `id`.
    fn hover_option(&mut self, id: &ItemId, cx: &mut Context<'_, Self>);
    /// A pointer or accessibility click picked the option `id`.
    fn pick_option(&mut self, id: &ItemId, cx: &mut Context<'_, Self>);
}

/// One list's options, highlight, and scrolling.
pub(crate) struct OptionList {
    options: Vec<ListOption>,
    highlight: Option<usize>,
    scroll: ScrollHandle,
    /// The fading indicator shown while options overflow the list.
    scrollbar: ListScrollbar,
    /// The owner's row height, in place of the one the options imply.
    row_height: Option<Pixels>,
    /// The height beyond which the options scroll, padding included.
    max_height: Option<Pixels>,
    /// The laid-out height the highlight was last revealed in.
    viewport: Pixels,
    content: Option<RowContent>,
    /// The option under the pointer, by id, so it follows the option.
    hovered: Option<ItemId>,
}

impl OptionList {
    pub(crate) fn new() -> Self {
        let scroll = ScrollHandle::new();
        Self {
            options: Vec::new(),
            highlight: None,
            scrollbar: ListScrollbar::new(scroll.clone()),
            scroll,
            row_height: None,
            max_height: None,
            viewport: px(0.0),
            content: None,
            hovered: None,
        }
    }

    pub(crate) fn options(&self) -> &[ListOption] {
        &self.options
    }

    pub(crate) fn highlight(&self) -> Option<usize> {
        self.highlight
    }

    pub(crate) fn highlighted(&self) -> Option<&ListOption> {
        self.options.get(self.highlight?)
    }

    /// The option under the pointer, if it is still listed.
    pub(crate) fn hovered(&self) -> Option<&ListOption> {
        let id = self.hovered.as_ref()?;
        self.options.iter().find(|option| &option.id == id)
    }

    /// Records the pointer entering or leaving the option `id`. A leave
    /// clears only that option, since the next one's enter may come first.
    fn set_hovered(&mut self, id: &ItemId, hovering: bool) {
        if hovering {
            self.hovered = Some(id.clone());
        } else if self.hovered.as_ref() == Some(id) {
            self.hovered = None;
        }
    }

    pub(crate) fn position(&self, id: &str) -> Option<usize> {
        self.options.iter().position(|option| option.id == id)
    }

    pub(crate) fn set_row_height(&mut self, height: Option<Pixels>) {
        self.row_height = height;
    }

    pub(crate) fn set_content(&mut self, content: Option<RowContent>) {
        self.content = content;
    }

    pub(crate) fn max_height(&self) -> Option<Pixels> {
        self.max_height
    }

    /// Sets the height beyond which the options scroll, revealing the
    /// highlight within it.
    pub(crate) fn set_max_height(&mut self, max_height: Option<Pixels>) {
        if self.max_height != max_height {
            self.max_height = max_height;
            if let Some(row) = self.highlight {
                self.reveal(row);
            }
        }
    }

    /// Every option's height: the owner's, else taller when any option has
    /// a description, so all rows match and a page is whole rows.
    pub(crate) fn row_height(&self) -> Pixels {
        self.row_height.unwrap_or_else(|| {
            if self
                .options
                .iter()
                .any(|option| option.description.is_some())
            {
                DESCRIBED_ROW_HEIGHT
            } else {
                ROW_HEIGHT
            }
        })
    }

    /// The options' height before any scrolling, padding included.
    pub(crate) fn content_height(&self) -> Pixels {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a list never holds 2^24 options"
        )]
        let rows = self.options.len() as f32;
        self.row_height() * rows + px(PADDING * 2.0)
    }

    /// The height the options show in: the last layout's, which a parent
    /// may hold below the maximum, else the maximum; `None` before any
    /// layout without a maximum.
    fn visible_height(&self) -> Option<Pixels> {
        let laid_out = self.scroll.bounds().size.height;
        if laid_out > px(0.0) {
            Some(laid_out)
        } else {
            self.max_height.map(|max| max.min(self.content_height()))
        }
    }

    /// How many whole options show at once, at least one.
    pub(crate) fn page(&self) -> usize {
        let Some(height) = self.visible_height() else {
            return 1;
        };
        let rows = (height - px(PADDING * 2.0)) / self.row_height();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a whole, non-negative count of visible rows"
        )]
        let rows = rows.floor().max(1.0) as usize;
        rows
    }

    /// Replaces the options, keeping the highlight on the same option
    /// where it still is. It scrolls into view when its row moved or the
    /// rows' height changed, and with `reveal_kept` also when it stayed on
    /// its row; otherwise the list keeps the user's scroll position, as
    /// when the same options come back with another title. Returns whether
    /// the highlighted option changed.
    pub(crate) fn set_options(
        &mut self,
        options: Vec<ListOption>,
        reveal_kept: bool,
    ) -> bool {
        let before = self.highlighted().map(|option| option.id.clone());
        let height = self.row_height();
        let highlight = remap(&self.options, self.highlight, &options);
        self.options = options;
        // An option that left the list is no longer under the pointer.
        if self.hovered().is_none() {
            self.hovered = None;
        }
        if !self.set_highlight(highlight)
            && (reveal_kept || self.row_height() != height)
            && let Some(row) = self.highlight
        {
            self.reveal(row);
        }
        self.highlighted().map(|option| &option.id) != before.as_ref()
    }

    /// Highlights the option at `highlight`, or none, and scrolls it into
    /// view. Returns whether the highlight moved.
    pub(crate) fn set_highlight(&mut self, highlight: Option<usize>) -> bool {
        let highlight = highlight.filter(|row| *row < self.options.len());
        if self.highlight == highlight {
            return false;
        }
        self.highlight = highlight;
        if let Some(row) = highlight {
            self.reveal(row);
            self.scrollbar.show();
        }
        true
    }

    /// Moves the highlight by `step`, wrapping at the ends when `wrap` is
    /// set. Returns whether it moved.
    pub(crate) fn step(&mut self, step: Move, wrap: bool) -> bool {
        let target =
            moved(self.highlight, step, self.options.len(), self.page(), wrap);
        self.set_highlight(target)
    }

    /// Scrolls the fewest points that show `row` whole, with the padding
    /// beside it. The fixed row height gives its place before any layout,
    /// so the scrollbar drawn in the same frame already follows; without a
    /// known height, GPUI scrolls at the next layout.
    pub(crate) fn reveal(&self, row: usize) {
        let Some(height) = self.visible_height() else {
            self.scroll.scroll_to_item(row);
            return;
        };
        let row_height = f32::from(self.row_height());
        #[expect(
            clippy::cast_precision_loss,
            reason = "a list never holds 2^24 options"
        )]
        let top = row_height * row as f32;
        let bottom = top + row_height + PADDING * 2.0;
        let height = f32::from(height);
        let max = (f32::from(self.content_height()) - height).max(0.0);
        let shown = -f32::from(self.scroll.offset().y);
        let shown = if top < shown {
            top
        } else if bottom > shown + height {
            bottom - height
        } else {
            shown
        };
        self.scroll
            .set_offset(Point::new(px(0.0), px(-shown.clamp(0.0, max))));
    }

    pub(crate) fn scroll(&self) -> &ScrollHandle {
        &self.scroll
    }

    pub(crate) fn scrollbar(&self) -> &ListScrollbar {
        &self.scrollbar
    }

    /// Shows the scroll indicator, as on opening.
    pub(crate) fn show_scrollbar(&mut self) {
        self.scrollbar.show();
    }

    pub(crate) fn schedule(&self, now: Instant) -> AnimationSchedule {
        self.scrollbar.schedule(now)
    }

    pub(crate) fn advance(&mut self, now: Instant) -> bool {
        self.scrollbar.advance(now)
    }

    /// Whether the last frame drew `row` whole inside the list.
    #[cfg(all(test, feature = "test-support"))]
    pub(crate) fn shows(&self, row: usize) -> bool {
        let viewport = self.scroll.bounds();
        let Some(item) = self.scroll.bounds_for_item(row) else {
            return false;
        };
        let top = item.top() + self.scroll.offset().y;
        top >= viewport.top() && top + item.size.height <= viewport.bottom()
    }

    /// The options as a `ListBox` named `label` in a relative frame with
    /// its scrollbar. A moving pointer highlights when `pointer_highlights`;
    /// otherwise rows only show a hover fill. In an `overlay`, presses stop
    /// at the list, so nothing beneath it acts on them.
    pub(crate) fn render<V: Owner>(
        &mut self,
        label: SharedString,
        tokens: Tokens,
        pointer_highlights: bool,
        overlay: bool,
        window: &mut Window,
        cx: &mut Context<'_, V>,
    ) -> Div {
        // A parent may resize the list without a change here, so a frame
        // whose layout differs from the last asks for another, which
        // reveals the highlight in the new height.
        let laid_out = self.scroll.bounds().size.height;
        if laid_out > px(0.0) && laid_out != self.viewport {
            self.viewport = laid_out;
            if let Some(row) = self.highlight {
                self.reveal(row);
            }
        }
        let seen = self.viewport;
        let scroll = self.scroll.clone();
        let resized = canvas(
            move |_, window, _| {
                let laid_out = scroll.bounds().size.height;
                if laid_out > px(0.0) && laid_out != seen {
                    window.request_animation_frame();
                }
            },
            |_, (), _, _| {},
        )
        .absolute()
        .size(px(0.0));
        let look = Look {
            tokens,
            pointer_highlights,
            row_height: self.row_height(),
        };
        let content = self.content.clone();
        let rows: Vec<AnyElement> = self
            .options
            .iter()
            .enumerate()
            .map(|(row, option)| {
                let highlighted = self.highlight == Some(row);
                let inner = match &content {
                    Some(content) => content(option, highlighted, window, cx),
                    None => default_content(option, highlighted, tokens),
                };
                option_row(option, row, highlighted, look, inner, cx)
            })
            .collect();
        let view = cx.entity();
        let scrollbar = self.scrollbar.elements(
            "options-scrollbar",
            tokens.scrollbar,
            &view,
            |owner: &mut V| &mut owner.option_list().scrollbar,
            cx,
        );
        let count = rows.len();
        div()
            .relative()
            .flex()
            .flex_col()
            .min_h_0()
            .child(
                div()
                    .id("options")
                    .role(gpui::Role::ListBox)
                    .aria_label(label)
                    .aria_size_of_set(count)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .p(px(PADDING))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .when_some(self.max_height, gpui::Styled::max_h)
                    .text_size(size::TEXT_BODY)
                    .text_color(tokens.fg)
                    // The field keeps focus, so it is never blurred by a
                    // press here.
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        window.prevent_default();
                        if overlay {
                            cx.stop_propagation();
                        }
                    })
                    .on_scroll(cx.listener(|owner: &mut V, _, _, cx| {
                        owner.option_list().scrollbar.show();
                        cx.notify();
                    }))
                    .children(rows),
            )
            .child(resized)
            .children(scrollbar)
    }
}

/// How a list draws its rows.
#[derive(Clone, Copy)]
struct Look {
    tokens: Tokens,
    pointer_highlights: bool,
    row_height: Pixels,
}

/// The row for `option` at `row`, around its drawn `inner` content.
fn option_row<V: Owner>(
    option: &ListOption,
    row: usize,
    highlighted: bool,
    look: Look,
    inner: AnyElement,
    cx: &mut Context<'_, V>,
) -> AnyElement {
    let tokens = look.tokens;
    let (hovered, entered, picked) =
        (option.id.clone(), option.id.clone(), option.id.clone());
    div()
        // Keyed by the option, so a press is not completed by whichever
        // option takes the row's place if the list changes before the
        // release.
        .id((ElementId::from("option"), option.id.clone()))
        .role(gpui::Role::ListBoxOption)
        .aria_label(option.label.clone())
        .aria_position_in_set(row)
        .aria_selected(highlighted)
        .when(
            highlighted,
            StatefulInteractiveElement::aria_active_descendant,
        )
        .flex_none()
        .flex()
        .flex_col()
        .justify_center()
        .h(look.row_height)
        .px(px(10.0))
        .rounded(px(size::CONTROL_RADIUS))
        .overflow_hidden()
        .when(highlighted, |row| row.bg(tokens.selection))
        .when(!highlighted && !look.pointer_highlights, |row| {
            row.hover(|style| style.bg(tokens.hover))
        })
        // A held row keeps its own text, so a highlighted one takes the
        // selection's pressed fill.
        .active(move |style| {
            style.bg(if highlighted {
                tokens.selection_pressed()
            } else {
                tokens.pressed()
            })
        })
        .on_hover(cx.listener(move |owner, hovering: &bool, _, _| {
            owner.option_list().set_hovered(&entered, *hovering);
        }))
        // Only a moving pointer highlights: a list that opens or filters
        // under a resting pointer keeps its highlight.
        .when(look.pointer_highlights, |row| {
            row.on_mouse_move(cx.listener(
                move |owner, _: &MouseMoveEvent, _, cx| {
                    owner.hover_option(&hovered, cx);
                },
            ))
        })
        .on_click(cx.listener(move |owner, event, _, cx| {
            if !from_keyboard(event) {
                owner.pick_option(&picked, cx);
            }
        }))
        .child(inner)
        .into_any_element()
}

/// An option's label, and its description as secondary text.
fn default_content(
    option: &ListOption,
    highlighted: bool,
    tokens: Tokens,
) -> AnyElement {
    let secondary = if highlighted {
        tokens.muted_on(tokens.selection)
    } else {
        tokens.muted
    };
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .child(div().truncate().child(option.label.clone()))
        .when_some(option.description.clone(), |content, description| {
            content.child(
                div()
                    .truncate()
                    .text_size(size::TEXT_SMALL)
                    .text_color(secondary)
                    .child(description),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(labels: &[&str]) -> Vec<ListOption> {
        labels
            .iter()
            .map(|label| ListOption::new(label.to_lowercase(), *label))
            .collect()
    }

    #[test]
    fn next_and_previous_wrap_only_when_asked() {
        assert_eq!(moved(Some(3), Move::Next, 4, 2, true), Some(0));
        assert_eq!(moved(Some(3), Move::Next, 4, 2, false), Some(3));
        assert_eq!(moved(Some(0), Move::Previous, 4, 2, true), Some(3));
        assert_eq!(moved(Some(0), Move::Previous, 4, 2, false), Some(0));
        assert_eq!(moved(Some(1), Move::Next, 4, 2, false), Some(2));
        assert_eq!(moved(Some(2), Move::Previous, 4, 2, true), Some(1));
    }

    #[test]
    fn a_move_without_a_highlight_starts_at_an_end() {
        assert_eq!(moved(None, Move::Next, 4, 2, false), Some(0));
        assert_eq!(moved(None, Move::PageDown, 4, 2, false), Some(0));
        assert_eq!(moved(None, Move::Previous, 4, 2, false), Some(3));
        assert_eq!(moved(None, Move::PageUp, 4, 2, false), Some(3));
        assert_eq!(moved(None, Move::Next, 0, 2, true), None);
    }

    #[test]
    fn pages_move_by_whole_pages_and_stop_at_the_ends() {
        assert_eq!(moved(Some(1), Move::PageDown, 10, 4, true), Some(5));
        assert_eq!(moved(Some(8), Move::PageDown, 10, 4, true), Some(9));
        assert_eq!(moved(Some(5), Move::PageUp, 10, 4, true), Some(1));
        assert_eq!(moved(Some(2), Move::PageUp, 10, 4, true), Some(0));
        assert_eq!(moved(Some(2), Move::PageDown, 10, 0, true), Some(3));
        assert_eq!(moved(Some(4), Move::First, 10, 4, true), Some(0));
        assert_eq!(moved(Some(4), Move::Last, 10, 4, true), Some(9));
    }

    #[test]
    fn a_stale_highlight_past_the_end_moves_from_the_last_option() {
        assert_eq!(moved(Some(9), Move::Previous, 3, 1, false), Some(1));
        assert_eq!(moved(Some(9), Move::Next, 3, 1, false), Some(2));
    }

    #[test]
    fn a_remapped_highlight_follows_its_option() {
        let old = options(&["Menlo", "Monaco", "Mononoki"]);
        let new = options(&["Monaco", "Mononoki"]);
        assert_eq!(remap(&old, Some(1), &new), Some(0));
        assert_eq!(remap(&old, Some(0), &new), None, "Menlo is gone");
        assert_eq!(remap(&old, None, &new), None);
        assert_eq!(remap(&old, Some(7), &new), None);
    }
}
