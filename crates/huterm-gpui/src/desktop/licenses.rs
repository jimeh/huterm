//! The Licenses view the About panel opens: a searchable list of the open
//! source components this build ships, beside the selected one's license
//! texts. It replaces the About panel in the same modal layer, and Done or
//! Escape returns to it. Every keyboard operation arrives as a
//! `Palette`-scope catalog command through [`LicensesView::run`]: the view
//! takes the `Palette` key context so the palette's list and text bindings
//! apply.

// User input picks positions here, so a stale index must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use std::sync::Arc;
use std::time::Instant;

use gpui::{
    App, ClickEvent, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, MouseButton, MouseDownEvent, MouseUpEvent, Render,
    ScrollHandle, ScrollWheelEvent, SharedString, Subscription, Task, Window,
    div, point, prelude::*, px, relative,
};
use huterm_protocol::{CommandError, CommandInvocation, CommandOutcome, ids};

use super::about::footer_button;
use super::key_hint::KeyHint;
use super::overlay::{
    OverlayColors, Swatch, key_cap, key_cap_on, mono_font_family, raised_panel,
    scrim,
};
use super::palette::FORWARDED_TEXT_COMMANDS;
use crate::keymap::Platform;
use crate::licenses::{Catalog, Component, LoadError};
use crate::ui::animation::AnimationSchedule;
use crate::ui::list_scrollbar::ListScrollbar;
use crate::ui::text_field::{Changed, TextField};

const PANEL_WIDTH: f32 = 860.0;
const PANEL_HEIGHT: f32 = 600.0;
const PANEL_TOP_INSET: f32 = 40.0;
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

