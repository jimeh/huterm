//! Menu: a list of commands or options on a raised surface, opened from a
//! button, a right-click, or a select.
//!
//! Anatomy: rows of items (a label, then a shortcut, a count badge, or a
//! disabled hint on the right), separators, and button rows such as Edit's
//! Copy, Paste, and Select All. Rows overflowing the owner's `max_height`
//! scroll with the fading list scrollbar. The owner places it in a
//! [`crate::components::popover::Popover`].
//!
//! Keyboard: the menu takes focus and carries the [`KEY_CONTEXT`] key
//! context; the owner routes its commands to [`Menu::select_next`],
//! [`Menu::select_previous`], [`Menu::select_first`], [`Menu::select_last`],
//! [`Menu::select_right`], [`Menu::select_left`], [`Menu::confirm`], and
//! [`Menu::dismiss`]. Printable text selects the next item it starts, through
//! GPUI's input handler. A menu opened from the keyboard selects its first
//! item; a select's menu selects the value its trigger shows.
//!
//! Pointer: only a moving pointer selects a row, because GPUI's hover also
//! fires when layout puts a row under a resting pointer; a click picks.
//! Presses inside stop before what is beneath. GPUI's implicit keyboard
//! click is ignored, so only the routed commands act on keys.
//!
//! Accessibility: a `Menu` of `MenuItem` rows, or with [`Role::ListBox`] a
//! `ListBox` of selectable `ListBoxOption` rows, named by its label. The
//! selected row is the active descendant; disabled rows report it, and rows
//! with shortcuts report them.

use std::ops::Range;
use std::time::Instant;

use gpui::{
    App, Bounds, Context, ElementInputHandler, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, MouseMoveEvent, Pixels, Point,
    Render, ScrollHandle, SharedString, Styled, UTF16Selection, Window, canvas,
    div, prelude::*, px,
};

use crate::a11y::States;
use crate::animation::AnimationSchedule;
use crate::click::from_keyboard;
use crate::components::badge::{Tone, badge};
use crate::components::kbd;
use crate::components::separator;
use crate::components::surface::surface;
use crate::components::tooltip::Tooltip;
use crate::list_scrollbar::ListScrollbar;
use crate::listbox::{
    Button, Item, ItemId, Model, Row, Selection, remap_selection,
};
use crate::tokens::{Tokens, size};
use crate::typography::{DESIGN_REM, Typography, ui};

/// The key context a focused menu carries.
pub const KEY_CONTEXT: &str = "menu";

/// What a menu reports to its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEvent {
    /// An enabled item or button was chosen; the owner closes the menu.
    Picked(ItemId),
    /// Escape, Tab, or `menu_close`; the owner closes the menu and returns
    /// focus.
    Dismissed,
}

const ROW_HEIGHT: f32 = 28.0;
const BUTTON_ROW_HEIGHT: f32 = 30.0;
/// A one-point line with four points of margin above and below.
const SEPARATOR_HEIGHT: f32 = 9.0;
const PANEL_PADDING: f32 = 5.0;
const PANEL_BORDER: f32 = 1.0;
/// An item row's horizontal padding.
const ROW_PADDING: f32 = 10.0;
/// The space between an item's label and what sits beside it.
const ROW_GAP: f32 = 10.0;
/// The space before a shortcut or disabled hint.
const SIDE_INSET: f32 = 24.0;
/// Item labels' text size.
const TEXT_SIZE: f32 = 12.5;
/// Shortcuts' and disabled hints' text size.
const SIDE_TEXT_SIZE: f32 = 11.5;
/// A button row's buttons' text size.
const BUTTON_TEXT_SIZE: f32 = 12.0;
/// The space between a button row's label and buttons, and between buttons.
const BUTTON_GAP: f32 = 6.0;
/// A button row's padding after its last button.
const BUTTON_ROW_END: f32 = 4.0;
/// A count badge's horizontal padding and border, beyond its digits.
const BADGE_PADDING: f32 = 14.0;
/// The menu's default minimum width at the design size; command labels
/// never need more, so placement treats it, at the window's interface
/// scale ([`min_width`]), as the width.
pub const MIN_WIDTH: Pixels = px(264.0);

/// [`MIN_WIDTH`] at `window`'s interface scale: the width a menu with the
/// default minimum draws at, and so the width to place it at.
#[must_use]
pub fn min_width(window: &Window) -> Pixels {
    MIN_WIDTH * (window.rem_size() / DESIGN_REM)
}

/// Which accessibility roles a menu reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    /// A command menu: `Menu` with `MenuItem` rows.
    #[default]
    Menu,
    /// A select's options: `ListBox` with selectable `ListBoxOption` rows.
    ListBox,
}

/// The rendered menu. Presses inside stop before what is beneath, so a
/// click on a row cannot start a selection there. Focus the handle from
/// [`Focusable`] after mounting so [`KEY_CONTEXT`] bindings dispatch.
pub struct Menu {
    label: SharedString,
    role: Role,
    model: Model,
    selection: Option<Selection>,
    tokens: Tokens,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// The fading indicator shown while rows overflow `max_height`.
    scrollbar: ListScrollbar,
    max_height: Option<Pixels>,
    min_width: Pixels,
    /// The window's rem size at the last render, which reveals scroll by
    /// between renders.
    rem: Pixels,
}

