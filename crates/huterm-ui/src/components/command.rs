//! Command: a search field over a list of items it filters, inline rather
//! than in a popover, after shadcn/ui's Command and cmdk, such as a list of
//! commands to run or of components beside the selected one's details.
//!
//! Anatomy: the owner's search [`Input`], drawn by the owner with any
//! chrome, usually above the `Command` entity, which draws the items as a
//! list that fills its height or scrolls within [`Command::set_max_height`],
//! or an empty state in their place. The owner places the two, so a search
//! can also sit in a header beside the list.
//!
//! Items: the owner filters and orders them for the query and passes them
//! to [`Command::set_options`], typically from the field's `Changed`
//! event. Search rules differ: a command palette ranks fuzzy matches by
//! use and marks the matched characters, and a license list matches a
//! component's name, license, and kind. The highlight stays on the same
//! item while it still matches. Otherwise it moves to the first item, or,
//! with [`HighlightFallback::Clear`], to none, so that Confirm can never
//! report an item other than the one the user highlighted. An owner that
//! restarts at the first item on every query calls [`Command::select_first`]
//! after [`Command::set_options`]; [`Command::clear_highlight`] removes the
//! highlight.
//!
//! Keyboard: focus stays in the search field, which keeps every key. The
//! owner routes its commands to [`Command::select_next`],
//! [`Command::select_previous`], [`Command::select_page_down`],
//! [`Command::select_page_up`], [`Command::select_first`],
//! [`Command::select_last`], and [`Command::confirm`]. Moves stop at the
//! ends, and a page is the items that show at once. Confirm reports the
//! highlighted item.
//!
//! Pointer: a moving pointer highlights an item, unless the owner turns
//! that off with [`Command::with_pointer_highlight`] for a list whose
//! highlight shows details, where rows only show a hover fill; a click
//! highlights the item and reports it picked. A press on an item keeps
//! focus in the search. In either mode, [`Command::hovered`] is the item
//! under the pointer until the pointer leaves the items, for an owner that
//! reports it.
//!
//! Events: [`CommandEvent::Highlighted`] whenever another item becomes
//! highlighted, from any cause, and [`CommandEvent::Picked`] for Confirm or
//! a click. A highlight that clears reports nothing; Confirm then reports
//! nothing and returns `false`.
//!
//! States: none of its own. A search field that is disabled takes no focus,
//! so the owner's commands for it do not arrive. An owner that reports the
//! list's scrolling, as in smoke state, reads [`Command::scroll_position`]
//! and [`Command::scrollbar`].
//!
//! Accessibility: the search field is an `EditableComboBox` whose list
//! shows while it has items, with the highlighted item as its description;
//! the empty state reports the list collapsed. The list is
//! a `ListBox` named by the command's label, of `ListBoxOption` rows whose
//! highlighted one is selected and claims the active descendant. GPUI
//! honors that only inside the focused node, which is the field, so
//! assistive technology follows the highlight through the description.
//!
//! Reference: shadcn/ui's Command (cmdk), and WAI-ARIA's combobox with a
//! listbox that always shows.

use std::time::Instant;

use gpui::{
    Bounds, Context, ElementId, Entity, EventEmitter, Pixels, Render,
    SharedString, Window, div, prelude::*,
};

use crate::animation::AnimationSchedule;
use crate::components::empty::Empty;
use crate::components::input::{Input, ListState};
use crate::list_scrollbar::ListScrollbar;
use crate::listbox::ItemId;
use crate::option_list::{ListOption, Move, OptionList, Owner, RowContent};
use crate::tokens::Tokens;

/// What a command reports to its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandEvent {
    /// The highlighted item changed.
    Highlighted(ItemId),
    /// An item was picked by Confirm or a click.
    Picked(ItemId),
}

/// Where the highlight goes when [`Command::set_options`] drops its item.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HighlightFallback {
    /// The first item, also when nothing was highlighted.
    #[default]
    First,
    /// No item, as a picker whose Confirm must never reach another item
    /// needs. Nothing highlighted stays that way.
    Clear,
}

/// A command list: the items, their highlight, and the search field they
/// answer.
pub struct Command {
    id: ElementId,
    label: SharedString,
    input: Entity<Input>,
    tokens: Tokens,
    pointer_highlights: bool,
    fallback: HighlightFallback,
    empty: SharedString,
    list: OptionList,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Command")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("options", &self.list.options())
            .field("highlight", &self.list.highlight())
            .finish_non_exhaustive()
    }
}