/// Where keyboard focus is: the search field, which also takes typing and
/// list navigation, or one of the buttons. Tab and Shift-Tab move it; Enter
/// presses the focused button.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum LicensesFocus {
    #[default]
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
    input: Entity<TextField>,
    /// The search text, lowercased and trimmed.
    query: String,
    /// Indices of the catalog's components that match the query.
    matches: Vec<usize>,
    /// The selected position in `matches`.
    selected: usize,
    hover: Option<usize>,
    focus: LicensesFocus,
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
    /// Opens the view and starts `load` off the UI thread. `hints` shows
    /// key caps on the Done button and in the footer.
    pub(super) fn new(
        colors: OverlayColors,
        hints: bool,
        load: impl FnOnce() -> Result<Catalog, LoadError> + Send + 'static,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let input = cx
            .new(|cx| TextField::new("Search licenses", colors.foreground, cx));
        let subscription =
            cx.subscribe(&input, |view, input, _: &Changed, cx| {
                let query = input.read(cx).text().trim().to_lowercase();
                // Typing always goes to the search, so focus follows it.
                view.focus = LicensesFocus::Search;
                if query != view.query {
                    view.query = query;
                    view.refilter(cx);
                }
            });
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
            focus: LicensesFocus::Search,
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

    pub(super) fn focus(&self) -> LicensesFocus {
        self.focus
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
            input.set_foreground(colors.foreground, cx);
        });
        cx.notify();
    }

    /// Runs one `Palette`-scope catalog command: Up and Down select, Page
    /// Up and Page Down scroll the license text, Tab and Shift-Tab move
    /// focus between the search and the buttons, Enter presses the focused
    /// button, and Escape clears the search before returning to About.
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
            ids::PALETTE_SELECT_NEXT => self.move_selection(1, cx),
            ids::PALETTE_SELECT_PREVIOUS => self.move_selection(-1, cx),
            ids::PALETTE_PAGE_DOWN => self.scroll_text(1.0, cx),
            ids::PALETTE_PAGE_UP => self.scroll_text(-1.0, cx),
            ids::PALETTE_BACK => self.back(cx),
            ids::PALETTE_EXPAND | ids::PALETTE_NEXT_SLOT => {
                self.move_focus(true, cx);
            }
            ids::PALETTE_PREVIOUS_SLOT => self.move_focus(false, cx),
            ids::PALETTE_CONFIRM => self.press_focused(cx),
            ids::PALETTE_POP => {}
            id if FORWARDED_TEXT_COMMANDS.contains(&id) => {
                self.input.update(cx, |field, cx| {
                    field.run(invocation, window, cx)
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
        self.keep_focus_available();
        self.list_scroll.scroll_to_item(self.selected);
        cx.notify();
    }

    /// The focus stops in Tab order: the search, then the buttons the
    /// current state shows.
    fn focus_order(&self) -> Vec<LicensesFocus> {
        let mut order = vec![LicensesFocus::Search];
        if let Some(component) = self.selected_component() {
            if component.url.is_some() {
                order.push(LicensesFocus::Website);
            }
            order.push(LicensesFocus::Copy);
        }
        order.push(LicensesFocus::Done);
        order
    }

    /// Returns focus to the search when its button no longer shows.
    fn keep_focus_available(&mut self) {
        if !self.focus_order().contains(&self.focus) {
            self.focus = LicensesFocus::Search;
        }
    }

    fn move_focus(&mut self, forward: bool, cx: &mut Context<'_, Self>) {
        let order = self.focus_order();
        let index = order
            .iter()
            .position(|focus| *focus == self.focus)
            .unwrap_or(0);
        let next = if forward {
            index + 1
        } else {
            index + order.len().saturating_sub(1)
        };
        if let Some(focus) = next
            .checked_rem(order.len())
            .and_then(|next| order.get(next))
        {
            self.focus = *focus;
        }
        cx.notify();
    }

    fn press_focused(&mut self, cx: &mut Context<'_, Self>) {
        match self.focus {
            LicensesFocus::Search => {}
            LicensesFocus::Website => {
                if let Some(url) =
                    self.selected_component().and_then(|item| item.url.clone())
                {
                    cx.open_url(&url);
                }
            }
            LicensesFocus::Copy => self.copy_selected(cx),
            LicensesFocus::Done => cx.emit(Done),
        }
    }

    fn select(&mut self, position: usize, cx: &mut Context<'_, Self>) {
        if position >= self.matches.len() || position == self.selected {
            return;
        }
        self.selected = position;
        self.copied = None;
        self.keep_focus_available();
        self.reset_text();
        self.list_scroll.scroll_to_item(position);
        self.list_scrollbar.show();
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<'_, Self>) {
        let Some(last) = self.matches.len().checked_sub(1) else {
            return;
        };
        let target = self.selected.saturating_add_signed(delta).min(last);
        self.select(target, cx);
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

    /// Clears a search, or asks to return to About when there is none.
    fn back(&mut self, cx: &mut Context<'_, Self>) {
        if self.input.read(cx).is_empty() {
            cx.emit(Done);
        } else {
            self.input.update(cx, |input, cx| input.set_text("", cx));
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
                        cx.listener(move |view, _: &MouseDownEvent, _, cx| {
                            view.select(position, cx);
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
                            .text_color(swatch.muted)
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
        if let Some(url) = component.url.clone() {
            actions = actions.child(
                self.ring(
                    footer_button("license-website", swatch, false, false),
                    LicensesFocus::Website,
                    swatch,
                )
                .on_click(move |_: &ClickEvent, _, cx| cx.open_url(&url))
                .child("Website"),
            );
        }
        actions = actions.child(
            self.ring(
                footer_button("license-copy", swatch, false, false),
                LicensesFocus::Copy,
                swatch,
            )
            .on_click(cx.listener(|view, _: &ClickEvent, _, cx| {
                view.copy_selected(cx);
            }))
            .child(if copied { "Copied" } else { "Copy" }),
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

    /// `button` with the focus ring while keyboard focus is on `which`.
    fn ring(
        &self,
        button: gpui::Stateful<gpui::Div>,
        which: LicensesFocus,
        swatch: Swatch,
    ) -> gpui::Stateful<gpui::Div> {
        button.when(self.focus == which, |button| {
            button.shadow(swatch.focus_ring())
        })
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
            .bg(swatch.fg.opacity(0.03))
            .child(hints)
            .child(
                self.ring(
                    footer_button("licenses-done", swatch, true, self.hints),
                    LicensesFocus::Done,
                    swatch,
                )
                .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(Done)))
                .child("Done")
                .when(self.hints, |button| {
                    button.child(key_cap_on(
                        &KeyHint::keys(&["escape"], platform),
                        swatch.bg,
                    ))
                }),
            )
    }
}

impl Render for LicensesView {
    fn render(
        &mut self,
        _window: &mut Window,
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
                    .when(self.focus == LicensesFocus::Search, |field| {
                        field.shadow(swatch.focus_ring())
                    })
                    .bg(swatch.bg)
                    .text_size(px(12.5))
                    .child(self.input.clone()),
            );
        let input_focus = self.focus_handle(cx);
        scrim(swatch)
            .key_context("Palette")
            .flex()
            .items_center()
            .justify_center()
            // The top inset clears the macOS traffic lights in short windows.
            .pt(px(PANEL_TOP_INSET))
            .pb(px(16.0))
            .px(px(16.0))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_: &MouseUpEvent, _, cx| {
                cx.stop_propagation();
            })
            .on_scroll_wheel(|_: &ScrollWheelEvent, _, cx| {
                cx.stop_propagation();
            })
            .child(
                raised_panel(swatch, 10.0)
                    .id("licenses")
                    .w_full()
                    .max_w(px(PANEL_WIDTH))
                    .h_full()
                    .max_h(px(PANEL_HEIGHT))
                    .flex()
                    .flex_col()
                    .text_color(swatch.fg)
                    // A press anywhere in the panel returns keys and keyboard
                    // focus to the search; a button's click still acts.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(
                            move |view, _: &MouseDownEvent, window, cx| {
                                input_focus.focus(window, cx);
                                if view.focus != LicensesFocus::Search {
                                    view.focus = LicensesFocus::Search;
                                    cx.notify();
                                }
                            },
                        ),
                    )
                    .child(header)
                    .child(body)
                    .child(self.render_footer(swatch, cx)),
            )
    }
}