impl std::fmt::Debug for Menu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Menu")
            .field("label", &self.label)
            .field("role", &self.role)
            .field("model", &self.model)
            .field("selection", &self.selection)
            .finish_non_exhaustive()
    }
}

impl EventEmitter<MenuEvent> for Menu {}

impl Focusable for Menu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Menu {
    /// A menu named `label` over `model`; `from_keyboard` selects the
    /// first enabled row, while a pointer-opened menu starts with no
    /// selection.
    pub fn new(
        label: impl Into<SharedString>,
        model: Model,
        tokens: Tokens,
        from_keyboard: bool,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let selection = from_keyboard.then(|| model.first()).flatten();
        let scroll = ScrollHandle::new();
        let mut scrollbar = ListScrollbar::new(scroll.clone());
        // An overflowing menu shows its indicator on opening, so the rows
        // beyond the edge are visible at a glance.
        scrollbar.show();
        Self {
            label: label.into(),
            role: Role::Menu,
            model,
            selection,
            tokens,
            focus: cx.focus_handle(),
            scroll,
            scrollbar,
            max_height: None,
            min_width: MIN_WIDTH,
            rem: DESIGN_REM,
        }
    }

    /// The menu reporting `role` to accessibility, such as a select's
    /// [`Role::ListBox`].
    #[must_use]
    pub fn with_role(mut self, role: Role) -> Self {
        self.role = role;
        self
    }

    /// The menu at least `width` wide instead of [`MIN_WIDTH`], such as a
    /// select's, which its owner sizes through [`select_width`]. Like
    /// [`MIN_WIDTH`], `width` is a design width that follows the interface
    /// scale.
    #[must_use]
    pub fn with_min_width(mut self, width: Pixels) -> Self {
        self.min_width = width;
        self
    }

    /// The rows.
    #[must_use]
    pub fn model(&self) -> &Model {
        &self.model
    }

    /// The content height before any scrolling: every row's height at the
    /// window's rem size `rem` plus the panel's padding and border.
    /// Placement uses it, so the menu sits flush against its anchor when
    /// it opens upward.
    #[must_use]
    pub fn content_height(model: &Model, rem: Pixels) -> Pixels {
        let rows = model
            .rows
            .iter()
            .map(|row| row_height(row, rem))
            .fold(Pixels::ZERO, |sum, height| sum + height);
        rows + px((PANEL_PADDING + PANEL_BORDER) * 2.0)
    }

    /// The width the rows ask for: the widest row's text, measured in the
    /// interface family at `window`'s interface scale, and what sits beside
    /// it, plus the row's and the panel's padding.
    #[must_use]
    pub fn content_width(model: &Model, window: &Window, cx: &App) -> Pixels {
        let mut style = window.text_style();
        style.font_family = Typography::of(cx).family;
        let rem = window.rem_size();
        let measure = |text: &str, size: f32| {
            if text.is_empty() {
                return 0.0;
            }
            let run = style.to_run(text.len());
            let line = window.text_system().shape_line(
                SharedString::from(text.to_owned()),
                ui(size).to_pixels(rem),
                &[run],
                None,
            );
            f32::from(line.width)
        };
        let row = |row: &Row| match row {
            Row::Item(item) => {
                let side = if let Some(count) = item.badge {
                    ROW_GAP
                        + measure(&count.to_string(), SIDE_TEXT_SIZE)
                        + BADGE_PADDING
                } else if item.enabled && !item.shortcut.is_empty() {
                    ROW_GAP
                        + SIDE_INSET
                        + measure(&kbd::text(&item.shortcut), SIDE_TEXT_SIZE)
                } else if let Some(hint) =
                    item.disabled_hint.as_deref().filter(|_| !item.enabled)
                {
                    ROW_GAP + SIDE_INSET + measure(hint, SIDE_TEXT_SIZE)
                } else {
                    0.0
                };
                ROW_PADDING * 2.0 + measure(&item.label, TEXT_SIZE) + side
            }
            Row::Separator => 0.0,
            Row::Buttons { label, buttons } => {
                let buttons: f32 = buttons
                    .iter()
                    .map(|button| {
                        measure(&button.label, BUTTON_TEXT_SIZE)
                            + ROW_PADDING * 2.0
                            + PANEL_BORDER * 2.0
                            + BUTTON_GAP
                    })
                    .sum();
                ROW_PADDING
                    + measure(label, TEXT_SIZE)
                    + BUTTON_GAP
                    + buttons
                    + BUTTON_ROW_END
            }
        };
        let widest = model.rows.iter().map(row).fold(0.0, f32::max);
        px((widest + (PANEL_PADDING + PANEL_BORDER) * 2.0).ceil())
    }

    /// Whether the rows overflow the menu's height and scroll, and whether
    /// the scroll indicator is drawn, for smoke state.
    #[must_use]
    pub fn scroll_state(&self) -> (bool, bool) {
        (
            self.scroll.max_offset().y > px(0.0),
            self.scrollbar.indicator_visible(),
        )
    }

    /// The selected row and button.
    #[must_use]
    pub fn selection(&self) -> Option<Selection> {
        self.selection
    }

