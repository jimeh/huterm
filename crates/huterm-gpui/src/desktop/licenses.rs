//! The Licenses view the About panel opens: a searchable list of the open
//! source components this build ships, beside the selected one's license
//! texts. It draws a `huterm-ui` dialog in the window's dialog slot, in
//! the About panel's place, and Done or Escape returns to it. Every
//! keyboard operation arrives as a `Focused`-scope catalog command through
//! [`LicensesView::run`]: the dialog's content takes the `Palette` and
//! `TextField` key contexts, so the palette's list bindings and the text
//! bindings apply whether the search field or a button has focus. The
//! field verbs go on to the palette's bindings for the same keys, and Tab
//! and Shift-Tab
//! (`palette_expand`, `palette_previous_slot`) move through the dialog's
//! focus cycle. While a button has focus, the search's input handler
//! takes the platform's text input, composition included, and the first
//! text moves focus to the search; text edits on a button move it there
//! too, so the search shows what they change. The component list is a
//! `huterm-ui` `Command` over the search: the view filters the components,
//! and the list's highlight is the selected component, whose details show
//! beside it.

// User input picks positions here, so a stale index must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, Corners,
    ElementInputHandler, Entity, EventEmitter, FocusHandle, Focusable,
    FontWeight, InputHandler, MouseButton, Pixels, Point, Render, ScrollEvent,
    ScrollHandle, SharedString, Size, Subscription, Task,
    TextInputConfiguration, UTF16Selection, WeakEntity, Window, canvas, div,
    point, prelude::*, px, relative,
};
use huterm_protocol::{CommandError, CommandInvocation, CommandOutcome, ids};

use super::about::footer_button;
use super::key_hint::KeyHint;
use super::overlay::{OverlayColors, Swatch, key_cap, mono_font_family};
use super::palette::FORWARDED_TEXT_COMMANDS;
use super::refresh::FrameClock;
use crate::keymap::Platform;
use crate::licenses::{Catalog, Component, LoadError};
use crate::ui::animation::AnimationSchedule;
use crate::ui::list_scrollbar::ListScrollbar;
use huterm_ui::components::command::{Command, CommandEvent};
use huterm_ui::components::dialog::{Dialog, Modal};
use huterm_ui::components::input::{Changed, Chrome, Input};
use huterm_ui::option_list::ListOption;
use huterm_ui::typography::ui;

const PANEL_WIDTH: f32 = 860.0;
const PANEL_HEIGHT: f32 = 600.0;
/// The panel's top edge clears the macOS traffic lights in short windows.
const PANEL_TOP_INSET: f32 = 40.0;
/// The dialog's scrim keeps this margin around the panel.
const SCRIM_MARGIN: f32 = 16.0;
/// Fixed so the list scrolls by whole rows and smokes can address them.
const ROW_HEIGHT: f32 = 44.0;
/// How much of the visible text one Page Up or Page Down moves.
const PAGE_FRACTION: f32 = 0.9;

enum Load {
    Loading,
    Ready(Arc<Catalog>),
    Failed(LoadError),
}

/// Asks the owner to return to the About panel.
pub(super) struct Done;

/// Where keyboard focus is: the search field, which also takes typing, or
/// one of the buttons. Tab and Shift-Tab move it; Enter presses the focused
/// button. The list's keys work from any of them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LicensesFocus {
    /// The search field, which takes focus on opening.
    Search,
    Website,
    Copy,
    Done,
}

impl LicensesFocus {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Website => "website",
            Self::Copy => "copy",
            Self::Done => "done",
        }
    }
}

/// The buttons' focus handles; the search field has its own.
struct Buttons {
    website: FocusHandle,
    copy: FocusHandle,
    done: FocusHandle,
}

/// What the view shows, for smoke state and tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LicensesState {
    Loading,
    Unavailable,
    Ready {
        matches: usize,
        selected: Option<String>,
    },
}

