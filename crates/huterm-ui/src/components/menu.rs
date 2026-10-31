//! Menu: a list of commands or options on a raised surface, opened from a
//! button, a right-click, or a select.
//!
//! Anatomy: rows of items (a label, then a shortcut, a count badge, or a
//! disabled hint on the right), separators, button rows such as Edit's
//! Copy, Paste, and Select All, and submenu rows, which show `›` on the
//! right. Rows overflowing the owner's `max_height` scroll with the fading
//! list scrollbar. The owner places it in a
//! [`crate::components::popover::Popover`].
//!
//! Submenus drill in: opening one replaces the rows with a back row, `‹`
//! and the submenu's label, followed by the submenu's rows. The back row
//! returns to the parent rows with the submenu row selected. The open
//! submenu is kept by its row's id when the owner replaces the rows, and
//! closes when that row goes.
//!
//! Keyboard: the menu takes focus and carries the [`KEY_CONTEXT`] key
//! context; the owner routes its commands to [`Menu::select_next`],
//! [`Menu::select_previous`], [`Menu::select_first`], [`Menu::select_last`],
//! [`Menu::select_right`], [`Menu::select_left`], [`Menu::confirm`], and
//! [`Menu::dismiss`]. Enter or [`Menu::select_right`] on a submenu row opens
//! it and selects its first row; [`Menu::select_left`] or Enter on the back
//! row leaves it. Printable text selects the next item it starts, through
//! GPUI's input handler. A menu opened from the keyboard selects its first
//! item; a select's menu selects the value its trigger shows.
//!
//! Pointer: only a moving pointer selects a row, because GPUI's hover also
//! fires when layout puts a row under a resting pointer; a click picks, or
//! opens or leaves a submenu, on release. A submenu's rows carry their own
//! element ids, so the release that opens one never completes a click on
//! the row that appears under the pointer.
//! Presses inside stop before what is beneath. GPUI's implicit keyboard
//! click is ignored, so only the routed commands act on keys.
//!
//! Accessibility: a `Menu` of `MenuItem` rows, or with [`Role::ListBox`] a
//! `ListBox` of selectable `ListBoxOption` rows, named by its label. The
//! selected row is the active descendant; disabled rows report it, rows
//! with shortcuts report them, a submenu row reports itself collapsed, and
//! the back row is named "Back from" and the submenu's label.

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
/// The back row's id while a submenu shows. `‹` keeps it apart from any
/// owner's id.
const BACK: &str = "‹back";

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
    /// The open submenu's row id.
    submenu: Option<ItemId>,
    /// The rows drawn: `model`, or the open submenu's back row and rows.
    /// The selection indexes these.
    shown: Model,
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
            shown: model.clone(),
            model,
            submenu: None,
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

    /// The rows, submenus included.
    #[must_use]
    pub fn model(&self) -> &Model {
        &self.model
    }

    /// The id of the open submenu's row.
    #[must_use]
    pub fn submenu(&self) -> Option<&ItemId> {
        self.submenu.as_ref()
    }

    /// The id under the selection, among the rows the menu shows. The back
    /// row has an id of its own.
    #[must_use]
    pub fn selected_id(&self) -> Option<ItemId> {
        self.shown.id_at(self.selection?)
    }

    /// The content height before any scrolling: every row's height at the
    /// window's rem size `rem` plus the panel's padding and border, for the
    /// tallest of the top rows and each submenu's. Placement uses it, so
    /// the menu opens in the direction every level fits, and with
    /// [`Self::shown_height`] keeps its anchored edge where it is when a
    /// submenu opens.
    #[must_use]
    pub fn content_height(model: &Model, rem: Pixels) -> Pixels {
        levels(model)
            .map(|level| panel_height(&level, rem))
            .fold(Pixels::ZERO, Pixels::max)
    }

    /// The content height of the rows the menu shows now, the top rows or
    /// an open submenu's, measured as [`Self::content_height`] measures
    /// each level.
    #[must_use]
    pub fn shown_height(&self, rem: Pixels) -> Pixels {
        panel_height(&self.shown, rem)
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
            Row::Submenu { item, .. } => {
                ROW_PADDING * 2.0
                    + measure(&item.label, TEXT_SIZE)
                    + ROW_GAP
                    + SIDE_INSET
                    + measure(SUBMENU_MARK, SIDE_TEXT_SIZE)
            }
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
        // Each submenu's rows count too, after a back row that draws `‹`
        // before the submenu's label.
        let mut widest = model.rows.iter().map(row).fold(0.0, f32::max);
        for line in &model.rows {
            if let Row::Submenu { item, rows } = line {
                let back = ROW_PADDING * 2.0
                    + measure(BACK_MARK, TEXT_SIZE)
                    + BACK_GAP
                    + measure(&item.label, TEXT_SIZE);
                widest = rows.iter().map(row).fold(widest.max(back), f32::max);
            }
        }
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
    /// An open submenu stays open while a submenu row with its id remains,
    /// and otherwise the top rows show again. An unchanged model is
    /// ignored, so owners may refresh it on every notification.
    pub fn set_model(&mut self, model: Model, cx: &mut Context<'_, Self>) {
        if self.model == model {
            return;
        }
        let before = self.selection.and_then(|selection| {
            row_span(&self.shown, selection.row, self.rem)
        });
        let was_open = self.submenu.is_some();
        let submenu = self
            .submenu
            .take()
            .and_then(|id| level(&model, &id).map(|shown| (id, shown)));
        let closed = was_open && submenu.is_none();
        let shown = match submenu {
            Some((id, shown)) => {
                self.submenu = Some(id);
                shown
            }
            None => model.clone(),
        };
        self.selection = remap_selection(&self.shown, self.selection, &shown);
        self.shown = shown;
        self.model = model;
        if closed {
            self.scroll.set_offset(Point::default());
        }
        // Only a selection the new rows moved scrolls; a refresh leaves a
        // list the user scrolled away from where it is.
        if let Some(selection) = self.selection
            && row_span(&self.shown, selection.row, self.rem) != before
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
            (self.max_height, row_span(&self.shown, row, self.rem))
        else {
            return;
        };
        let height = f32::from(height);
        let bottom = bottom + PANEL_PADDING * 2.0;
        let rows: f32 = self
            .shown
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
        if let Some(selection) = self.shown.selection_of(id) {
            self.set_selection(Some(selection), cx);
        }
    }

    /// Selects the next enabled row, wrapping.
    pub fn select_next(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.shown.next(self.selection), cx);
    }

    /// Selects the previous enabled row, wrapping.
    pub fn select_previous(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.shown.previous(self.selection), cx);
    }

    /// Selects the first enabled row.
    pub fn select_first(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.shown.first(), cx);
    }

    /// Selects the last enabled row.
    pub fn select_last(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.shown.last(), cx);
    }

    /// Selects the next button in a button row, wrapping, or opens the
    /// selected submenu and selects its first row.
    pub fn select_right(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(id) = self.selected_submenu() {
            self.open_submenu(id, true, cx);
        } else {
            self.set_selection(self.shown.right(self.selection), cx);
        }
    }

    /// Selects the previous button in a button row, wrapping, or leaves an
    /// open submenu with its row selected.
    pub fn select_left(&mut self, cx: &mut Context<'_, Self>) {
        let in_buttons = self.selection.is_some_and(|selection| {
            matches!(
                self.shown.rows.get(selection.row),
                Some(Row::Buttons { .. })
            )
        });
        if self.submenu.is_some() && !in_buttons {
            self.leave_submenu(cx);
        } else {
            self.set_selection(self.shown.left(self.selection), cx);
        }
    }

    /// The selected row's id when it opens a submenu.
    fn selected_submenu(&self) -> Option<ItemId> {
        let selection = self.selection?;
        self.shown
            .is_submenu(selection)
            .then(|| self.shown.id_at(selection))
            .flatten()
    }

    /// Shows the submenu `id` in place of the rows, selecting its first
    /// row when the keyboard opened it and nothing when a pointer did.
    fn open_submenu(
        &mut self,
        id: ItemId,
        from_keyboard: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(shown) = level(&self.model, &id) else {
            return;
        };
        self.shown = shown;
        self.submenu = Some(id);
        self.selection = None;
        self.scroll.set_offset(Point::default());
        let first = from_keyboard
            .then(|| {
                self.shown.next(Some(Selection {
                    row: 0,
                    button: None,
                }))
            })
            .flatten();
        self.set_selection(first, cx);
        cx.notify();
    }

    /// Shows the top rows again with the submenu's row selected.
    fn leave_submenu(&mut self, cx: &mut Context<'_, Self>) {
        let Some(id) = self.submenu.take() else {
            return;
        };
        self.shown = self.model.clone();
        self.selection = None;
        self.scroll.set_offset(Point::default());
        self.set_selection(self.shown.selection_of(&id), cx);
        cx.notify();
    }

    /// Whether `selection` is the open submenu's back row.
    fn is_back(&self, selection: Selection) -> bool {
        self.submenu.is_some() && selection.row == 0
    }

    /// Jumps to the next item starting with `text`; unmatched text leaves
    /// the selection alone.
    pub fn type_ahead(&mut self, text: &str, cx: &mut Context<'_, Self>) {
        if let Some(selection) = self.shown.type_ahead(self.selection, text) {
            self.set_selection(Some(selection), cx);
        }
    }

    /// Runs the selection: picks it, opens its submenu, or leaves the open
    /// submenu from its back row. Nothing happens without one.
    pub fn confirm(&mut self, cx: &mut Context<'_, Self>) {
        let Some(selection) = self.selection else {
            return;
        };
        self.activate(selection, true, cx);
    }

    fn activate(
        &mut self,
        selection: Selection,
        from_keyboard: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(id) = self.shown.id_at(selection) else {
            return;
        };
        if self.is_back(selection) {
            self.leave_submenu(cx);
        } else if self.shown.is_submenu(selection) {
            self.open_submenu(id, from_keyboard, cx);
        } else {
            cx.emit(MenuEvent::Picked(id));
        }
    }

    /// Runs a pointer pick of `id` when the current rows still enable it:
    /// the model may have changed since the frame the press landed on.
    fn pick(&mut self, id: &ItemId, cx: &mut Context<'_, Self>) {
        // `selection_of` resolves enabled targets only.
        if let Some(selection) = self.shown.selection_of(id) {
            self.activate(selection, false, cx);
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
        if let Some(selection) = self.shown.hover(row, button) {
            self.set_selection(Some(selection), cx);
        }
    }

    /// Renders `item` at `row` as `kind`, at the zero-based `position`
    /// among the menu's items and buttons.
    fn render_item(
        &self,
        row: usize,
        item: &Item,
        kind: ItemKind,
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
        let side = self.render_side(row, item, kind, side_text);
        let id = item.id.clone();
        let enabled = item.enabled;
        // A submenu's rows have ids of their own, so a press on the top
        // rows never completes a click on the row that replaced it.
        let element_id = if self.submenu.is_some() {
            ("menu-subitem", row)
        } else {
            ("menu-item", row)
        };
        let aria_label = match kind {
            ItemKind::Back => format!("Back from {}", item.label),
            ItemKind::Item | ItemKind::Submenu => item.label.clone(),
        };
        let row_element = div()
            .id(element_id)
            .role(match self.role {
                Role::Menu => gpui::Role::MenuItem,
                Role::ListBox => gpui::Role::ListBoxOption,
            })
            .aria_label(aria_label)
            .aria_position_in_set(position)
            .when(kind == ItemKind::Submenu, |row| row.aria_expanded(false))
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
            .when(kind == ItemKind::Back, |row| row.child(BACK_MARK))
            .child(div().truncate().child(item.label.clone()))
            .children(side);
        States {
            disabled: !enabled,
            ..States::default()
        }
        .apply(row_element)
        .into_any_element()
    }

    /// What an item row draws on its right, in `color`: a count badge, `›`
    /// for a submenu, an enabled item's shortcut, or a disabled item's hint.
    fn render_side(
        &self,
        row: usize,
        item: &Item,
        kind: ItemKind,
        color: gpui::Hsla,
    ) -> Option<gpui::Div> {
        if let Some(count) = item.badge {
            return Some(div().ml_auto().child(badge(
                ("menu-badge", row),
                count.to_string(),
                Tone::Warning,
                self.tokens,
            )));
        }
        let side = |content: gpui::Div| {
            div()
                .ml_auto()
                .pl(px(SIDE_INSET))
                .text_size(ui(SIDE_TEXT_SIZE))
                .text_color(color)
                .whitespace_nowrap()
                .child(content)
        };
        if kind == ItemKind::Submenu {
            Some(side(div().child(SUBMENU_MARK)))
        } else if item.enabled {
            (!item.shortcut.is_empty())
                .then(|| side(kbd::glyphs(&item.shortcut, px(11.0), color)))
        } else {
            item.disabled_hint
                .clone()
                .map(|hint| side(div().child(hint)))
        }
    }

    /// Renders the rows the menu shows, with the size of the set their
    /// items and buttons form.
    fn render_rows(
        &self,
        cx: &mut Context<'_, Self>,
    ) -> (Vec<gpui::AnyElement>, usize) {
        // Items and buttons form one set: AccessKit positions are
        // zero-based, and adapters read the set's size from the container.
        let mut position = 0;
        let rows = self
            .shown
            .rows
            .iter()
            .enumerate()
            .map(|(row, model_row)| match model_row {
                Row::Item(item) => {
                    position += 1;
                    let kind = if self.submenu.is_some() && row == 0 {
                        ItemKind::Back
                    } else {
                        ItemKind::Item
                    };
                    self.render_item(row, item, kind, position - 1, cx)
                }
                Row::Submenu { item, .. } => {
                    position += 1;
                    let kind = ItemKind::Submenu;
                    self.render_item(row, item, kind, position - 1, cx)
                }
                Row::Separator => separator::horizontal(self.tokens)
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
        (rows, position)
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

/// How an item row draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ItemKind {
    Item,
    /// Opens a submenu: `›` in place of a shortcut.
    Submenu,
    /// Leaves the open submenu: `‹` before the submenu's label.
    Back,
}

/// The submenu row's mark, drawn as a shortcut is.
const SUBMENU_MARK: &str = "›";
/// The back row's mark, drawn as its label is.
const BACK_MARK: &str = "‹";
/// The space between the back row's mark and its label: the row's gap.
const BACK_GAP: f32 = ROW_GAP;

/// The rows the submenu `id` of `model` shows: its back row, labelled with
/// the submenu's label, then its rows. `None` once no enabled submenu row
/// has that id.
fn level(model: &Model, id: &str) -> Option<Model> {
    let (item, rows) = model.submenu(id)?;
    let mut shown = vec![Row::Item(Item::new(BACK, item.label.clone()))];
    shown.extend(rows.iter().cloned());
    Some(Model::new(shown))
}

/// The rows `model` can show: its own, then each submenu's.
fn levels(model: &Model) -> impl Iterator<Item = Model> + '_ {
    std::iter::once(model.clone()).chain(model.rows.iter().filter_map(|row| {
        match row {
            Row::Submenu { item, .. } => level(model, &item.id),
            _ => None,
        }
    }))
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

/// The panel's height for `model`'s rows at the rem size `rem`, before
/// scrolling: every row plus the panel's padding and border.
fn panel_height(model: &Model, rem: Pixels) -> Pixels {
    model
        .rows
        .iter()
        .map(|row| row_height(row, rem))
        .fold(px((PANEL_PADDING + PANEL_BORDER) * 2.0), |sum, height| {
            sum + height
        })
}

/// A row's height at the rem size `rem`: text rows follow the interface
/// scale, and separators keep their hairline and margins.
fn row_height(row: &Row, rem: Pixels) -> Pixels {
    match row {
        Row::Item(_) | Row::Submenu { .. } => ui(ROW_HEIGHT).to_pixels(rem),
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
        let (rows, set_size) = self.render_rows(cx);
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

    #[test]
    fn the_content_height_is_the_tallest_of_the_rows_and_each_submenu() {
        let submenu = |count: usize| Row::Submenu {
            item: Item::new("more", "More"),
            rows: (0..count)
                .map(|index| Row::Item(Item::new(format!("{index}"), "Row")))
                .collect(),
        };
        // Two top rows; the submenu shows a back row and its own.
        let short =
            Model::new(vec![Row::Item(Item::new("a", "A")), submenu(1)]);
        assert_eq!(
            Menu::content_height(&short, DESIGN_REM),
            px(2.0 * 28.0 + 12.0)
        );
        let tall = Model::new(vec![Row::Item(Item::new("a", "A")), submenu(4)]);
        assert_eq!(
            Menu::content_height(&tall, DESIGN_REM),
            px(5.0 * 28.0 + 12.0)
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

    /// Rows with an Arrange submenu second, whose rows are three layouts.
    fn with_submenu() -> Model {
        Model::new(vec![
            Row::Item(Item::new("balance", "Balance Panes")),
            Row::Submenu {
                item: Item::new("arrange", "Arrange Panes"),
                rows: vec![
                    Row::Item(Item::new("arrange:columns", "Equal Columns")),
                    Row::Item(
                        Item::new("arrange:tiny", "Tiny")
                            .disabled(Some("Holds at most 2 panes".to_owned())),
                    ),
                    Row::Item(Item::new("arrange:rows", "Equal Rows")),
                ],
            },
            Row::Item(Item::new("close", "Close Pane")),
        ])
    }

    /// A drawn menu over [`with_submenu`] and the events it reports.
    fn open_submenu_menu(
        cx: &mut TestAppContext,
        from_keyboard: bool,
    ) -> (
        Entity<Menu>,
        Rc<RefCell<Vec<MenuEvent>>>,
        &mut gpui::VisualTestContext,
    ) {
        let (host, cx) = cx.add_window_view(|_, _| Host { menu: None });
        let events = Rc::new(RefCell::new(Vec::new()));
        let seen = events.clone();
        let menu = cx.update(|_, cx| {
            let menu = cx.new(|cx| {
                Menu::new("Pane", with_submenu(), tokens(), from_keyboard, cx)
            });
            cx.subscribe(&menu, move |_, event: &MenuEvent, _| {
                seen.borrow_mut().push(event.clone());
            })
            .detach();
            host.update(cx, |host, cx| {
                host.menu = Some(menu.clone());
                cx.notify();
            });
            menu
        });
        cx.run_until_parked();
        (menu, events, cx)
    }

    #[gpui::test]
    fn the_keyboard_drills_into_a_submenu_and_back_to_its_row(
        cx: &mut TestAppContext,
    ) {
        let (menu, events, cx) = open_submenu_menu(cx, true);
        let state = |cx: &mut gpui::VisualTestContext| {
            cx.read(|cx| {
                let menu = menu.read(cx);
                (
                    menu.submenu().map(ToString::to_string),
                    menu.selected_id().map(|id| id.to_string()),
                )
            })
        };
        let some = |text: &str| Some(text.to_owned());
        menu.update(cx, Menu::select_next);
        assert_eq!(state(cx), (None, some("arrange")));
        // Right opens it on its first row.
        menu.update(cx, Menu::select_right);
        assert_eq!(state(cx), (some("arrange"), some("arrange:columns")));
        // Left returns with the submenu row selected.
        menu.update(cx, Menu::select_left);
        assert_eq!(state(cx), (None, some("arrange")));
        // Enter opens it too, and Enter on the back row leaves.
        menu.update(cx, Menu::confirm);
        assert_eq!(state(cx), (some("arrange"), some("arrange:columns")));
        menu.update(cx, Menu::select_first);
        menu.update(cx, Menu::confirm);
        assert_eq!(state(cx), (None, some("arrange")));
        // A submenu row is picked like any item, skipping disabled ones.
        menu.update(cx, |menu, cx| {
            menu.select_right(cx);
            menu.select_next(cx);
            menu.confirm(cx);
        });
        cx.run_until_parked();
        assert_eq!(
            *events.borrow(),
            [MenuEvent::Picked("arrange:rows".into())]
        );
    }

    #[gpui::test]
    fn the_release_that_opens_a_submenu_never_picks_the_row_under_it(
        cx: &mut TestAppContext,
    ) {
        let (menu, events, cx) = open_submenu_menu(cx, false);
        // The submenu row is the second; the submenu's first layout draws
        // there once it opens, below the back row.
        let second = point(px(40.0), px(48.0));
        cx.simulate_click(second, Modifiers::none());
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.read(|cx| {
            let menu = menu.read(cx);
            assert_eq!(menu.submenu().map(AsRef::as_ref), Some("arrange"));
            assert_eq!(menu.selection(), None, "a pointer opens unselected");
        });
        assert_eq!(*events.borrow(), []);
        // The next click there picks that layout, and one on the back row
        // above leaves.
        cx.simulate_click(second, Modifiers::none());
        assert_eq!(
            *events.borrow(),
            [MenuEvent::Picked("arrange:columns".into())]
        );
        cx.simulate_click(point(px(40.0), px(20.0)), Modifiers::none());
        cx.read(|cx| {
            let menu = menu.read(cx);
            assert_eq!(menu.submenu(), None);
            assert_eq!(menu.selected_id().as_deref(), Some("arrange"));
        });
    }

    #[gpui::test]
    fn a_refresh_keeps_the_open_submenu_until_its_row_goes(
        cx: &mut TestAppContext,
    ) {
        let (menu, _, cx) = open_submenu_menu(cx, true);
        menu.update(cx, |menu, cx| {
            menu.select_next(cx);
            menu.select_right(cx);
            menu.select_next(cx);
        });
        // Rows above it and in it change; the submenu and its selected
        // layout stay.
        let mut rows = with_submenu().rows;
        rows.insert(0, Row::Item(Item::new("split", "Split Right")));
        if let Some(Row::Submenu { rows, .. }) = rows.get_mut(2) {
            rows.insert(0, Row::Item(Item::new("arrange:dev", "dev")));
        }
        menu.update(cx, |menu, cx| menu.set_model(Model::new(rows), cx));
        cx.read(|cx| {
            let menu = menu.read(cx);
            assert_eq!(menu.submenu().map(AsRef::as_ref), Some("arrange"));
            assert_eq!(menu.selected_id().as_deref(), Some("arrange:rows"));
        });
        // Without the row, as when the tab stops being split, the top rows
        // show again.
        let rows = vec![Row::Item(Item::new("close", "Close Pane"))];
        menu.update(cx, |menu, cx| menu.set_model(Model::new(rows), cx));
        cx.read(|cx| {
            let menu = menu.read(cx);
            assert_eq!(menu.submenu(), None);
            assert_eq!(menu.selection(), None);
        });
    }

    #[gpui::test]
    fn a_submenu_row_reports_it_collapsed_and_the_back_row_names_it(
        cx: &mut TestAppContext,
    ) {
        let (menu, _, cx) = open_submenu_menu(cx, true);
        let labelled = |cx: &mut gpui::VisualTestContext, label: &str| {
            cx.update(|window, _| {
                crate::testing::a11y_nodes(window)
                    .into_iter()
                    .find(|node| node["aria"]["label"] == label)
                    .map(|node| node["aria"].clone())
            })
        };
        let row = labelled(cx, "Arrange Panes").expect("the submenu row");
        assert_eq!(row["expanded"], false, "{row}");
        menu.update(cx, |menu, cx| {
            menu.select_next(cx);
            menu.select_right(cx);
        });
        cx.run_until_parked();
        let back = labelled(cx, "Back from Arrange Panes").expect("back");
        assert_eq!(back["role"], "MenuItem");
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