    /// Replaces the rows, keeping the selection where it still resolves.
    /// An unchanged model is ignored, so owners may refresh it on every
    /// notification.
    pub fn set_model(&mut self, model: Model, cx: &mut Context<'_, Self>) {
        if self.model == model {
            return;
        }
        let before = self.selection.and_then(|selection| {
            row_span(&self.model, selection.row, self.rem)
        });
        self.selection = remap_selection(&self.model, self.selection, &model);
        self.model = model;
        // Only a selection the new rows moved scrolls; a refresh leaves a
        // list the user scrolled away from where it is.
        if let Some(selection) = self.selection
            && row_span(&self.model, selection.row, self.rem) != before
        {
            self.reveal(selection.row);
        }
        cx.notify();
    }

    /// The height from [`Placement::max_height`], beyond which the list
    /// scrolls.
    ///
    /// [`Placement::max_height`]: crate::components::popover::Placement::max_height
    pub fn set_max_height(
        &mut self,
        max_height: Option<Pixels>,
        cx: &mut Context<'_, Self>,
    ) {
        if self.max_height != max_height {
            self.max_height = max_height;
            if let Some(selection) = self.selection {
                self.reveal(selection.row);
            }
            self.scrollbar.show();
            cx.notify();
        }
    }

    /// Scrolls the fewest points that show `row` whole, with the panel's
    /// padding beside it, within the menu's height. The fixed row heights
    /// give its place before any layout. GPUI's `scroll_to_item` would wait
    /// for the next frame's layout, after the scrollbar has drawn from the
    /// old offset, and before the first frame it has no rows to scroll to.
    fn reveal(&self, row: usize) {
        let (Some(height), Some((top, bottom))) =
            (self.max_height, row_span(&self.model, row, self.rem))
        else {
            return;
        };
        let height = f32::from(height);
        let bottom = bottom + PANEL_PADDING * 2.0;
        let rows: f32 = self
            .model
            .rows
            .iter()
            .map(|row| f32::from(row_height(row, self.rem)))
            .sum();
        let max = (rows + PANEL_PADDING * 2.0 - height).max(0.0);
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

    fn set_selection(
        &mut self,
        selection: Option<Selection>,
        cx: &mut Context<'_, Self>,
    ) {
        if self.selection == selection {
            return;
        }
        self.selection = selection;
        if let Some(selection) = selection {
            self.reveal(selection.row);
            self.scrollbar.show();
        }
        cx.notify();
    }

    /// Selects the item `id`, such as the value a select shows, when it is
    /// enabled.
    pub fn select_item(&mut self, id: &str, cx: &mut Context<'_, Self>) {
        if let Some(selection) = self.model.selection_of(id) {
            self.set_selection(Some(selection), cx);
        }
    }

    /// Selects the next enabled row, wrapping.
    pub fn select_next(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.model.next(self.selection), cx);
    }

