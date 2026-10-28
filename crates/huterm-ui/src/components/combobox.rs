//! Combobox: a text field whose typing filters a list of suggestions shown
//! below it, after shadcn/ui's Combobox and WAI-ARIA's editable combobox
//! with list autocomplete, such as a font family field over the installed
//! families.
//!
//! Anatomy: the owner's [`Input`], drawn by the owner with any chrome, and
//! the suggestions in a non-modal [`Popover`] below the field, or above it
//! from the window's lower half, at least as wide as the field. The list
//! shows at most eight suggestions and scrolls the rest, within the window's
//! margin. Mount the `Combobox` entity as the last child of a relatively
//! positioned wrapper around the field: it draws nothing in flow, measures
//! the wrapper as its anchor, and draws the list above everything else.
//!
//! Suggestions: the owner's filter turns the field into suggestions
//! whenever the user edits it. It runs synchronously, so the list,
//! the highlight, and the open state always match the text; call
//! [`Combobox::refresh`] when the filter's own data changes. The field
//! accepts any text, not only a suggestion.
//!
//! Keyboard: focus stays in the field, which keeps every key, so typing
//! continues while the list shows. The owner routes its commands to
//! [`Combobox::toggle`], [`Combobox::select_next`],
//! [`Combobox::select_previous`], [`Combobox::select_page_down`],
//! [`Combobox::select_page_up`], [`Combobox::select_first`],
//! [`Combobox::select_last`], [`Combobox::confirm`], and
//! [`Combobox::close`]; each returns whether it acted, so a key the
//! combobox leaves alone can do what it does in a plain field. Down and Up
//! on a closed list open it on the first or last suggestion. Moving changes
//! the highlight, never the field's text; Next and Previous wrap. No
//! suggestion is highlighted until the user moves to one, so Confirm leaves
//! the typed text to the owner, unless [`Combobox::with_auto_highlight`]
//! highlights the first suggestion on every edit, so Confirm completes it.
//!
//! Pointer: a moving pointer highlights a suggestion, and a click picks it.
//! A press on a suggestion prevents the field's blur, so a pick is reported
//! once, with no blur before it. A press on the field leaves the list open.
//!
//! Dismissal: a pick, Confirm, and Close close the list. Escape is the
//! owner's [`Combobox::close`]: the first closes the list and returns
//! `true`, and a second, with the list closed, returns `false` and is the
//! owner's to handle, such as by putting back the value. The list also
//! closes on a press outside it and the field, which still reaches what it
//! lands on, on blur, on window deactivation, and when its field turns
//! disabled or read-only. Focus stays where it is.
//!
//! States: the combobox follows its field. A disabled or read-only field
//! never opens the list.
//!
//! Accessibility: the field is an `EditableComboBox` reporting its value,
//! whether the list shows, and the highlighted suggestion as its
//! description. The list is a `ListBox` named by the combobox's label, of
//! `ListBoxOption` rows whose highlighted one is selected. GPUI honors an
//! active descendant only inside the focused node, and the list draws
//! outside the field, so assistive technology may announce the
//! highlighted suggestion through the description alone.
//!
//! Reference: WAI-ARIA's editable combobox with list autocomplete, and
//! shadcn/ui's Combobox and its `autoHighlight`.

use std::rc::Rc;
use std::time::Instant;

use gpui::{
    App, Bounds, Context, ElementId, Entity, EventEmitter, Focusable as _,
    Pixels, Render, SharedString, Subscription, Window, div, prelude::*, px,
    size as area,
};

use crate::animation::AnimationSchedule;
use crate::components::input::{Changed, Edited, Input, ListState};
use crate::components::popover::{Anchor, Popover, place};
use crate::components::surface::surface;
use crate::listbox::ItemId;
use crate::option_list::{
    ListOption, Move, OptionList, Owner, PADDING, RowContent,
};
use crate::tokens::{Tokens, size};

/// What a combobox reports to its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComboboxEvent {
    /// A suggestion was picked by Confirm or a click; the list has closed,
    /// and the owner applies it to the field.
    Picked(ItemId),
}