impl EventEmitter<CommandEvent> for Command {}

impl Command {
    /// An empty command named `label` over the owner's search `input`,
    /// which reports itself as the command's field from now on.
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        input: &Entity<Input>,
        tokens: Tokens,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let command = Self {
            id: id.into(),
            label: label.into(),
            input: input.clone(),
            tokens,
            pointer_highlights: true,
            fallback: HighlightFallback::First,
            empty: "No results".into(),
            list: OptionList::new(),
        };
        command.sync_field(cx);
        command
    }

    /// Whether a moving pointer highlights items, as it does by default.
    #[must_use]
    pub fn with_pointer_highlight(mut self, highlights: bool) -> Self {
        self.pointer_highlights = highlights;
        self
    }

    /// Where the highlight goes when its item leaves the list: the first
    /// item by default.
    #[must_use]
    pub fn with_highlight_fallback(
        mut self,
        fallback: HighlightFallback,
    ) -> Self {
        self.fallback = fallback;
        self
    }

    /// The command drawing each item's content with `content`.
    #[must_use]
    pub fn with_content(mut self, content: RowContent) -> Self {
        self.list.set_content(Some(content));
        self
    }

    /// The command's rows `height` tall, in place of the height their
    /// descriptions imply.
    #[must_use]
    pub fn with_row_height(mut self, height: Pixels) -> Self {
        self.list.set_row_height(Some(height));
        self
    }

    /// The search field.
    #[must_use]
    pub fn input(&self) -> &Entity<Input> {
        &self.input
    }

    /// The items.
    #[must_use]
    pub fn options(&self) -> &[ListOption] {
        self.list.options()
    }

    /// The highlighted item.
    #[must_use]
    pub fn highlighted(&self) -> Option<&ListOption> {
        self.list.highlighted()
    }

    /// The item under the pointer, whether or not the pointer highlights;
    /// `None` once the pointer leaves the items or the item leaves the list.
    #[must_use]
    pub fn hovered(&self) -> Option<&ListOption> {
        self.list.hovered()
    }

    /// Recolors the list, as after a theme change; unchanged tokens are
    /// ignored, so an owner may call this on every render.
    pub fn set_tokens(&mut self, tokens: Tokens, cx: &mut Context<'_, Self>) {
        if self.tokens != tokens {
            self.tokens = tokens;
            cx.notify();
        }
    }

    /// Replaces the items with those matching the query, keeping the
    /// highlight on the same item while it is still there, and otherwise
    /// moving it as the [`HighlightFallback`] says.
    pub fn set_options(
        &mut self,
        options: Vec<ListOption>,
        cx: &mut Context<'_, Self>,
    ) {
        let before = self.list.highlighted().map(|option| option.id.clone());
        self.list.set_options(options);
        if self.fallback == HighlightFallback::First
            && self.list.highlight().is_none()
        {
            self.list.set_highlight(Some(0));
        }
        self.changed(before.as_ref(), cx);
    }

    /// The title the empty state shows while there are no items, such as
    /// what the query failed to match.
    pub fn set_empty(
        &mut self,
        title: impl Into<SharedString>,
        cx: &mut Context<'_, Self>,
    ) {
        let title = title.into();
        if self.empty != title {
            self.empty = title;
            cx.notify();
        }
    }

    /// The height beyond which the items scroll; with `None` the list
    /// fills the height its owner gives it.
    pub fn set_max_height(
        &mut self,
        max_height: Option<Pixels>,
        cx: &mut Context<'_, Self>,
    ) {
        if self.list.max_height() != max_height {
            self.list.set_max_height(max_height);
            cx.notify();
        }
    }

    /// Highlights the item `id`, when there is one.
    pub fn select(&mut self, id: &str, cx: &mut Context<'_, Self>) {
        if let Some(row) = self.list.position(id) {
            self.move_to(Some(row), cx);
        }
    }

    /// Highlights no item, so Confirm reports nothing.
    pub fn clear_highlight(&mut self, cx: &mut Context<'_, Self>) {
        self.move_to(None, cx);
    }

    /// Highlights the next item, stopping at the last.
    pub fn select_next(&mut self, cx: &mut Context<'_, Self>) {
        self.step(Move::Next, cx);
    }

    /// Highlights the previous item, stopping at the first.
    pub fn select_previous(&mut self, cx: &mut Context<'_, Self>) {
        self.step(Move::Previous, cx);
    }

    /// Highlights the item a page down.
    pub fn select_page_down(&mut self, cx: &mut Context<'_, Self>) {
        self.step(Move::PageDown, cx);
    }

    /// Highlights the item a page up.
    pub fn select_page_up(&mut self, cx: &mut Context<'_, Self>) {
        self.step(Move::PageUp, cx);
    }

    /// Highlights the first item.
    pub fn select_first(&mut self, cx: &mut Context<'_, Self>) {
        self.step(Move::First, cx);
    }

    /// Highlights the last item.
    pub fn select_last(&mut self, cx: &mut Context<'_, Self>) {
        self.step(Move::Last, cx);
    }

    /// Reports the highlighted item picked; returns `false` when there is
    /// none, as in an empty list.
    pub fn confirm(&mut self, cx: &mut Context<'_, Self>) -> bool {
        let Some(id) = self.list.highlighted().map(|option| option.id.clone())
        else {
            return false;
        };
        cx.emit(CommandEvent::Picked(id));
        true
    }

    /// How far the items are scrolled from the top, and the list's bounds
    /// as last laid out, for an owner that reports them, such as in smoke
    /// state.
    #[must_use]
    pub fn scroll_position(&self) -> (Pixels, Bounds<Pixels>) {
        let scroll = self.list.scroll();
        (-scroll.offset().y, scroll.bounds())
    }

    /// The list's scroll indicator, for an owner that reports its state.
    #[must_use]
    pub fn scrollbar(&self) -> &ListScrollbar {
        self.list.scrollbar()
    }

    /// When the scroll indicator next needs a frame or a wake.
    #[must_use]
    pub fn schedule(&self, now: Instant) -> AnimationSchedule {
        self.list.schedule(now)
    }

    /// Advances the scroll indicator to `now`.
    pub fn advance(&mut self, now: Instant, cx: &mut Context<'_, Self>) {
        if self.list.advance(now) {
            cx.notify();
        }
    }

    fn step(&mut self, step: Move, cx: &mut Context<'_, Self>) {
        let before = self.list.highlighted().map(|option| option.id.clone());
        self.list.step(step, false);
        self.changed(before.as_ref(), cx);
    }

    fn move_to(&mut self, row: Option<usize>, cx: &mut Context<'_, Self>) {
        let before = self.list.highlighted().map(|option| option.id.clone());
        self.list.set_highlight(row);
        self.changed(before.as_ref(), cx);
    }

    /// Reports a highlight that moved off `before` and redraws.
    fn changed(&mut self, before: Option<&ItemId>, cx: &mut Context<'_, Self>) {
        let now = self.list.highlighted().map(|option| option.id.clone());
        if now.as_ref() != before
            && let Some(id) = now
        {
            cx.emit(CommandEvent::Highlighted(id));
        }
        self.sync_field(cx);
        cx.notify();
    }

    /// Tells the search field what to report about the list.
    fn sync_field(&self, cx: &mut Context<'_, Self>) {
        let state = ListState {
            expanded: !self.list.options().is_empty(),
            active: self.list.highlighted().map(|option| option.label.clone()),
        };
        self.input.update(cx, |input, cx| {
            input.set_list_state(Some(state), cx);
        });
    }
}

impl Owner for Command {
    fn option_list(&mut self) -> &mut OptionList {
        &mut self.list
    }

    fn hover_option(&mut self, id: &ItemId, cx: &mut Context<'_, Self>) {
        self.select(id, cx);
    }

    fn pick_option(&mut self, id: &ItemId, cx: &mut Context<'_, Self>) {
        // The items may have changed since the frame the press landed on.
        if self.list.position(id).is_none() {
            return;
        }
        self.select(id, cx);
        cx.emit(CommandEvent::Picked(id.clone()));
    }
}

impl Render for Command {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let tokens = self.tokens;
        let content = if self.list.options().is_empty() {
            Empty::new("empty", self.empty.clone(), tokens).into_any_element()
        } else {
            self.list
                .render(
                    self.label.clone(),
                    tokens,
                    self.pointer_highlights,
                    false,
                    window,
                    cx,
                )
                .flex_1()
                .into_any_element()
        };
        div()
            .id(self.id.clone())
            .flex()
            .flex_col()
            .min_h_0()
            .size_full()
            .child(content)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests;