    /// Selects the previous enabled row, wrapping.
    pub fn select_previous(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.model.previous(self.selection), cx);
    }

    /// Selects the first enabled row.
    pub fn select_first(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.model.first(), cx);
    }

    /// Selects the last enabled row.
    pub fn select_last(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.model.last(), cx);
    }

    /// Selects the next button in a button row, wrapping.
    pub fn select_right(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.model.right(self.selection), cx);
    }

    /// Selects the previous button in a button row, wrapping.
    pub fn select_left(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.model.left(self.selection), cx);
    }

    /// Jumps to the next item starting with `text`; unmatched text leaves
    /// the selection alone.
    pub fn type_ahead(&mut self, text: &str, cx: &mut Context<'_, Self>) {
        if let Some(selection) = self.model.type_ahead(self.selection, text) {
            self.set_selection(Some(selection), cx);
        }
    }

    /// Runs the selection; nothing happens without one.
    pub fn confirm(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(id) = self.selection.and_then(|s| self.model.id_at(s)) {
            cx.emit(MenuEvent::Picked(id));
        }
    }

    /// Reports a pointer pick of `id` when the current rows still enable
    /// it: the model may have changed since the frame the press landed on.
    fn pick(&self, id: &ItemId, cx: &mut Context<'_, Self>) {
        // `selection_of` resolves enabled targets only.
        if self.model.selection_of(id).is_some() {
            cx.emit(MenuEvent::Picked(id.clone()));
        }
    }

    /// When the scroll indicator next needs a frame or a wake.
    #[must_use]
    pub fn schedule(&self, now: Instant) -> AnimationSchedule {
        self.scrollbar.schedule(now)
    }

    /// Advances the scroll indicator to `now`.
    pub fn advance(&mut self, now: Instant, cx: &mut Context<'_, Self>) {
        if self.scrollbar.advance(now) {
            cx.notify();
        }
    }

    /// Reports Escape, Tab, or `menu_close`.
    pub fn dismiss(cx: &mut Context<'_, Self>) {
        cx.emit(MenuEvent::Dismissed);
    }

    fn hover(
        &mut self,
        row: usize,
        button: Option<usize>,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(selection) = self.model.hover(row, button) {
            self.set_selection(Some(selection), cx);
        }
    }

    /// Renders `item` at `row`, at the zero-based `position` among the
    /// menu's items and buttons.
    fn render_item(
        &self,
        row: usize,
        item: &Item,
        position: usize,
        cx: &mut Context<'_, Self>,
    ) -> gpui::AnyElement {
        let tokens = self.tokens;
        let selected = self.selection == Some(Selection { row, button: None });
        let text = if selected {
            tokens.on_accent()
        } else if item.enabled {
            tokens.fg
        } else {
            tokens.dim
        };
        // Shortcuts stay opaque on the accent: any fade there falls below
        // 4.5:1. An enabled item's shortcut is secondary text.
        let side_text = if selected {
            tokens.on_accent()
        } else if item.enabled {
            tokens.muted
        } else {
            tokens.dim
        };
        let side = if let Some(count) = item.badge {
            Some(div().ml_auto().child(badge(
                ("menu-badge", row),
                count.to_string(),
                Tone::Warning,
                tokens,
            )))
        } else {
            let side = |content: gpui::Div| {
                div()
                    .ml_auto()
                    .pl(px(SIDE_INSET))
                    .text_size(ui(SIDE_TEXT_SIZE))
                    .text_color(side_text)
                    .whitespace_nowrap()
                    .child(content)
            };
            if item.enabled {
                (!item.shortcut.is_empty()).then(|| {
                    side(kbd::glyphs(&item.shortcut, px(11.0), side_text))
                })
            } else {
                item.disabled_hint
                    .clone()
                    .map(|hint| side(div().child(hint)))
            }
        };
        let id = item.id.clone();
        let enabled = item.enabled;
        let row_element = div()
            .id(("menu-item", row))
            .role(match self.role {
                Role::Menu => gpui::Role::MenuItem,
                Role::ListBox => gpui::Role::ListBoxOption,
            })
            .aria_label(item.label.clone())
            .aria_position_in_set(position)
            .when(self.role == Role::ListBox, |row| {
                row.aria_selected(selected)
            })
            .when(selected, StatefulInteractiveElement::aria_active_descendant)
            .when(!item.shortcut.is_empty(), |row| {
                row.aria_keyshortcuts(kbd::text(&item.shortcut))
            })
            .flex()
            .flex_none()
            .items_center()
            .gap(px(ROW_GAP))
            .h(ui(ROW_HEIGHT))
            .px(px(ROW_PADDING))
            .rounded(px(6.0))
            .text_color(text)
            .when(selected, |item| item.bg(tokens.accent))
            .when(enabled, |item| {
                item.active(|style| style.bg(tokens.accent_pressed()))
            })
            // Only a moving pointer selects: a menu that opens under a
            // resting pointer keeps the selection it opened with.
            .on_mouse_move(cx.listener(
                move |menu, _: &MouseMoveEvent, _, cx| {
                    menu.hover(row, None, cx);
                },
            ))
            .when(enabled, |item| {
                item.on_click(cx.listener(move |menu, event, _, cx| {
                    if !from_keyboard(event) {
                        menu.pick(&id, cx);
                    }
                }))
            })
            .child(div().truncate().child(item.label.clone()))
            .children(side);
        States {
            disabled: !enabled,
            ..States::default()
        }
        .apply(row_element)
        .into_any_element()
    }

    /// Renders a button row at `row`, its first button at the zero-based
    /// `position` among the menu's items and buttons.
    fn render_buttons(
        &self,
        row: usize,
        label: &str,
        buttons: &[Button],
        position: usize,
        cx: &mut Context<'_, Self>,
    ) -> gpui::AnyElement {
        let tokens = self.tokens;
        // The buttons sit in their own group: the gap between them does not
        // survive beside an auto margin in GPUI's layout.
        let mut group =
            div().flex().flex_none().items_center().gap(px(BUTTON_GAP));
        for (index, button) in buttons.iter().enumerate() {
            let selected = self.selection
                == Some(Selection {
                    row,
                    button: Some(index),
                });
            let id = button.id.clone();
            let enabled = button.enabled;
            let tooltip = button.tooltip.clone();
            let disabled = States {
                disabled: !enabled,
                ..States::default()
            };
            group = group.child(
                disabled.apply(
                    div()
                        .id(SharedString::from(format!(
                            "menu-button-{row}-{index}"
                        )))
                        .role(gpui::Role::MenuItem)
                        .aria_label(button.label.clone())
                        .aria_position_in_set(position + index)
                        .when(
                            selected,
                            StatefulInteractiveElement::aria_active_descendant,
                        )
                        .when_some(tooltip, |button, tooltip| {
                            button.tooltip(move |_, cx| {
                                Tooltip::view(tooltip.clone(), tokens, cx)
                            })
                        })
                        .flex_none()
                        .h(ui(24.0))
                        .px(px(10.0))
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(if selected {
                            gpui::transparent_black()
                        } else {
                            tokens.line
                        })
                        .text_size(ui(BUTTON_TEXT_SIZE))
                        .line_height(ui(22.0))
                        .text_color(if selected {
                            tokens.on_accent()
                        } else if enabled {
                            tokens.fg
                        } else {
                            tokens.dim
                        })
                        .when(selected, |button| button.bg(tokens.accent))
                        .when(enabled, |button| {
                            button.active(|style| {
                                style
                                    .bg(tokens.accent_pressed())
                                    .text_color(tokens.on_accent())
                            })
                        })
                        .on_mouse_move(cx.listener(
                            move |menu, _: &MouseMoveEvent, _, cx| {
                                menu.hover(row, Some(index), cx);
                            },
                        ))
                        .when(enabled, |button| {
                            button.on_click(cx.listener(
                                move |menu, event, _, cx| {
                                    if !from_keyboard(event) {
                                        menu.pick(&id, cx);
                                    }
                                },
                            ))
                        })
                        .child(button.label.clone()),
                ),
            );
        }
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(BUTTON_GAP))
            .h(ui(BUTTON_ROW_HEIGHT))
            .pl(px(ROW_PADDING))
            .pr(px(BUTTON_ROW_END))
            .child(
                div()
                    .flex_1()
                    .text_color(tokens.muted)
                    .child(label.to_owned()),
            )
            .child(group)
            .into_any_element()
    }
}