/// The suggestions for the field as it is.
type Filter = Rc<dyn Fn(&Input) -> Vec<ListOption>>;

/// How many suggestions show before the rest scroll.
const VISIBLE_ROWS: f32 = 8.0;
/// The list's narrowest width, for narrow fields.
const MIN_WIDTH: Pixels = px(200.0);
const BORDER: f32 = 1.0;

/// A combobox: its field's suggestions, their highlight, and whether they
/// show.
pub struct Combobox {
    id: ElementId,
    label: SharedString,
    input: Entity<Input>,
    tokens: Tokens,
    filter: Filter,
    auto_highlight: bool,
    open: bool,
    popover: Popover,
    list: OptionList,
    _subscriptions: [Subscription; 4],
}

impl std::fmt::Debug for Combobox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Combobox")
            .field("label", &self.label)
            .field("auto_highlight", &self.auto_highlight)
            .field("open", &self.open)
            .field("options", &self.list.options())
            .field("highlight", &self.list.highlight())
            .finish_non_exhaustive()
    }
}

impl EventEmitter<ComboboxEvent> for Combobox {}

impl Combobox {
    /// A combobox named `label` over the owner's `input`, suggesting what
    /// `filter` returns for the field. The field reports itself as the
    /// combobox from now on.
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        input: &Entity<Input>,
        tokens: Tokens,
        filter: impl Fn(&Input) -> Vec<ListOption> + 'static,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let focus = input.read(cx).focus_handle(cx);
        let subscriptions = [
            cx.subscribe(input, |combobox, _, _: &Edited, cx| {
                combobox.edited(cx);
            }),
            // An owner's write refilters a list that shows; only the
            // user's edits open one.
            cx.subscribe(input, |combobox, _, _: &Changed, cx| {
                if combobox.open {
                    combobox.refresh(cx);
                }
            }),
            // A field turned disabled or read-only notifies.
            cx.observe(input, |combobox, _, cx| {
                if combobox.open && !combobox.usable(cx) {
                    combobox.close(cx);
                }
            }),
            // Blur runs inside a draw, where a change would not show until
            // the next input; deferring lets the close draw. A window that
            // deactivates blurs its focused field too.
            cx.on_blur(&focus, window, |_, window, cx| {
                cx.defer_in(window, |combobox, _, cx| {
                    combobox.close(cx);
                });
            }),
        ];
        let combobox = Self {
            id: id.into(),
            label: label.into(),
            input: input.clone(),
            tokens,
            filter: Rc::new(filter),
            auto_highlight: false,
            open: false,
            popover: Popover::new(),
            list: OptionList::new(),
            _subscriptions: subscriptions,
        };
        combobox.sync_field(cx);
        combobox
    }

    /// The combobox highlighting the first suggestion on every edit, so
    /// Confirm completes it, as shadcn/ui's `autoHighlight` does.
    #[must_use]
    pub fn with_auto_highlight(mut self, auto_highlight: bool) -> Self {
        self.auto_highlight = auto_highlight;
        self
    }

    /// The combobox drawing each suggestion's content with `content`.
    #[must_use]
    pub fn with_content(mut self, content: RowContent) -> Self {
        self.list.set_content(Some(content));
        self
    }

    /// The field.
    #[must_use]
    pub fn input(&self) -> &Entity<Input> {
        &self.input
    }

    /// Whether the list shows.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The suggestions from the last filtering.
    #[must_use]
    pub fn options(&self) -> &[ListOption] {
        self.list.options()
    }

    /// The highlighted suggestion.
    #[must_use]
    pub fn highlighted(&self) -> Option<&ListOption> {
        self.list.highlighted()
    }

    /// The list's painted bounds, while it shows.
    #[must_use]
    pub fn list_bounds(&self) -> Option<Bounds<Pixels>> {
        self.popover.content_bounds()
    }

    /// Recolors the list, as after a theme change; unchanged tokens are
    /// ignored, so an owner may call this on every render.
    pub fn set_tokens(&mut self, tokens: Tokens, cx: &mut Context<'_, Self>) {
        if self.tokens != tokens {
            self.tokens = tokens;
            cx.notify();
        }
    }

    /// Opens the list when its field is usable and has suggestions;
    /// returns whether it shows.
    pub fn open(&mut self, cx: &mut Context<'_, Self>) -> bool {
        if !self.open && self.usable(cx) {
            self.filter(cx);
            self.list.set_highlight(self.auto_highlight.then_some(0));
            if !self.list.options().is_empty() {
                self.show(cx);
            }
        }
        self.open
    }

    /// Opens a closed list or closes an open one; returns whether it acted.
    pub fn toggle(&mut self, cx: &mut Context<'_, Self>) -> bool {
        if self.open {
            self.close(cx)
        } else {
            self.open(cx)
        }
    }

    /// Closes the list; returns whether it was open, so a second Escape
    /// can reach the owner.
    pub fn close(&mut self, cx: &mut Context<'_, Self>) -> bool {
        if !self.open {
            return false;
        }
        self.open = false;
        let _ = self.popover.close();
        self.sync_field(cx);
        cx.notify();
        true
    }

    /// Highlights the next suggestion, wrapping; a closed list opens on the
    /// first. Returns whether the list shows.
    pub fn select_next(&mut self, cx: &mut Context<'_, Self>) -> bool {
        self.step(Move::Next, true, cx)
    }

    /// Highlights the previous suggestion, wrapping; a closed list opens on
    /// the last. Returns whether the list shows.
    pub fn select_previous(&mut self, cx: &mut Context<'_, Self>) -> bool {
        self.step(Move::Previous, true, cx)
    }

    /// Highlights the suggestion a page down; returns whether the list
    /// shows.
    pub fn select_page_down(&mut self, cx: &mut Context<'_, Self>) -> bool {
        self.step(Move::PageDown, false, cx)
    }

    /// Highlights the suggestion a page up; returns whether the list
    /// shows.
    pub fn select_page_up(&mut self, cx: &mut Context<'_, Self>) -> bool {
        self.step(Move::PageUp, false, cx)
    }

    /// Highlights the first suggestion; returns whether the list shows.
    pub fn select_first(&mut self, cx: &mut Context<'_, Self>) -> bool {
        self.step(Move::First, false, cx)
    }

    /// Highlights the last suggestion; returns whether the list shows.
    pub fn select_last(&mut self, cx: &mut Context<'_, Self>) -> bool {
        self.step(Move::Last, false, cx)
    }

    /// Picks the highlighted suggestion and returns `true`. Without one it
    /// closes the list and returns `false`, leaving the typed text to the
    /// owner, such as to save it.
    pub fn confirm(&mut self, cx: &mut Context<'_, Self>) -> bool {
        let picked = self
            .list
            .highlighted()
            .filter(|_| self.open)
            .map(|option| option.id.clone());
        self.close(cx);
        match picked {
            Some(id) => {
                cx.emit(ComboboxEvent::Picked(id));
                true
            }
            None => false,
        }
    }

    /// Runs the filter again for data it reads that changed, keeping the
    /// highlight on the same suggestion; an open list with nothing left
    /// closes.
    pub fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        if !self.open {
            return;
        }
        self.filter(cx);
        if self.auto_highlight && self.list.highlight().is_none() {
            self.list.set_highlight(Some(0));
        }
        if self.list.options().is_empty() {
            self.close(cx);
        } else {
            self.sync_field(cx);
            cx.notify();
        }
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

    /// Whether the field takes edits, so suggestions may change it.
    fn usable(&self, cx: &App) -> bool {
        let input = self.input.read(cx);
        !input.is_disabled() && !input.is_read_only()
    }

    /// Replaces the suggestions with the filter's, keeping the highlight
    /// on the same suggestion.
    fn filter(&mut self, cx: &App) {
        let options = (self.filter)(self.input.read(cx));
        self.list.set_options(options);
    }

    /// The user edited the field: filter it, highlight afresh, and show
    /// the list while anything matches.
    fn edited(&mut self, cx: &mut Context<'_, Self>) {
        if !self.usable(cx) {
            return;
        }
        self.filter(cx);
        let first = self.auto_highlight.then_some(0);
        self.list.set_highlight(first);
        if self.list.options().is_empty() {
            self.close(cx);
        } else {
            self.show(cx);
        }
    }

    fn show(&mut self, cx: &mut Context<'_, Self>) {
        if !self.open {
            self.open = true;
            let anchor = self.popover.trigger_bounds().unwrap_or_default();
            self.popover.open(Anchor::Trigger(anchor), false);
            self.list.set_max_height(Some(self.max_height(None)));
            self.list.show_scrollbar();
        }
        self.sync_field(cx);
        cx.notify();
    }

    fn step(
        &mut self,
        step: Move,
        wrap: bool,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let step = if self.open {
            step
        } else {
            // Only Down and Up open a closed list, on its first or last
            // suggestion.
            let end = match step {
                Move::Next => Move::First,
                Move::Previous => Move::Last,
                _ => return false,
            };
            if !self.open(cx) {
                return false;
            }
            end
        };
        self.list.step(step, wrap);
        self.sync_field(cx);
        cx.notify();
        true
    }

    /// The list's height: eight rows at most, and no more than the window
    /// leaves below or above the field.
    fn max_height(&self, room: Option<Pixels>) -> Pixels {
        let rows = self.list.row_height() * VISIBLE_ROWS + px(PADDING * 2.0);
        room.map_or(rows, |room| rows.min(room - px(BORDER * 2.0)))
    }

    /// Tells the field what to report about the list.
    fn sync_field(&self, cx: &mut Context<'_, Self>) {
        let state = ListState {
            expanded: self.open,
            active: self
                .list
                .highlighted()
                .filter(|_| self.open)
                .map(|option| option.label.clone()),
        };
        self.input.update(cx, |input, cx| {
            input.set_list_state(Some(state), cx);
        });
    }
}