pub(super) struct LicensesView {
    load: Load,
    input: Entity<Input>,
    /// The search text, lowercased and trimmed.
    query: String,
    /// The components that match the query, each named by its index in
    /// the catalog; the highlighted one is selected.
    list: Entity<Command>,
    /// The window's dialog slot, which the view's dialog draws in.
    modal: Modal,
    buttons: Buttons,
    /// The window's content size, which the panel fits.
    viewport: Size<Pixels>,
    /// The corners the scrim rounds to match the window's content.
    scrim_corners: Corners<Pixels>,
    /// The catalog index of the component whose texts the Copy button last
    /// copied.
    copied: Option<usize>,
    colors: OverlayColors,
    hints: bool,
    text_scroll: ScrollHandle,
    text_scrollbar: ListScrollbar,
    _load: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Done> for LicensesView {}

impl LicensesView {
    /// Opens the view in the window's dialog slot `modal` and starts `load`
    /// off the UI thread. `hints` shows key caps on the Done button and in
    /// the footer, and the component list's scroll indicator fades on
    /// `clock`.
    pub(super) fn new(
        modal: Modal,
        colors: OverlayColors,
        hints: bool,
        clock: &Rc<FrameClock>,
        load: impl FnOnce() -> Result<Catalog, LoadError> + Send + 'static,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            Input::new(
                "licenses-search",
                "Search licenses",
                Swatch::new(colors),
                cx,
            )
            .with_placeholder("Search licenses")
            .with_chrome(Chrome::Bare)
        });
        let view = cx.weak_entity();
        let list = cx.new(|cx| {
            clock.observe(cx);
            // The highlight shows the details, so the pointer only hovers.
            Command::new(
                "licenses-list",
                "Components",
                &input,
                Swatch::new(colors),
                cx,
            )
            .with_pointer_highlight(false)
            .with_row_height(ui(ROW_HEIGHT))
            .with_content(Rc::new(
                move |option: &ListOption,
                      highlighted,
                      _: &mut Window,
                      cx: &mut App| {
                    component_row(&view, option, highlighted, cx)
                },
            ))
        });
        let edits = cx.subscribe_in(
            &input,
            window,
            |view, input, _: &Changed, window, cx| {
                let query = input.read(cx).text().trim().to_lowercase();
                if query != view.query {
                    view.query = query;
                    view.refilter(cx);
                    view.keep_focus_available(window, cx);
                }
            },
        );
        let selections = cx.subscribe_in(
            &list,
            window,
            |view, _, event: &CommandEvent, window, cx| {
                if let CommandEvent::Highlighted(_) = event {
                    view.selected(window, cx);
                }
            },
        );
        let task = cx.background_executor().spawn(async move { load() });
        let load = cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |view, cx| view.loaded(result, cx));
        });
        let text_scroll = ScrollHandle::new();
        Self {
            load: Load::Loading,
            input,
            query: String::new(),
            list,
            modal,
            buttons: Buttons {
                website: cx.focus_handle(),
                copy: cx.focus_handle(),
                done: cx.focus_handle(),
            },
            viewport: Size::default(),
            scrim_corners: Corners::default(),
            copied: None,
            colors,
            hints,
            text_scroll: text_scroll.clone(),
            text_scrollbar: ListScrollbar::new(text_scroll),
            _load: load,
            _subscriptions: vec![edits, selections],
        }
    }

    pub(super) fn state(&self, cx: &App) -> LicensesState {
        match &self.load {
            Load::Loading => LicensesState::Loading,
            Load::Failed(_) => LicensesState::Unavailable,
            Load::Ready(_) => LicensesState::Ready {
                matches: self.list.read(cx).options().len(),
                selected: self
                    .selected_component(cx)
                    .map(|item| item.name.clone()),
            },
        }
    }

    /// The control that has focus, if one of the view's does.
    pub(super) fn focus(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<LicensesFocus> {
        let search = self.focus_handle(cx);
        [
            (LicensesFocus::Search, &search),
            (LicensesFocus::Website, &self.buttons.website),
            (LicensesFocus::Copy, &self.buttons.copy),
            (LicensesFocus::Done, &self.buttons.done),
        ]
        .into_iter()
        .find(|(_, handle)| handle.is_focused(window))
        .map(|(focus, _)| focus)
    }

    /// Sets the window content size the panel fits, and the corners the
    /// scrim rounds to match it.
    pub(super) fn set_viewport(
        &mut self,
        viewport: Size<Pixels>,
        scrim_corners: Corners<Pixels>,
    ) {
        self.viewport = viewport;
        self.scrim_corners = scrim_corners;
    }

    pub(super) fn set_presentation(
        &mut self,
        colors: OverlayColors,
        hints: bool,
        cx: &mut Context<'_, Self>,
    ) {
        self.colors = colors;
        self.hints = hints;
        self.input.update(cx, |input, cx| {
            input.set_tokens(Swatch::new(colors), cx);
        });
        self.list.update(cx, |list, cx| {
            list.set_tokens(Swatch::new(colors), cx);
        });
        cx.notify();
    }

    /// Runs one `Focused`-scope catalog command: Up and Down select, Page
    /// Up and Page Down scroll the license text, Tab and Shift-Tab move
    /// focus through the dialog's cycle of the search and the buttons, Enter
    /// presses the focused button, and Escape clears the search before
    /// returning to About.
    ///
    /// # Errors
    /// Reports `UnknownCommand` for anything outside that set.
    pub(super) fn run(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        match invocation.id {
            ids::PALETTE_SELECT_NEXT => {
                self.list.update(cx, Command::select_next);
            }
            ids::PALETTE_SELECT_PREVIOUS => {
                self.list.update(cx, Command::select_previous);
            }
            ids::PALETTE_PAGE_DOWN => self.scroll_text(1.0, cx),
            ids::PALETTE_PAGE_UP => self.scroll_text(-1.0, cx),
            ids::PALETTE_BACK => self.back(window, cx),
            ids::PALETTE_EXPAND | ids::PALETTE_NEXT_SLOT => {
                self.modal.focus_next(window, cx);
            }
            ids::PALETTE_PREVIOUS_SLOT => {
                self.modal.focus_previous(window, cx);
            }
            ids::PALETTE_CONFIRM => self.press_focused(window, cx),
            ids::PALETTE_POP => {}
            id if FORWARDED_TEXT_COMMANDS.contains(&id) => {
                // Copying changes nothing to show, so focus stays put.
                if id != ids::TEXT_COPY {
                    self.focus_search(window, cx);
                }
                self.input.update(cx, |field, cx| {
                    crate::ui::input::run(field, invocation, cx)
                })?;
            }
            other => return Err(CommandError::UnknownCommand(other)),
        }
        Ok(CommandOutcome::Completed)
    }

    fn loaded(
        &mut self,
        result: Result<Catalog, LoadError>,
        cx: &mut Context<'_, Self>,
    ) {
        self.load = match result {
            Ok(catalog) => Load::Ready(Arc::new(catalog)),
            Err(error) => Load::Failed(error),
        };
        self.refilter(cx);
    }

    fn catalog(&self) -> Option<&Catalog> {
        match &self.load {
            Load::Ready(catalog) => Some(catalog),
            Load::Loading | Load::Failed(_) => None,
        }
    }

    /// The selected component's index in the catalog.
    fn selected_index(&self, cx: &App) -> Option<usize> {
        self.list.read(cx).highlighted()?.id.parse().ok()
    }

    fn selected_component(&self, cx: &App) -> Option<&Component> {
        let index = self.selected_index(cx)?;
        self.catalog()?.components.get(index)
    }

    /// The search field.
    pub(super) fn search(&self) -> Entity<Input> {
        self.input.clone()
    }

    /// Moves focus to the search from a button, so it shows the edit that
    /// follows.
    fn focus_search(&self, window: &mut Window, cx: &mut App) {
        let search = self.focus_handle(cx);
        if !search.is_focused(window) {
            search.focus(window, cx);
        }
    }

    /// Recomputes the matches, keeping the selected component when it still
    /// matches and otherwise selecting the first, in view.
    fn refilter(&mut self, cx: &mut Context<'_, Self>) {
        let options = self.catalog().map_or_else(Vec::new, |catalog| {
            catalog
                .components
                .iter()
                .enumerate()
                .filter(|(_, component)| component.matches(&self.query))
                .map(|(index, component)| {
                    let title = match &component.version {
                        Some(version) => {
                            format!("{} {version}", component.name)
                        }
                        None => component.name.clone(),
                    };
                    ListOption::new(index.to_string(), title)
                        .description(component.license.clone())
                })
                .collect()
        });
        let empty = format!("No matches for “{}”", self.query);
        self.list.update(cx, |list, cx| {
            list.set_empty(empty, cx);
            list.set_options(options, cx);
            list.reveal_highlight(cx);
        });
        self.copied = None;
        cx.notify();
    }

    /// Shows another selected component: its texts from the top, and focus
    /// back in the search when the button that had it no longer shows.
    fn selected(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.copied = None;
        self.keep_focus_available(window, cx);
        self.reset_text();
        cx.notify();
    }

    /// Whether the selected component shows the Website and Copy buttons.
    fn shows_buttons(&self, cx: &App) -> (bool, bool) {
        self.selected_component(cx)
            .map_or((false, false), |component| (component.url.is_some(), true))
    }

    /// Returns focus to the search when the button that has it no longer
    /// shows.
    fn keep_focus_available(&self, window: &mut Window, cx: &mut App) {
        let (website, copy) = self.shows_buttons(cx);
        if (!website && self.buttons.website.is_focused(window))
            || (!copy && self.buttons.copy.is_focused(window))
        {
            self.focus_handle(cx).focus(window, cx);
        }
    }

    fn press_focused(&mut self, window: &Window, cx: &mut Context<'_, Self>) {
        match self.focus(window, cx) {
            None | Some(LicensesFocus::Search) => {}
            Some(LicensesFocus::Website) => {
                if let Some(url) = self
                    .selected_component(cx)
                    .and_then(|item| item.url.clone())
                {
                    cx.open_url(&url);
                }
            }
            Some(LicensesFocus::Copy) => self.copy_selected(cx),
            Some(LicensesFocus::Done) => cx.emit(Done),
        }
    }

    fn reset_text(&mut self) {
        self.text_scroll.set_offset(point(px(0.0), px(0.0)));
    }

    /// Scrolls the license text by `pages` visible heights.
    fn scroll_text(&mut self, pages: f32, cx: &mut Context<'_, Self>) {
        let height = f32::from(self.text_scroll.bounds().size.height);
        let max = f32::from(self.text_scroll.max_offset().y);
        let current = f32::from(self.text_scroll.offset().y);
        let next = (current - pages * height * PAGE_FRACTION).clamp(-max, 0.0);
        self.text_scroll.set_offset(point(px(0.0), px(next)));
        self.text_scrollbar.show();
        cx.notify();
    }

    /// Clears a search, focusing it so typing starts a new one, or asks to
    /// return to About when there is none.
    fn back(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.input.read(cx).is_empty() {
            cx.emit(Done);
        } else {
            self.input.update(cx, |input, cx| input.set_text("", cx));
            self.focus_handle(cx).focus(window, cx);
        }
    }

    fn copy_selected(&mut self, cx: &mut Context<'_, Self>) {
        let Some(component) = self.selected_component(cx) else {
            return;
        };
        let text = component
            .texts
            .iter()
            .map(|text| text.body.as_ref())
            .collect::<Vec<_>>()
            .join("\n\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = self.selected_index(cx);
        cx.notify();
    }

    /// An element over the search field that, while a button has focus,
    /// registers the search's input handler for that button, so text
    /// typed or composed there reaches the search. The field's bounds place
    /// the IME's candidate window.
    fn button_text_input(&self, cx: &App) -> impl IntoElement + use<> {
        let buttons = [
            self.buttons.website.clone(),
            self.buttons.copy.clone(),
            self.buttons.done.clone(),
        ];
        let search = self.focus_handle(cx);
        let input = self.input.clone();
        canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                if let Some(button) =
                    buttons.iter().find(|button| button.is_focused(window))
                {
                    window.handle_input(
                        button,
                        SearchFromButton::new(search, bounds, input),
                        cx,
                    );
                }
            },
        )
        .absolute()
        .inset_0()
    }

    fn advance(&mut self, now: Instant, cx: &mut Context<'_, Self>) {
        if self.text_scrollbar.advance(now) {
            cx.notify();
        }
    }
}

