//! The Licenses view the About panel opens: a searchable list of the open
//! source components this build ships, beside the selected one's license
//! texts. It draws a `huterm-ui` dialog in the window's dialog slot, in
//! the About panel's place, and Done or Escape returns to it. Every
//! keyboard operation arrives as a `Palette`-scope catalog command through
//! [`LicensesView::run`]: the dialog's content takes the `Palette` key
//! context, so the palette's list and text bindings apply whether the
//! search field or a button has focus, and Tab and Shift-Tab
//! (`palette_expand`, `palette_previous_slot`) move through the dialog's
//! focus cycle. Typing and text editing on a button move focus to the
//! search first, so the search shows what they change.

// User input picks positions here, so a stale index must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use std::sync::Arc;
use std::time::Instant;

use gpui::{
    App, ClipboardItem, Context, Corners, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, KeyDownEvent, MouseButton, Pixels, Render,
    ScrollHandle, ScrollWheelEvent, SharedString, Size, Subscription, Task,
    Window, div, point, prelude::*, px, relative,
};
use huterm_protocol::{CommandError, CommandInvocation, CommandOutcome, ids};

use super::about::footer_button;
use super::key_hint::KeyHint;
use super::overlay::{OverlayColors, Swatch, key_cap, mono_font_family};
use super::palette::FORWARDED_TEXT_COMMANDS;
use crate::keymap::Platform;
use crate::licenses::{Catalog, Component, LoadError};
use crate::ui::animation::AnimationSchedule;
use crate::ui::list_scrollbar::ListScrollbar;
use huterm_ui::components::dialog::{Dialog, Modal};
use huterm_ui::components::input::{Changed, Chrome, Input};

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
    /// Indices of the catalog's components that match the query.
    matches: Vec<usize>,
    /// The selected position in `matches`.
    selected: usize,
    hover: Option<usize>,
    /// The window's dialog slot, which the view's dialog draws in.
    modal: Modal,
    buttons: Buttons,
    /// The window's content size, which the panel fits.
    viewport: Size<Pixels>,
    /// The corners the scrim rounds to match the window's content.
    scrim_corners: Corners<Pixels>,
    /// The `matches` position whose texts the Copy button last copied.
    copied: Option<usize>,
    colors: OverlayColors,
    hints: bool,
    list_scroll: ScrollHandle,
    list_scrollbar: ListScrollbar,
    text_scroll: ScrollHandle,
    text_scrollbar: ListScrollbar,
    _load: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<Done> for LicensesView {}