impl Owner for Combobox {
    fn option_list(&mut self) -> &mut OptionList {
        &mut self.list
    }

    fn hover_option(&mut self, id: &ItemId, cx: &mut Context<'_, Self>) {
        if let Some(row) = self.list.position(id)
            && self.list.set_highlight(Some(row))
        {
            self.sync_field(cx);
            cx.notify();
        }
    }

    fn pick_option(&mut self, id: &ItemId, cx: &mut Context<'_, Self>) {
        // The options may have changed since the frame the press landed on.
        if !self.open || self.list.position(id).is_none() {
            return;
        }
        self.close(cx);
        cx.emit(ComboboxEvent::Picked(id.clone()));
    }
}

impl Render for Combobox {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let tokens = self.tokens;
        let view = cx.entity().downgrade();
        let guard = self.popover.guard(move |_, _, cx| {
            let _ = view.update(cx, Combobox::close);
        });
        let viewport = window.viewport_size();
        let list = match (self.open, self.popover.trigger_bounds()) {
            (true, Some(field)) => {
                let width = field.size.width.max(MIN_WIDTH);
                let height =
                    self.list.content_height().min(self.max_height(None))
                        + px(BORDER * 2.0);
                let placement = place(
                    Anchor::Trigger(field),
                    area(width, height),
                    viewport,
                );
                self.list.set_max_height(Some(
                    self.max_height(Some(placement.max_height)),
                ));
                let options = self.list.render(
                    self.label.clone(),
                    tokens,
                    true,
                    true,
                    window,
                    cx,
                );
                let content = surface(tokens, size::PANEL_RADIUS)
                    .id(self.id.clone())
                    .occlude()
                    .w(width)
                    .child(options);
                self.popover.render(placement, viewport, content)
            }
            // The wrapper paints its bounds in this frame; draw the list in
            // the next.
            (true, None) => {
                window.request_animation_frame();
                None
            }
            (false, _) => None,
        };
        div()
            .absolute()
            .inset_0()
            .child(self.popover.trigger_probe())
            .child(guard)
            .children(list)
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests;