/// The search's own input handler, registered while a button has focus.
/// Text, marked text, or a paste moves focus to the search before it
/// applies, and a composition the button started continues there under the
/// search's handler, since the search keeps the marked text.
struct SearchFromButton {
    search: FocusHandle,
    input: ElementInputHandler<Input>,
}

impl SearchFromButton {
    fn new(
        search: FocusHandle,
        bounds: Bounds<Pixels>,
        input: Entity<Input>,
    ) -> Self {
        Self {
            search,
            input: ElementInputHandler::new(bounds, input),
        }
    }

    fn focus_search(&self, window: &mut Window, cx: &mut App) {
        if !self.search.is_focused(window) {
            self.search.focus(window, cx);
        }
    }
}

impl InputHandler for SearchFromButton {
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        self.input
            .selected_text_range(ignore_disabled_input, window, cx)
    }

    fn marked_text_range(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Range<usize>> {
        self.input.marked_text_range(window, cx)
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        self.input
            .text_for_range(range_utf16, adjusted_range, window, cx)
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.focus_search(window, cx);
        self.input
            .replace_text_in_range(replacement_range, text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.focus_search(window, cx);
        self.input.replace_and_mark_text_in_range(
            range_utf16,
            new_text,
            new_selected_range,
            window,
            cx,
        );
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        self.input.unmark_text(window, cx);
    }

    fn paste(
        &mut self,
        item: ClipboardItem,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.focus_search(window, cx);
        self.input.paste(item, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.input.bounds_for_range(range_utf16, window, cx)
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.input.character_index_for_point(point, window, cx)
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.input.set_selected_text_range(range_utf16, window, cx);
    }

    fn element_bounds(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.input.element_bounds(window, cx)
    }

    fn text_length_utf16(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.input.text_length_utf16(window, cx)
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        self.input.apple_press_and_hold_enabled()
    }

    fn accepts_text_input(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        self.input.accepts_text_input(window, cx)
    }

    fn text_input_editable_range(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Range<usize>> {
        self.input.text_input_editable_range(window, cx)
    }

    fn prefers_ime_for_printable_keys(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        self.input.prefers_ime_for_printable_keys(window, cx)
    }

    fn text_input_configuration(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> TextInputConfiguration {
        self.input.text_input_configuration(window, cx)
    }
}

impl Focusable for LicensesView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl super::refresh::Animated for LicensesView {
    fn animation_schedule(&self, now: Instant) -> AnimationSchedule {
        self.text_scrollbar.schedule(now)
    }

    fn advance_animation(
        &mut self,
        now: Instant,
        _frame: bool,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.advance(now, cx);
    }
}

/// A centred status line, with an optional muted detail below it.
fn message(swatch: Swatch, text: &str, detail: Option<String>) -> gpui::Div {
    div()
        .flex()
        .flex_1()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .p(px(24.0))
        .child(
            div()
                .text_size(ui(13.0))
                .text_color(swatch.muted)
                .child(text.to_owned()),
        )
        .when_some(detail, |message, detail| {
            message.child(
                div()
                    .max_w(px(520.0))
                    .text_size(ui(11.5))
                    .text_color(swatch.muted)
                    .font_family(mono_font_family())
                    .child(detail),
            )
        })
}

/// A component's row: its name and version over its license, from the
/// `option` the view lists for it, in `view`'s colors.
fn component_row(
    view: &WeakEntity<LicensesView>,
    option: &ListOption,
    highlighted: bool,
    cx: &App,
) -> AnyElement {
    let Some(view) = view.upgrade() else {
        return div().into_any_element();
    };
    let swatch = Swatch::new(view.read(cx).colors);
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .child(
            div()
                .truncate()
                .text_size(ui(12.5))
                .text_color(swatch.fg)
                .child(option.label.clone()),
        )
        .when_some(option.description.clone(), |row, license| {
            row.child(
                div()
                    .truncate()
                    .text_size(ui(11.0))
                    .text_color(if highlighted {
                        swatch.muted_on(swatch.selection)
                    } else {
                        swatch.muted
                    })
                    .child(license),
            )
        })
        .into_any_element()
}

impl LicensesView {
    fn render_list(&self, swatch: Swatch, cx: &App) -> gpui::Div {
        let search = self.focus_handle(cx);
        div()
            .flex_none()
            .h_full()
            .w(relative(0.32))
            .min_w(px(180.0))
            .max_w(px(260.0))
            .border_r_1()
            .border_color(swatch.line)
            // A press on a row keeps the default focus change from
            // happening, so it returns focus to the search from a button
            // here.
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                search.focus(window, cx);
            })
            .child(self.list.clone())
    }

    /// The selected component's name, version, license, and kind, with its
    /// Website and Copy buttons.
    fn detail_header(
        &self,
        component: &Component,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> gpui::Div {
        let mut facts = vec![component.kind.clone()];
        facts.push(match &component.used {
            Some(used) => format!("{}, used under {used}", component.license),
            None => component.license.clone(),
        });
        let copied =
            self.copied.is_some() && self.copied == self.selected_index(cx);
        let mut actions = div().flex().flex_none().items_center().gap(px(8.0));
        let view = cx.entity().downgrade();
        if let Some(url) = component.url.clone() {
            actions = actions.child(
                footer_button(
                    "license-website",
                    "Website",
                    swatch,
                    false,
                    None,
                )
                .focus(&self.buttons.website)
                .on_click(move |_, cx| cx.open_url(&url)),
            );
        }
        actions = actions.child(
            footer_button(
                "license-copy",
                if copied { "Copied" } else { "Copy" },
                swatch,
                false,
                None,
            )
            .focus(&self.buttons.copy)
            .on_click(move |_, cx| {
                let _ = view.update(cx, LicensesView::copy_selected);
            }),
        );
        div()
            .flex()
            .flex_none()
            .items_start()
            .gap(px(12.0))
            .px(px(18.0))
            .pt(px(14.0))
            .pb(px(12.0))
            .border_b_1()
            .border_color(swatch.line)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(ui(15.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(component.name.clone()),
                            )
                            .when_some(
                                component.version.clone(),
                                |line, version| {
                                    line.child(
                                        div()
                                            .font_family(mono_font_family())
                                            .text_size(ui(11.5))
                                            .text_color(swatch.muted)
                                            .child(version),
                                    )
                                },
                            ),
                    )
                    .child(
                        div()
                            .text_size(ui(12.0))
                            .text_color(swatch.muted)
                            .child(facts.join(" · ")),
                    ),
            )
            .child(actions)
    }

    fn render_detail(
        &mut self,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> gpui::Div {
        let Some(component) = self.selected_component(cx).cloned() else {
            return div().flex_1();
        };
        let header = self.detail_header(&component, swatch, cx);
        let mut texts = div()
            .id("license-text")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .px(px(18.0))
            .py(px(14.0))
            .overflow_y_scroll()
            .track_scroll(&self.text_scroll)
            .on_scroll(cx.listener(|view, _: &ScrollEvent, _, cx| {
                view.text_scrollbar.show();
                cx.notify();
            }));
        for text in &component.texts {
            texts = texts.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(ui(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(swatch.muted)
                            .child(text.title.clone()),
                    )
                    .child(
                        div()
                            .font_family(mono_font_family())
                            .text_size(ui(11.0))
                            .line_height(ui(15.0))
                            .text_color(swatch.fg)
                            .child(SharedString::from(Arc::clone(&text.body))),
                    ),
            );
        }
        let view = cx.entity();
        let scrollbar = self.text_scrollbar.elements(
            "licenses-text-scrollbar",
            swatch.scrollbar,
            &view,
            |view| &mut view.text_scrollbar,
            cx,
        );
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex_col()
            .child(header)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(texts)
                    .children(scrollbar),
            )
    }

    fn render_footer(
        &self,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> gpui::Div {
        let platform = Platform::current();
        let count = match &self.load {
            Load::Ready(catalog) if self.query.is_empty() => {
                format!("{} components", catalog.components.len())
            }
            Load::Ready(catalog) => format!(
                "{} of {} components",
                self.list.read(cx).options().len(),
                catalog.components.len()
            ),
            Load::Loading | Load::Failed(_) => String::new(),
        };
        let escape = self.hints.then(|| KeyHint::keys(&["escape"], platform));
        let view = cx.entity().downgrade();
        let mut hints = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(14.0))
            .text_size(ui(11.5))
            .text_color(swatch.muted)
            .child(count);
        if self.hints {
            for (keys, label) in [
                (&["up", "down"][..], "select"),
                (&["pageup", "pagedown"][..], "scroll"),
                (&["tab"][..], "buttons"),
            ] {
                hints = hints.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .children(keys.iter().map(|key| {
                            key_cap(&KeyHint::keys(&[key], platform), swatch)
                        }))
                        .child(label),
                );
            }
        }
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(10.0))
            .border_t_1()
            .border_color(swatch.line)
            .bg(swatch.footer())
            .child(hints)
            .child(
                footer_button(
                    "licenses-done",
                    "Done",
                    swatch,
                    true,
                    escape.as_ref(),
                )
                .focus(&self.buttons.done)
                .on_click(move |_, cx| {
                    let _ = view.update(cx, |_, cx| cx.emit(Done));
                }),
            )
    }
}