impl LicensesView {
    /// Opens the view in the window's dialog slot `modal` and starts `load`
    /// off the UI thread. `hints` shows key caps on the Done button and in
    /// the footer.
    pub(super) fn new(
        modal: Modal,
        colors: OverlayColors,
        hints: bool,
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
        let subscription = cx.subscribe_in(
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
        let task = cx.background_executor().spawn(async move { load() });
        let load = cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |view, cx| view.loaded(result, cx));
        });
        let list_scroll = ScrollHandle::new();
        let text_scroll = ScrollHandle::new();
        Self {
            load: Load::Loading,
            input,
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            hover: None,
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
            list_scroll: list_scroll.clone(),
            list_scrollbar: ListScrollbar::new(list_scroll),
            text_scroll: text_scroll.clone(),
            text_scrollbar: ListScrollbar::new(text_scroll),
            _load: load,
            _subscriptions: vec![subscription],
        }
    }

    pub(super) fn state(&self) -> LicensesState {
        match &self.load {
            Load::Loading => LicensesState::Loading,
            Load::Failed(_) => LicensesState::Unavailable,
            Load::Ready(_) => LicensesState::Ready {
                matches: self.matches.len(),
                selected: self
                    .selected_component()
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
        cx.notify();
    }

    /// Runs one `Palette`-scope catalog command: Up and Down select, Page
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
            ids::PALETTE_SELECT_NEXT => self.move_selection(1, window, cx),
            ids::PALETTE_SELECT_PREVIOUS => {
                self.move_selection(-1, window, cx);
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
                self.focus_search(window, cx);
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

    fn selected_component(&self) -> Option<&Component> {
        let index = *self.matches.get(self.selected)?;
        self.catalog()?.components.get(index)
    }

    /// Moves focus to the search from a button, so it shows the edit that
    /// follows.
    fn focus_search(&self, window: &mut Window, cx: &mut App) {
        let search = self.focus_handle(cx);
        if !search.is_focused(window) {
            search.focus(window, cx);
        }
    }

    /// Types a printable key pressed on a button into the search, which
    /// takes focus; Space stays the button's, and shortcuts with Command or
    /// Control pass by.
    fn type_from_button(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let keystroke = &event.keystroke;
        let Some(text) = keystroke.key_char.as_deref() else {
            return;
        };
        if keystroke.modifiers.platform
            || keystroke.modifiers.control
            || keystroke.key == "space"
            || text.is_empty()
            || text.chars().any(char::is_control)
            || self
                .focus(window, cx)
                .is_none_or(|focus| focus == LicensesFocus::Search)
        {
            return;
        }
        self.focus_search(window, cx);
        self.input.update(cx, |input, cx| {
            input.replace(input.selection(), text, cx);
        });
        cx.stop_propagation();
    }

    /// Recomputes the matches, keeping the selected component when it still
    /// matches.
    fn refilter(&mut self, cx: &mut Context<'_, Self>) {
        let previous = self.matches.get(self.selected).copied();
        self.matches = self.catalog().map_or_else(Vec::new, |catalog| {
            catalog
                .components
                .iter()
                .enumerate()
                .filter(|(_, component)| component.matches(&self.query))
                .map(|(index, _)| index)
                .collect()
        });
        let kept = previous.and_then(|index| {
            self.matches.iter().position(|item| *item == index)
        });
        self.selected = kept.unwrap_or(0);
        if kept.is_none() {
            self.reset_text();
        }
        self.hover = None;
        self.copied = None;
        self.list_scroll.scroll_to_item(self.selected);
        cx.notify();
    }

    /// Whether the selected component shows the Website and Copy buttons.
    fn shows_buttons(&self) -> (bool, bool) {
        self.selected_component()
            .map_or((false, false), |component| (component.url.is_some(), true))
    }

    /// Returns focus to the search when the button that has it no longer
    /// shows.
    fn keep_focus_available(&self, window: &mut Window, cx: &mut App) {
        let (website, copy) = self.shows_buttons();
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
                if let Some(url) =
                    self.selected_component().and_then(|item| item.url.clone())
                {
                    cx.open_url(&url);
                }
            }
            Some(LicensesFocus::Copy) => self.copy_selected(cx),
            Some(LicensesFocus::Done) => cx.emit(Done),
        }
    }

    fn select(
        &mut self,
        position: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if position >= self.matches.len() || position == self.selected {
            return;
        }
        self.selected = position;
        self.copied = None;
        self.keep_focus_available(window, cx);
        self.reset_text();
        self.list_scroll.scroll_to_item(position);
        self.list_scrollbar.show();
        cx.notify();
    }

    fn move_selection(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(last) = self.matches.len().checked_sub(1) else {
            return;
        };
        let target = self.selected.saturating_add_signed(delta).min(last);
        self.select(target, window, cx);
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
        let Some(component) = self.selected_component() else {
            return;
        };
        let text = component
            .texts
            .iter()
            .map(|text| text.body.as_ref())
            .collect::<Vec<_>>()
            .join("\n\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(self.selected);
        cx.notify();
    }

    fn advance(&mut self, now: Instant, cx: &mut Context<'_, Self>) {
        // Both always advance; `|` keeps either from short-circuiting.
        if self.list_scrollbar.advance(now) | self.text_scrollbar.advance(now) {
            cx.notify();
        }
    }
}

impl Focusable for LicensesView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl super::refresh::Animated for LicensesView {
    fn animation_schedule(&self, now: Instant) -> AnimationSchedule {
        self.list_scrollbar
            .schedule(now)
            .merge(self.text_scrollbar.schedule(now))
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
                .text_size(px(13.0))
                .text_color(swatch.muted)
                .child(text.to_owned()),
        )
        .when_some(detail, |message, detail| {
            message.child(
                div()
                    .max_w(px(520.0))
                    .text_size(px(11.5))
                    .text_color(swatch.muted)
                    .font_family(mono_font_family())
                    .child(detail),
            )
        })
}

impl LicensesView {
    fn render_list(
        &mut self,
        catalog: &Catalog,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> gpui::Div {
        let mut list = div()
            .id("licenses-list")
            .size_full()
            .flex()
            .flex_col()
            .py(px(4.0))
            .overflow_y_scroll()
            .track_scroll(&self.list_scroll)
            .on_scroll_wheel(cx.listener(
                |view, _: &ScrollWheelEvent, _, cx| {
                    view.list_scrollbar.show();
                    cx.notify();
                },
            ));
        if self.matches.is_empty() {
            list = list.child(
                div()
                    .p(px(14.0))
                    .text_size(px(12.0))
                    .text_color(swatch.fg)
                    .child(format!("No matches for “{}”", self.query)),
            );
        }
        for (position, index) in self.matches.iter().copied().enumerate() {
            let Some(component) = catalog.components.get(index) else {
                continue;
            };
            let selected = position == self.selected;
            let hovered = self.hover == Some(position);
            let title = match &component.version {
                Some(version) => format!("{} {version}", component.name),
                None => component.name.clone(),
            };
            list = list.child(
                div()
                    .id(("license-row", position))
                    .flex_none()
                    .h(px(ROW_HEIGHT))
                    .mx(px(6.0))
                    .px(px(10.0))
                    .rounded(px(6.0))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .when(selected, |row| row.bg(swatch.selection))
                    .when(!selected && hovered, |row| row.bg(swatch.hover))
                    .on_hover(cx.listener(
                        move |view, hovering: &bool, _, cx| {
                            let hover = hovering.then_some(position);
                            if *hovering || view.hover == Some(position) {
                                view.hover = hover;
                                cx.notify();
                            }
                        },
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, window, cx| {
                            view.select(position, window, cx);
                        }),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(swatch.fg)
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.0))
                            .text_color(if selected {
                                swatch.muted_on(swatch.selection)
                            } else {
                                swatch.muted
                            })
                            .child(component.license.clone()),
                    ),
            );
        }
        let view = cx.entity();
        let scrollbar = self.list_scrollbar.elements(
            "licenses-list-scrollbar",
            swatch.scrollbar,
            &view,
            |view| &mut view.list_scrollbar,
            cx,
        );
        div()
            .relative()
            .flex_none()
            .h_full()
            .w(relative(0.32))
            .min_w(px(180.0))
            .max_w(px(260.0))
            .border_r_1()
            .border_color(swatch.line)
            .child(list)
            .children(scrollbar)
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
        let copied = self.copied == Some(self.selected);
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
                                    .text_size(px(15.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(component.name.clone()),
                            )
                            .when_some(
                                component.version.clone(),
                                |line, version| {
                                    line.child(
                                        div()
                                            .font_family(mono_font_family())
                                            .text_size(px(11.5))
                                            .text_color(swatch.muted)
                                            .child(version),
                                    )
                                },
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
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
        let Some(component) = self.selected_component().cloned() else {
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
            .on_scroll_wheel(cx.listener(
                |view, _: &ScrollWheelEvent, _, cx| {
                    view.text_scrollbar.show();
                    cx.notify();
                },
            ));
        for text in &component.texts {
            texts = texts.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(swatch.muted)
                            .child(text.title.clone()),
                    )
                    .child(
                        div()
                            .font_family(mono_font_family())
                            .text_size(px(11.0))
                            .line_height(px(15.0))
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
                self.matches.len(),
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
            .text_size(px(11.5))
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
            Load::Ready(catalog) => {
                let catalog = Arc::clone(catalog);
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_list(&catalog, swatch, cx))
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
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Open Source Licenses"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .max_w(px(320.0))
                    .ml_auto()
                    .h(px(28.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(swatch.line_strong)
                    .when(search.is_focused(window), |field| {
                        field.shadow(swatch.focus_ring())
                    })
                    .bg(swatch.bg)
                    .text_size(px(12.5))
                    .child(self.input.clone()),
            );
        let (website, copy) = self.shows_buttons();
        // The panel's top edge sits at the inset below the window's top, and
        // its height leaves the scrim's margin at the bottom.
        let top = px(PANEL_TOP_INSET - SCRIM_MARGIN);
        let height = (self.viewport.height
            - px(PANEL_TOP_INSET + SCRIM_MARGIN))
        .clamp(px(0.0), px(PANEL_HEIGHT));
        let content = div()
            .key_context("Palette")
            .on_key_down(cx.listener(Self::type_from_button))
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
}