/// Type-ahead: the menu keeps no text, so every insertion is a fresh
/// prefix and the remaining methods report an empty document.
impl EntityInputHandler for Menu {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<'_, Self>) {}

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.type_ahead(text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        _: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) {
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> Option<usize> {
        None
    }
}

/// Where `row` sits in `model`'s rows at the rem size `rem`, top and
/// bottom, before scrolling.
fn row_span(model: &Model, row: usize, rem: Pixels) -> Option<(f32, f32)> {
    let height = f32::from(row_height(model.rows.get(row)?, rem));
    let top: f32 = model.rows[..row]
        .iter()
        .map(|row| f32::from(row_height(row, rem)))
        .sum();
    Some((top, top + height))
}

/// The width a select's menu opens at: as wide as its `trigger`, wider
/// where the rows of `model` need it, and no wider than `viewport` leaves
/// inside the popover margins, where long labels truncate.
#[must_use]
pub fn select_width(
    trigger: Pixels,
    model: &Model,
    viewport: Pixels,
    window: &Window,
    cx: &App,
) -> Pixels {
    let room =
        (viewport - crate::components::popover::MARGIN * 2.0).max(px(0.0));
    trigger
        .max(Menu::content_width(model, window, cx))
        .min(room)
}

/// A row's height at the rem size `rem`: text rows follow the interface
/// scale, and separators keep their hairline and margins.
fn row_height(row: &Row, rem: Pixels) -> Pixels {
    match row {
        Row::Item(_) => ui(ROW_HEIGHT).to_pixels(rem),
        Row::Separator => px(SEPARATOR_HEIGHT),
        Row::Buttons { .. } => ui(BUTTON_ROW_HEIGHT).to_pixels(rem),
    }
}

impl Render for Menu {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        self.rem = window.rem_size();
        let tokens = self.tokens;
        // Items and buttons form one set: AccessKit positions are
        // zero-based, and adapters read the set's size from the container.
        let mut position = 0;
        let rows: Vec<gpui::AnyElement> = self
            .model
            .rows
            .clone()
            .iter()
            .enumerate()
            .map(|(row, model_row)| match model_row {
                Row::Item(item) => {
                    position += 1;
                    self.render_item(row, item, position - 1, cx)
                }
                Row::Separator => separator::horizontal(tokens)
                    .mx(px(4.0))
                    .my(px(4.0))
                    .into_any_element(),
                Row::Buttons { label, buttons } => {
                    position += buttons.len();
                    let first = position - buttons.len();
                    self.render_buttons(row, label, buttons, first, cx)
                }
            })
            .collect();
        let set_size = position;
        let focus = self.focus.clone();
        let menu = cx.entity();
        let scrollbar = self.scrollbar.elements(
            "menu-scrollbar",
            tokens.scrollbar,
            &menu,
            |menu| &mut menu.scrollbar,
            cx,
        );
        surface(tokens, size::PANEL_RADIUS)
            .id("menu")
            .role(match self.role {
                Role::Menu => gpui::Role::Menu,
                Role::ListBox => gpui::Role::ListBox,
            })
            .aria_label(self.label.clone())
            .aria_size_of_set(set_size)
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .occlude()
            .min_w(self.min_width * (self.rem / DESIGN_REM))
            .text_size(ui(TEXT_SIZE))
            .text_color(tokens.fg)
            .flex()
            .flex_col()
            // Typed text reaches `replace_text_in_range` for type-ahead only
            // while the menu has focus. The canvas stays outside the scrolled
            // rows container so the scroll handle's child bounds are rows.
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| {
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, menu.clone()),
                            cx,
                        );
                    },
                )
                .absolute()
                .inset_0(),
            )
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_mouse_up(gpui::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_mouse_up(gpui::MouseButton::Right, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("menu-rows")
                            .p(px(PANEL_PADDING))
                            .flex()
                            .flex_col()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .when_some(self.max_height, Styled::max_h)
                            .on_scroll(cx.listener(|menu, _, _, cx| {
                                menu.scrollbar.show();
                                cx.notify();
                            }))
                            .children(rows),
                    )
                    .children(scrollbar),
            )
    }
}

#[cfg(test)]
mod height_tests {
    use gpui::px;

    use super::Menu;
    use crate::listbox::{Button, Item, Model, Row};
    use crate::typography::DESIGN_REM;