impl Render for LicensesView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let swatch = Swatch::new(self.colors);
        let body = match &self.load {
            Load::Loading => message(swatch, "Loading licenses…", None),
            Load::Failed(error) => message(
                swatch,
                "License information isn’t available in this build.",
                Some(error.to_string()),
            ),
            Load::Ready(_) => {
                self.list.update(cx, |list, cx| list.set_tokens(swatch, cx));
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_list(swatch, cx))
                    .child(self.render_detail(swatch, cx))
            }
        };
        let search = self.focus_handle(cx);
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(14.0))
            .px(px(16.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(swatch.line)
            .child(
                div()
                    .flex_none()
                    .text_size(ui(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Open Source Licenses"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .max_w(px(320.0))
                    .ml_auto()
                    .h(ui(28.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(swatch.line_strong)
                    .when(search.is_focused(window), |field| {
                        field.shadow(swatch.focus_ring())
                    })
                    .relative()
                    .bg(swatch.bg)
                    .text_size(ui(12.5))
                    .child(self.input.clone())
                    .child(self.button_text_input(cx)),
            );
        let (website, copy) = self.shows_buttons(cx);
        // The panel's top edge sits at the inset below the window's top, and
        // its height leaves the scrim's margin at the bottom.
        let top = px(PANEL_TOP_INSET - SCRIM_MARGIN);
        let height = (self.viewport.height
            - px(PANEL_TOP_INSET + SCRIM_MARGIN))
        .clamp(px(0.0), px(PANEL_HEIGHT));
        // The search's keys work from the buttons too: the list keys under
        // `Palette` and its text commands under `TextField`, which the
        // search takes in `run`.
        let content = crate::ui::input::context_with(&["Palette"])
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            // A press in the panel that focuses nothing, such as on a row or
            // the text, returns keys and keyboard focus to the search.
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if !window.default_prevented() {
                    search.focus(window, cx);
                }
            })
            .child(header)
            .child(body)
            .child(self.render_footer(swatch, cx));
        Dialog::new("licenses", "Open Source Licenses", &self.modal, swatch)
            .bare()
            .focusable(&self.focus_handle(cx))
            .when(website, |dialog| dialog.focusable(&self.buttons.website))
            .when(copy, |dialog| dialog.focusable(&self.buttons.copy))
            .focusable(&self.buttons.done)
            .w_full()
            .max_w(px(PANEL_WIDTH))
            .h(height)
            .mt(top)
            .rounded(px(10.0))
            .scrim_corners(self.scrim_corners)
            .child(content)
    }
}

/// What desktop tests observe of the license text's scrolling.
#[cfg(all(test, feature = "test-support"))]
impl LicensesView {
    /// The text's vertical scroll offset, its furthest offset, and its
    /// painted bounds.
    pub(super) fn text_scroll(&self) -> (Pixels, Pixels, gpui::Bounds<Pixels>) {
        (
            self.text_scroll.offset().y,
            self.text_scroll.max_offset().y,
            self.text_scroll.bounds(),
        )
    }

    /// Sets the search's text as an edit that arrives without moving focus.
    pub(super) fn set_search(&self, text: &str, cx: &mut Context<'_, Self>) {
        self.input.update(cx, |input, cx| input.set_text(text, cx));
    }

    /// The component list's scroll offset from its top, and its first row's
    /// label.
    pub(super) fn list_scroll(
        &self,
        cx: &App,
    ) -> (Pixels, Option<SharedString>) {
        let list = self.list.read(cx);
        let first = list.options().first().map(|option| option.label.clone());
        (list.scroll_position().0, first)
    }

    /// The center of the component list's row at `row`, as last laid out.
    pub(super) fn row_center(&self, row: u8, cx: &App) -> Point<Pixels> {
        let (offset, bounds) = self.list.read(cx).scroll_position();
        let row_height =
            ROW_HEIGHT * huterm_ui::typography::Typography::of(cx).scale;
        let y = 4.0 + row_height * (f32::from(row) + 0.5);
        point(bounds.center().x, bounds.top() + px(y) - offset)
    }

    /// The input handler the view registers while a button has focus, for
    /// platform input GPUI's tests cannot send through the window, such as
    /// marked text.
    pub(super) fn button_input_handler(&self, cx: &App) -> impl InputHandler {
        SearchFromButton::new(
            self.focus_handle(cx),
            Bounds::default(),
            self.input.clone(),
        )
    }
}