    #[test]
    fn the_content_height_sums_fixed_row_heights_and_panel_chrome() {
        let model = Model::new(vec![
            Row::Item(Item::new("a", "A")),
            Row::Separator,
            Row::Item(Item::new("b", "B").disabled(None)),
            Row::Buttons {
                label: "Edit".to_owned(),
                buttons: vec![Button::new("copy", "Copy")],
            },
        ]);
        // Two items, one separator, one button row, plus 5pt padding and a
        // 1pt border on both sides.
        assert_eq!(
            Menu::content_height(&model, DESIGN_REM),
            px(2.0 * 28.0 + 9.0 + 30.0 + 12.0)
        );
        assert_eq!(
            Menu::content_height(&Model::default(), DESIGN_REM),
            px(12.0)
        );
        // At twice the scale the text rows double; the separator and the
        // panel chrome keep their points.
        assert_eq!(
            Menu::content_height(&model, DESIGN_REM * 2.0),
            px(2.0 * 56.0 + 9.0 + 60.0 + 12.0)
        );
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use gpui::{
        Context, Entity, IntoElement, Modifiers, MouseButton, MouseUpEvent,
        PlatformInput, Render, TestAppContext, Window, div, point, prelude::*,
        px,
    };

    use std::cell::RefCell;
    use std::rc::Rc;

    use super::{Menu, MenuEvent, Role, select_width};
    use crate::listbox::{Button, Item, Model, Row};
    use crate::testing::{a11y_node, tokens};
    use crate::typography::DESIGN_REM;

    fn model() -> Model {
        Model::new(vec![
            Row::Item(Item::new("palette", "Command Palette")),
            Row::Separator,
            Row::Item(Item::new("new_tab", "New Tab")),
            Row::Item(
                Item::new("copy_dir", "Copy Directory Path")
                    .disabled(Some("Directory unknown".to_owned())),
            ),
            Row::Item(Item::new("quit", "Quit")),
            Row::Buttons {
                label: "Edit".to_owned(),
                buttons: vec![
                    Button::new("copy", "Copy"),
                    Button::new("paste", "Paste"),
                ],
            },
        ])
    }

    /// Draws its menu once it has one, as a select does when clicked.
    struct Host {
        menu: Option<Entity<Menu>>,
    }

    impl Render for Host {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut Context<'_, Self>,
        ) -> impl IntoElement {
            div().size_full().children(self.menu.clone())
        }
    }

    fn open(
        cx: &mut TestAppContext,
        role: Role,
    ) -> (Entity<Menu>, &mut gpui::VisualTestContext) {
        let (host, cx) = cx.add_window_view(|_, _| Host { menu: None });
        let menu = cx.update(|_, cx| {
            let menu = cx.new(|cx| {
                Menu::new("Actions", model(), tokens(), true, cx)
                    .with_role(role)
            });
            host.update(cx, |host, cx| {
                host.menu = Some(menu.clone());
                cx.notify();
            });
            menu
        });
        cx.run_until_parked();
        (menu, cx)
    }

    /// A menu at its own width, inside a box tests measure.
    struct Sized {
        menu: Entity<Menu>,
    }

    impl Render for Sized {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut Context<'_, Self>,
        ) -> impl IntoElement {
            div().flex().items_start().child(
                div()
                    .debug_selector(|| "menu-box".into())
                    .child(self.menu.clone()),
            )
        }
    }

    #[gpui::test]
    fn the_default_minimum_width_follows_the_interface_scale(
        cx: &mut TestAppContext,
    ) {
        let (host, cx) = cx.add_window_view(|_, cx| Sized {
            menu: cx.new(|cx| {
                let short = Model::new(vec![Row::Item(Item::new("a", "A"))]);
                Menu::new("Actions", short, tokens(), true, cx)
            }),
        });
        cx.run_until_parked();
        let width = |cx: &mut gpui::VisualTestContext| {
            cx.debug_bounds("menu-box").expect("drawn").size.width
        };
        let placed = cx.update(|window, _| super::min_width(window));
        assert_eq!((width(cx), placed), (px(264.0), px(264.0)));
        // Below the design size the menu shrinks with its text, so it
        // draws at the width its owner placed it at.
        let small = DESIGN_REM * (10.0 / 13.0);
        cx.update(|window, cx| {
            window.set_rem_size(small);
            host.update(cx, |_, cx| cx.notify());
        });
        cx.run_until_parked();
        let placed = cx.update(|window, _| super::min_width(window));
        assert!((f32::from(placed) - 264.0 * 10.0 / 13.0).abs() < 1e-3);
        // Layout rounds to whole pixels.
        assert!((width(cx) - placed).abs() < px(0.5), "{:?}", width(cx));
    }

    #[gpui::test]
    fn a_select_menu_is_as_wide_as_its_trigger_or_its_widest_row(
        cx: &mut TestAppContext,
    ) {
        let (_, cx) = cx.add_window_view(|_, _| Host { menu: None });
        let short = Model::new(vec![Row::Item(Item::new("top", "Top"))]);
        let long = Model::new(vec![
            Row::Item(Item::new("top", "Top")),
            Row::Item(Item::new(
                "long",
                "A label far longer than the trigger it opens from",
            )),
        ]);
        let trigger = px(150.0);
        let (narrow, wide, capped, measured) = cx.update(|window, cx| {
            (
                select_width(trigger, &short, px(1000.0), window, cx),
                select_width(trigger, &long, px(1000.0), window, cx),
                select_width(trigger, &long, px(240.0), window, cx),
                Menu::content_width(&long, window, cx),
            )
        });
        assert_eq!(narrow, trigger, "short rows keep the trigger's width");
        assert!(wide > trigger, "a long row widens it: {wide:?}");
        assert_eq!(wide, measured);
        // The window's margins bound it, and the long label truncates.
        assert_eq!(capped, px(240.0 - 12.0));
    }

    #[gpui::test]
    fn a_menu_under_a_resting_pointer_keeps_its_selection(
        cx: &mut TestAppContext,
    ) {
        let (host, cx) = cx.add_window_view(|_, _| Host { menu: None });
        // The pointer rests where a later row will draw.
        let resting = point(px(60.0), px(70.0));
        cx.simulate_mouse_move(resting, None, Modifiers::none());
        let menu = cx.update(|_, cx| {
            let menu =
                cx.new(|cx| Menu::new("Actions", model(), tokens(), true, cx));
            host.update(cx, |host, cx| {
                host.menu = Some(menu.clone());
                cx.notify();
            });
            menu
        });
        // A second frame settles hover under the resting pointer.
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let first = model().first();
        assert_eq!(cx.read(|cx| menu.read(cx).selection()), first);
        // Moving it selects the row under it.
        cx.simulate_mouse_move(
            resting + point(px(1.0), px(0.0)),
            None,
            Modifiers::none(),
        );
        let moved = cx.read(|cx| menu.read(cx).selection());
        assert!(moved.is_some() && moved != first, "{moved:?}");
    }

    #[gpui::test]
    fn a_command_menu_reports_menu_items(cx: &mut TestAppContext) {
        let (_, cx) = open(cx, Role::Menu);
        cx.update(|window, _| {
            let menu = &a11y_node(window, "menu")["aria"];
            assert_eq!(menu["role"], "Menu");
            assert_eq!(menu["label"], "Actions");
            // AccessKit positions are zero-based, and adapters read the
            // set's size from the container, not the items.
            assert_eq!(menu["size_of_set"], 6);
            let mut items: Vec<_> = crate::testing::a11y_nodes(window)
                .into_iter()
                .filter(|node| node["aria"]["role"] == "MenuItem")
                .map(|node| {
                    let aria = &node["aria"];
                    assert!(aria["size_of_set"].is_null(), "{aria}");
                    (
                        aria["position_in_set"].as_u64(),
                        aria["label"].as_str().map(str::to_owned),
                    )
                })
                .collect();
            items.sort();
            let labels = [
                "Command Palette",
                "New Tab",
                "Copy Directory Path",
                "Quit",
                "Copy",
                "Paste",
            ];
            let expected: Vec<_> = (0..)
                .zip(labels)
                .map(|(position, label)| {
                    (Some(position), Some(label.to_owned()))
                })
                .collect();
            assert_eq!(items, expected);
        });
    }

    #[gpui::test]
    fn a_select_menu_reports_its_selected_option(cx: &mut TestAppContext) {
        let (menu, cx) = open(cx, Role::ListBox);
        menu.update(cx, |menu, cx| menu.select_item("new_tab", cx));
        cx.run_until_parked();
        cx.update(|window, _| {
            assert_eq!(a11y_node(window, "menu")["aria"]["role"], "ListBox");
            let options: Vec<_> = crate::testing::a11y_nodes(window)
                .into_iter()
                .filter(|node| node["aria"]["role"] == "ListBoxOption")
                .collect();
            let selected: Vec<_> = options
                .iter()
                .filter(|node| node["aria"]["selected"] == true)
                .map(|node| node["aria"]["label"].clone())
                .collect();
            assert_eq!(selected, ["New Tab"]);
            let disabled: Vec<_> = options
                .iter()
                .filter(|node| node["aria"]["disabled"] == true)
                .map(|node| node["aria"]["label"].clone())
                .collect();
            assert_eq!(disabled, ["Copy Directory Path"]);
        });
    }

    /// `count` items, `row-0` onwards.
    fn items(count: usize) -> Vec<Row> {
        (0..count)
            .map(|row| {
                Row::Item(Item::new(format!("row-{row}"), format!("Row {row}")))
            })
            .collect()
    }

    /// A host drawing a menu of `rows`, at most 200 points tall, with `id`
    /// selected before its first frame, as a select opens on its value.
    fn long<'a>(
        cx: &'a mut TestAppContext,
        rows: Vec<Row>,
        id: &str,
    ) -> (Entity<Menu>, &'a mut gpui::VisualTestContext) {
        let (host, cx) = cx.add_window_view(|_, _| Host { menu: None });
        let id = id.to_owned();
        let menu = cx.update(|_, cx| {
            let menu = cx.new(|cx| {
                let mut menu =
                    Menu::new("Fonts", Model::new(rows), tokens(), false, cx);
                menu.select_item(&id, cx);
                menu.set_max_height(Some(px(200.0)), cx);
                menu
            });
            host.update(cx, |host, cx| {
                host.menu = Some(menu.clone());
                cx.notify();
            });
            menu
        });
        cx.run_until_parked();
        (menu, cx)
    }

    /// Whether the last frame drew `row` whole inside the scrolled rows.
    fn shows(menu: &Menu, row: usize) -> bool {
        let viewport = menu.scroll.bounds();
        let item = menu
            .scroll
            .bounds_for_item(row)
            .expect("the last frame laid the row out");
        let top = item.top() + menu.scroll.offset().y;
        top >= viewport.top() && top + item.size.height <= viewport.bottom()
    }

    #[gpui::test]
    fn a_menu_opens_scrolled_to_its_selection(cx: &mut TestAppContext) {
        let (menu, cx) = long(cx, items(40), "row-30");
        cx.read(|cx| {
            let menu = menu.read(cx);
            assert_eq!(menu.selection().map(|s| s.row), Some(30));
            assert!(shows(menu, 30), "offset {:?}", menu.scroll.offset());
        });
    }

    #[gpui::test]
    fn a_keyboard_step_scrolls_before_the_frame_draws_the_thumb(
        cx: &mut TestAppContext,
    ) {
        let (menu, cx) = long(cx, items(40), "row-0");
        // The scrollbar's thumb comes from the offset as the menu renders,
        // so a step must not leave the scroll to the frame's layout.
        let stepped = menu.update(cx, |menu, cx| {
            menu.select_last(cx);
            menu.scroll.offset()
        });
        cx.run_until_parked();
        cx.read(|cx| {
            let menu = menu.read(cx);
            assert_eq!(menu.scroll.offset(), stepped);
            assert!(shows(menu, 39), "offset {stepped:?}");
        });
    }

    #[gpui::test]
    fn the_reveal_follows_drawn_rows_of_every_height_both_ways(
        cx: &mut TestAppContext,
    ) {
        let mut rows = Vec::new();
        for (index, row) in items(30).into_iter().enumerate() {
            rows.push(row);
            if index % 5 == 4 {
                rows.push(Row::Separator);
            }
            if index == 12 {
                rows.push(Row::Buttons {
                    label: "Zoom".to_owned(),
                    buttons: vec![Button::new("zoom-in", "In")],
                });
            }
        }
        let (menu, cx) = long(cx, rows, "row-0");
        // Down past separators and the button row, back up to rows above
        // the view, then the ends, where the scroll limits would mask a
        // wrong height.
        for id in ["row-20", "row-8", "row-29", "row-0"] {
            menu.update(cx, |menu, cx| menu.select_item(id, cx));
            cx.run_until_parked();
            cx.read(|cx| {
                let menu = menu.read(cx);
                let row = menu.selection().expect("a selection").row;
                let offset = menu.scroll.offset();
                assert!(shows(menu, row), "{id} at offset {offset:?}");
            });
        }
    }

    #[gpui::test]
    fn a_replaced_model_reveals_the_moved_selection(cx: &mut TestAppContext) {
        let (menu, cx) = long(cx, items(40), "row-0");
        // The selected item moves to the end.
        let mut rows = items(40);
        rows.rotate_left(1);
        menu.update(cx, |menu, cx| menu.set_model(Model::new(rows), cx));
        cx.run_until_parked();
        cx.read(|cx| {
            let menu = menu.read(cx);
            assert_eq!(menu.selection().map(|s| s.row), Some(39));
            assert!(shows(menu, 39), "offset {:?}", menu.scroll.offset());
        });
    }

    #[gpui::test]
    fn a_refresh_leaves_a_scrolled_list_where_it_is(cx: &mut TestAppContext) {
        let (menu, cx) = long(cx, items(40), "row-0");
        let scrolled = point(px(0.0), px(-400.0));
        menu.update(cx, |menu, _| menu.scroll.set_offset(scrolled));
        cx.run_until_parked();
        // A changed label leaves the selection on its row.
        let mut rows = items(40);
        rows[5] = Row::Item(Item::new("row-5", "Row five"));
        menu.update(cx, |menu, cx| menu.set_model(Model::new(rows), cx));
        cx.run_until_parked();
        cx.read(|cx| assert_eq!(menu.read(cx).scroll.offset(), scrolled));
    }

    #[gpui::test]
    fn a_model_that_moves_the_selection_up_reveals_it(cx: &mut TestAppContext) {
        let (menu, cx) = long(cx, items(40), "row-30");
        // Shorter rows above leave the selection at its index but move it
        // above the view.
        let mut rows = items(40);
        rows[..10].fill(Row::Separator);
        menu.update(cx, |menu, cx| menu.set_model(Model::new(rows), cx));
        cx.run_until_parked();
        cx.read(|cx| {
            let menu = menu.read(cx);
            assert_eq!(menu.selection().map(|s| s.row), Some(30));
            assert!(shows(menu, 30), "offset {:?}", menu.scroll.offset());
        });
    }

    #[gpui::test]
    fn a_pick_checks_the_rows_at_release(cx: &mut TestAppContext) {
        let (menu, cx) = open(cx, Role::Menu);
        let picks = Rc::new(RefCell::new(Vec::new()));
        let seen = picks.clone();
        cx.update(|_, cx| {
            cx.subscribe(&menu, move |_, event: &MenuEvent, _| {
                seen.borrow_mut().push(event.clone());
            })
            .detach();
        });
        // Command Palette is the first row, under the panel's inset.
        let palette = point(px(40.0), px(20.0));
        cx.simulate_click(palette, Modifiers::none());
        assert_eq!(*picks.borrow(), [MenuEvent::Picked("palette".into())]);
        // A row disabled between the press and the release ignores the
        // release, even before a frame redraws it.
        cx.simulate_mouse_down(palette, MouseButton::Left, Modifiers::none());
        cx.update(|window, cx| {
            menu.update(cx, |menu, cx| {
                let mut rows = model().rows;
                rows[0] = Row::Item(
                    Item::new("palette", "Command Palette").disabled(None),
                );
                menu.set_model(Model::new(rows), cx);
            });
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    position: palette,
                    button: MouseButton::Left,
                    modifiers: Modifiers::none(),
                    click_count: 1,
                }),
                cx,
            );
        });
        cx.run_until_parked();
        assert_eq!(picks.borrow().len(), 1);
    }
}
