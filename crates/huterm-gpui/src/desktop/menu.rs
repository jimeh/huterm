//! The shared menu behind the window menu button and the tab context menu.
//!
//! [`MenuModel`] holds the rows, and its selection and [`place_menu`]
//! placement logic are pure so they can be tested without a window. [`Menu`]
//! is the GPUI entity that renders a model under the `menu` key context and
//! reports picks and dismissal as [`MenuEvent`]s. The owner anchors it into
//! its tree (an absolute wrapper at the placement origin), routes the
//! `menu_*` catalog commands to its methods, and handles outside clicks and
//! focus return itself. Printable text typed while the menu has focus
//! arrives through the same input-handler route as the palette's text field
//! and drives type-ahead.

use std::ops::Range;

use gpui::{
    App, Bounds, Context, ElementInputHandler, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, Pixels, Point, Render, ScrollHandle,
    SharedString, Size, Styled, UTF16Selection, Window, canvas, div, point,
    prelude::*, px,
};

use super::key_hint::KeyHint;
use super::overlay::{Swatch, TextTooltip, raised_panel};
use crate::ui::animation::AnimationSchedule;
use crate::ui::list_scrollbar::ListScrollbar;

/// Identifies a menu item or button to its owner.
pub(crate) type MenuItemId = &'static str;

/// One selectable line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MenuItem {
    pub(crate) id: MenuItemId,
    pub(crate) label: String,
    /// The right-aligned shortcut in the platform's spelling.
    pub(crate) shortcut: Option<KeyHint>,
    pub(crate) enabled: bool,
    /// Shown in place of the shortcut while disabled.
    pub(crate) disabled_hint: Option<String>,
    /// A count badge in place of the shortcut.
    pub(crate) badge: Option<usize>,
}

impl MenuItem {
    pub(crate) fn new(id: MenuItemId, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            shortcut: None,
            enabled: true,
            disabled_hint: None,
            badge: None,
        }
    }

    pub(crate) fn shortcut(mut self, shortcut: Option<KeyHint>) -> Self {
        self.shortcut = shortcut;
        self
    }

    pub(crate) fn disabled(mut self, hint: Option<String>) -> Self {
        self.enabled = false;
        self.disabled_hint = hint;
        self
    }

    pub(crate) fn badge(mut self, count: usize) -> Self {
        self.badge = Some(count);
        self
    }
}

/// One button in a [`MenuRow::Buttons`] row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MenuButton {
    pub(crate) id: MenuItemId,
    pub(crate) label: String,
    pub(crate) enabled: bool,
    /// Explains a disabled button on hover.
    pub(crate) tooltip: Option<String>,
}

impl MenuButton {
    pub(crate) fn new(id: MenuItemId, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            enabled: true,
            tooltip: None,
        }
    }

    pub(crate) fn disabled(mut self, tooltip: Option<String>) -> Self {
        self.enabled = false;
        self.tooltip = tooltip;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MenuRow {
    Item(MenuItem),
    Separator,
    /// A label followed by inline buttons, such as the Edit row.
    Buttons {
        label: String,
        buttons: Vec<MenuButton>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MenuModel {
    pub(crate) rows: Vec<MenuRow>,
}

/// A selected row, plus the button within a button row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MenuSelection {
    pub(crate) row: usize,
    pub(crate) button: Option<usize>,
}

impl MenuModel {
    pub(crate) fn new(rows: Vec<MenuRow>) -> Self {
        Self { rows }
    }

    /// The rendered height before any scrolling: every row's fixed height
    /// plus the panel's padding and border. Placement uses it so the menu
    /// sits flush against its anchor when it opens upward.
    pub(crate) fn height(&self) -> Pixels {
        let rows: f32 = self
            .rows
            .iter()
            .map(|row| match row {
                MenuRow::Item(_) => ROW_HEIGHT,
                MenuRow::Separator => SEPARATOR_HEIGHT,
                MenuRow::Buttons { .. } => BUTTON_ROW_HEIGHT,
            })
            .sum();
        px(rows + (PANEL_PADDING + PANEL_BORDER) * 2.0)
    }

    /// The selection entering `row` from the keyboard: the item, or the
    /// first enabled button. `None` for separators and disabled rows.
    fn enter_row(&self, row: usize) -> Option<MenuSelection> {
        match self.rows.get(row)? {
            MenuRow::Item(item) if item.enabled => {
                Some(MenuSelection { row, button: None })
            }
            MenuRow::Buttons { buttons, .. } => buttons
                .iter()
                .position(|button| button.enabled)
                .map(|button| MenuSelection {
                    row,
                    button: Some(button),
                }),
            _ => None,
        }
    }

    fn selectable_rows(&self) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|row| self.enter_row(*row).is_some())
            .collect()
    }

    /// The identifier under `selection`, when it names an enabled target.
    pub(crate) fn id_at(&self, selection: MenuSelection) -> Option<MenuItemId> {
        match (self.rows.get(selection.row)?, selection.button) {
            (MenuRow::Item(item), None) if item.enabled => Some(item.id),
            (MenuRow::Buttons { buttons, .. }, Some(index)) => buttons
                .get(index)
                .filter(|button| button.enabled)
                .map(|button| button.id),
            _ => None,
        }
    }

    /// Where the enabled item or button `id` sits, if it does.
    pub(crate) fn selection_of(&self, id: MenuItemId) -> Option<MenuSelection> {
        self.rows.iter().enumerate().find_map(|(row, model_row)| {
            let selection = match model_row {
                MenuRow::Item(item) if item.id == id => {
                    MenuSelection { row, button: None }
                }
                MenuRow::Buttons { buttons, .. } => MenuSelection {
                    row,
                    button: Some(
                        buttons.iter().position(|button| button.id == id)?,
                    ),
                },
                MenuRow::Item(_) | MenuRow::Separator => return None,
            };
            self.id_at(selection).map(|_| selection)
        })
    }

    pub(crate) fn first(&self) -> Option<MenuSelection> {
        self.selectable_rows()
            .first()
            .and_then(|row| self.enter_row(*row))
    }

    pub(crate) fn last(&self) -> Option<MenuSelection> {
        self.selectable_rows()
            .last()
            .and_then(|row| self.enter_row(*row))
    }

    /// The next enabled row below `current`, wrapping; the first row with no
    /// selection.
    pub(crate) fn next(
        &self,
        current: Option<MenuSelection>,
    ) -> Option<MenuSelection> {
        let rows = self.selectable_rows();
        let Some(current) = current else {
            return self.first();
        };
        let row = rows
            .iter()
            .find(|row| **row > current.row)
            .or_else(|| rows.first())?;
        self.enter_row(*row)
    }

    /// The next enabled row above `current`, wrapping; the last row with no
    /// selection.
    pub(crate) fn previous(
        &self,
        current: Option<MenuSelection>,
    ) -> Option<MenuSelection> {
        let rows = self.selectable_rows();
        let Some(current) = current else {
            return self.last();
        };
        let row = rows
            .iter()
            .rev()
            .find(|row| **row < current.row)
            .or_else(|| rows.last())?;
        self.enter_row(*row)
    }

    fn step_button(
        &self,
        current: Option<MenuSelection>,
        forward: bool,
    ) -> Option<MenuSelection> {
        let current = current?;
        let (MenuRow::Buttons { buttons, .. }, Some(index)) =
            (self.rows.get(current.row)?, current.button)
        else {
            return Some(current);
        };
        let enabled: Vec<usize> = buttons
            .iter()
            .enumerate()
            .filter(|(_, button)| button.enabled)
            .map(|(index, _)| index)
            .collect();
        let position = enabled.iter().position(|button| *button == index)?;
        let next = if forward {
            (position + 1) % enabled.len()
        } else {
            (position + enabled.len() - 1) % enabled.len()
        };
        Some(MenuSelection {
            row: current.row,
            button: Some(enabled[next]),
        })
    }

    /// The next enabled button in the current button row, wrapping. Item
    /// rows and an empty selection are unchanged.
    pub(crate) fn right(
        &self,
        current: Option<MenuSelection>,
    ) -> Option<MenuSelection> {
        self.step_button(current, true)
    }

    pub(crate) fn left(
        &self,
        current: Option<MenuSelection>,
    ) -> Option<MenuSelection> {
        self.step_button(current, false)
    }

    /// The selection for a pointer over `row` (and `button` within a button
    /// row); `None` when that target is disabled.
    pub(crate) fn hover(
        &self,
        row: usize,
        button: Option<usize>,
    ) -> Option<MenuSelection> {
        let selection = MenuSelection { row, button };
        self.id_at(selection).map(|_| selection)
    }

    /// The next enabled item after `current` whose label starts with
    /// `text`, case-insensitively, wrapping round to the current row last.
    pub(crate) fn type_ahead(
        &self,
        current: Option<MenuSelection>,
        text: &str,
    ) -> Option<MenuSelection> {
        let text = text.trim().to_lowercase();
        if text.is_empty() {
            return None;
        }
        let matches = |row: usize| match &self.rows[row] {
            MenuRow::Item(item) if item.enabled => {
                item.label.to_lowercase().starts_with(&text)
            }
            _ => false,
        };
        let start = current.map_or(0, |current| current.row + 1);
        let order =
            (start..self.rows.len()).chain(0..start.min(self.rows.len()));
        order
            .filter(|row| matches(*row))
            .map(|row| MenuSelection { row, button: None })
            .next()
    }
}

/// Where `selection` in `old` lands in `new`: on the same command, not the
/// same row, since rows that appear or disappear above it would otherwise
/// move it onto a different command. `None` once that command is gone or
/// disabled.
fn remap_selection(
    old: &MenuModel,
    selection: Option<MenuSelection>,
    new: &MenuModel,
) -> Option<MenuSelection> {
    let id = old.id_at(selection?)?;
    new.selection_of(id)
}

// ---- placement ------------------------------------------------------------

/// Where a menu is opened from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MenuAnchor {
    /// The bounds of the control that opened it, in window coordinates.
    Button(Bounds<Pixels>),
    /// The pointer position for a context menu.
    Pointer(Point<Pixels>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MenuPlacement {
    /// Top-left corner in window coordinates.
    pub(crate) origin: Point<Pixels>,
    /// The menu extends above its anchor.
    pub(crate) opens_up: bool,
    /// The menu's right edge sits on the anchor's right edge.
    pub(crate) align_right: bool,
    /// The height beyond which the menu scrolls.
    pub(crate) max_height: Pixels,
}

/// Distance kept between a menu and the window edges.
pub(crate) const MENU_MARGIN: Pixels = px(6.0);
/// Gap between a button anchor and its menu.
const ANCHOR_GAP: Pixels = px(4.0);

/// Places a menu of `menu` size in a window of `window` size. Button
/// anchors open below, or above from the lower half, and right-align from
/// the right half; `max_height` is the space left in the opening direction.
/// Pointer anchors open at the pointer, upward from the lower half, and
/// slide along the window to show every row: they scroll only when taller
/// than the window within its margins, which is then their `max_height`.
pub(crate) fn place_menu(
    anchor: MenuAnchor,
    menu: Size<Pixels>,
    window: Size<Pixels>,
) -> MenuPlacement {
    let clamp = |value: Pixels, extent: Pixels, limit: Pixels| {
        value.min(limit - extent - MENU_MARGIN).max(MENU_MARGIN)
    };
    let (opens_up, align_right, x, top_edge, bottom_edge) = match anchor {
        MenuAnchor::Button(bounds) => {
            let centre = bounds.center();
            let align_right = centre.x > window.width / 2.0;
            let x = if align_right {
                bounds.right() - menu.width
            } else {
                bounds.left()
            };
            (
                centre.y > window.height / 2.0,
                align_right,
                x,
                bounds.top() - ANCHOR_GAP,
                bounds.bottom() + ANCHOR_GAP,
            )
        }
        MenuAnchor::Pointer(position) => {
            let opens_up = position.y > window.height / 2.0;
            let max_height = (window.height - MENU_MARGIN * 2.0).max(px(0.0));
            let height = menu.height.min(max_height);
            let y = if opens_up {
                position.y - height
            } else {
                position.y
            };
            return MenuPlacement {
                origin: point(
                    clamp(position.x, menu.width, window.width),
                    clamp(y, height, window.height),
                ),
                opens_up,
                align_right: false,
                max_height,
            };
        }
    };
    let max_height = if opens_up {
        top_edge - MENU_MARGIN
    } else {
        window.height - bottom_edge - MENU_MARGIN
    }
    .max(px(0.0));
    let height = menu.height.min(max_height);
    let y = if opens_up {
        top_edge - height
    } else {
        bottom_edge
    };
    MenuPlacement {
        origin: point(
            clamp(x, menu.width, window.width),
            clamp(y, height, window.height),
        ),
        opens_up,
        align_right,
        max_height,
    }
}

// ---- view -----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MenuEvent {
    /// An enabled item or button was chosen; the owner closes the menu.
    Picked(MenuItemId),
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
/// The menu's minimum width; labels never need more, so placement treats it
/// as the width.
pub(crate) const MENU_MIN_WIDTH: Pixels = px(264.0);

/// The rendered menu. Presses inside stop before the terminal, so a click on
/// a row cannot start a selection. Focus the handle from [`Focusable`] after
/// mounting so `menu`-context bindings dispatch.
pub(crate) struct Menu {
    model: MenuModel,
    selection: Option<MenuSelection>,
    swatch: Swatch,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// The fading indicator shown while rows overflow `max_height`.
    scrollbar: ListScrollbar,
    max_height: Option<Pixels>,
}

impl EventEmitter<MenuEvent> for Menu {}

impl Focusable for Menu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Menu {
    /// A menu over `model`; `from_keyboard` selects the first enabled row,
    /// while a pointer-opened menu starts with no selection.
    pub(crate) fn new(
        model: MenuModel,
        swatch: Swatch,
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
            model,
            selection,
            swatch,
            focus: cx.focus_handle(),
            scroll,
            scrollbar,
            max_height: None,
        }
    }

    pub(crate) fn model(&self) -> &MenuModel {
        &self.model
    }

    /// Whether the rows overflow the menu's height and scroll, and whether
    /// the scroll indicator is drawn, for smoke state.
    pub(crate) fn scroll_state(&self) -> (bool, bool) {
        (
            self.scroll.max_offset().height > px(0.0),
            self.scrollbar.indicator_visible(),
        )
    }

    pub(crate) fn selection(&self) -> Option<MenuSelection> {
        self.selection
    }

    /// Replaces the rows, keeping the selection where it still resolves.
    /// An unchanged model is ignored, so owners may refresh it on every
    /// notification.
    pub(crate) fn set_model(
        &mut self,
        model: MenuModel,
        cx: &mut Context<'_, Self>,
    ) {
        if self.model == model {
            return;
        }
        self.selection = remap_selection(&self.model, self.selection, &model);
        self.model = model;
        cx.notify();
    }

    /// The height from [`MenuPlacement::max_height`], beyond which the list
    /// scrolls.
    pub(crate) fn set_max_height(
        &mut self,
        max_height: Option<Pixels>,
        cx: &mut Context<'_, Self>,
    ) {
        if self.max_height != max_height {
            self.max_height = max_height;
            self.scrollbar.show();
            cx.notify();
        }
    }

    fn set_selection(
        &mut self,
        selection: Option<MenuSelection>,
        cx: &mut Context<'_, Self>,
    ) {
        if self.selection == selection {
            return;
        }
        self.selection = selection;
        if let Some(selection) = selection {
            self.scroll.scroll_to_item(selection.row);
            self.scrollbar.show();
        }
        cx.notify();
    }

    pub(crate) fn select_next(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.model.next(self.selection), cx);
    }

    pub(crate) fn select_previous(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.model.previous(self.selection), cx);
    }

    pub(crate) fn select_first(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.model.first(), cx);
    }

    pub(crate) fn select_last(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.model.last(), cx);
    }

    pub(crate) fn select_right(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.model.right(self.selection), cx);
    }

    pub(crate) fn select_left(&mut self, cx: &mut Context<'_, Self>) {
        self.set_selection(self.model.left(self.selection), cx);
    }

    /// Jumps to the next item starting with `text`; unmatched text leaves
    /// the selection alone.
    pub(crate) fn type_ahead(
        &mut self,
        text: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(selection) = self.model.type_ahead(self.selection, text) {
            self.set_selection(Some(selection), cx);
        }
    }

    /// Runs the selection; nothing happens without one.
    pub(crate) fn confirm(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(id) = self.selection.and_then(|s| self.model.id_at(s)) {
            cx.emit(MenuEvent::Picked(id));
        }
    }

    /// Reports Escape, Tab, or `menu_close`.
    pub(crate) fn dismiss(cx: &mut Context<'_, Self>) {
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

    fn render_item(
        &self,
        row: usize,
        item: &MenuItem,
        cx: &mut Context<'_, Self>,
    ) -> gpui::AnyElement {
        let swatch = self.swatch;
        let selected =
            self.selection == Some(MenuSelection { row, button: None });
        let text = if selected {
            swatch.bg
        } else if item.enabled {
            swatch.fg
        } else {
            swatch.dim
        };
        let side_text = if selected {
            swatch.bg.opacity(0.72)
        } else {
            swatch.dim
        };
        let side = if let Some(count) = item.badge {
            Some(
                div()
                    .ml_auto()
                    .min_w(px(18.0))
                    .px(px(6.0))
                    .rounded(px(9.0))
                    .bg(swatch.warning)
                    .text_size(px(10.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(swatch.bg)
                    .text_center()
                    .child(count.to_string()),
            )
        } else {
            let side = |content: gpui::Div| {
                div()
                    .ml_auto()
                    .pl(px(24.0))
                    .text_size(px(11.5))
                    .text_color(side_text)
                    .whitespace_nowrap()
                    .child(content)
            };
            if item.enabled {
                item.shortcut
                    .as_ref()
                    .map(|hint| side(hint.element(px(11.0), side_text)))
            } else {
                item.disabled_hint
                    .clone()
                    .map(|hint| side(div().child(hint)))
            }
        };
        let id = item.id;
        let enabled = item.enabled;
        div()
            .id(("menu-item", row))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.0))
            .h(px(ROW_HEIGHT))
            .px(px(10.0))
            .rounded(px(6.0))
            .text_color(text)
            .when(selected, |item| item.bg(swatch.accent))
            .when(enabled, |item| {
                item.active(|style| style.bg(swatch.accent_pressed()))
            })
            .on_hover(cx.listener(move |menu, hovering: &bool, _, cx| {
                if *hovering {
                    menu.hover(row, None, cx);
                }
            }))
            .when(enabled, |item| {
                item.on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(MenuEvent::Picked(id));
                }))
            })
            .child(div().truncate().child(item.label.clone()))
            .children(side)
            .into_any_element()
    }

    fn render_buttons(
        &self,
        row: usize,
        label: &str,
        buttons: &[MenuButton],
        cx: &mut Context<'_, Self>,
    ) -> gpui::AnyElement {
        let swatch = self.swatch;
        // The buttons sit in their own group: the gap between them does not
        // survive beside an auto margin in GPUI's layout.
        let mut group = div().flex().flex_none().items_center().gap(px(6.0));
        for (index, button) in buttons.iter().enumerate() {
            let selected = self.selection
                == Some(MenuSelection {
                    row,
                    button: Some(index),
                });
            let id = button.id;
            let enabled = button.enabled;
            let tooltip = button.tooltip.clone();
            group = group.child(
                div()
                    .id(SharedString::from(format!(
                        "menu-button-{row}-{index}"
                    )))
                    .when_some(tooltip, |button, tooltip| {
                        button.tooltip(move |_, cx| {
                            TextTooltip::view(tooltip.clone(), swatch, cx)
                        })
                    })
                    .flex_none()
                    .h(px(24.0))
                    .px(px(10.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(if selected {
                        gpui::transparent_black()
                    } else {
                        swatch.line
                    })
                    .text_size(px(12.0))
                    .line_height(px(22.0))
                    .text_color(if selected {
                        swatch.bg
                    } else if enabled {
                        swatch.fg
                    } else {
                        swatch.dim
                    })
                    .when(selected, |button| button.bg(swatch.accent))
                    .when(enabled, |button| {
                        button.active(|style| {
                            style
                                .bg(swatch.accent_pressed())
                                .text_color(swatch.bg)
                        })
                    })
                    .on_hover(cx.listener(
                        move |menu, hovering: &bool, _, cx| {
                            if *hovering {
                                menu.hover(row, Some(index), cx);
                            }
                        },
                    ))
                    .when(enabled, |button| {
                        button.on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(MenuEvent::Picked(id));
                        }))
                    })
                    .child(button.label.clone()),
            );
        }
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .h(px(30.0))
            .pl(px(10.0))
            .pr(px(4.0))
            .child(
                div()
                    .flex_1()
                    .text_color(swatch.muted)
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

impl super::refresh::Animated for Menu {
    fn animation_schedule(&self, now: std::time::Instant) -> AnimationSchedule {
        self.scrollbar.schedule(now)
    }

    fn advance_animation(
        &mut self,
        now: std::time::Instant,
        _frame: bool,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.scrollbar.advance(now) {
            cx.notify();
        }
    }
}

impl Render for Menu {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let swatch = self.swatch;
        let rows: Vec<gpui::AnyElement> = self
            .model
            .rows
            .clone()
            .iter()
            .enumerate()
            .map(|(row, model_row)| match model_row {
                MenuRow::Item(item) => self.render_item(row, item, cx),
                MenuRow::Separator => div()
                    .flex_none()
                    .h(px(1.0))
                    .mx(px(4.0))
                    .my(px(4.0))
                    .bg(swatch.line)
                    .into_any_element(),
                MenuRow::Buttons { label, buttons } => {
                    self.render_buttons(row, label, buttons, cx)
                }
            })
            .collect();
        let focus = self.focus.clone();
        let menu = cx.entity();
        let scrollbar = self.scrollbar.elements(
            "menu-scrollbar",
            swatch.scrollbar,
            &menu,
            |menu| &mut menu.scrollbar,
            cx,
        );
        raised_panel(swatch, 9.0)
            .id("menu")
            .key_context("menu")
            .track_focus(&self.focus)
            .occlude()
            .min_w(MENU_MIN_WIDTH)
            .text_size(px(12.5))
            .text_color(swatch.fg)
            .flex()
            .flex_col()
            // Typed text reaches `replace_text_in_range` for type-ahead only
            // while the menu has focus. The canvas stays outside the scrolled
            // rows container so `scroll_to_item` indexes rows alone.
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
                            .on_scroll_wheel(cx.listener(|menu, _, _, cx| {
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
mod tests {
    use super::*;
    use gpui::size;

    fn item(id: MenuItemId, label: &str) -> MenuRow {
        MenuRow::Item(MenuItem::new(id, label))
    }

    fn model() -> MenuModel {
        MenuModel::new(vec![
            item("palette", "Command Palette…"),
            MenuRow::Separator,
            item("new_tab", "New Tab"),
            MenuRow::Item(
                MenuItem::new("copy_dir", "Copy Directory Path")
                    .disabled(Some("Directory unknown".to_owned())),
            ),
            MenuRow::Separator,
            MenuRow::Buttons {
                label: "Edit".to_owned(),
                buttons: vec![
                    MenuButton::new("copy", "Copy")
                        .disabled(Some("Nothing selected".to_owned())),
                    MenuButton::new("paste", "Paste"),
                    MenuButton::new("select_all", "Select All"),
                ],
            },
            item("fullscreen", "Toggle Fullscreen"),
            MenuRow::Separator,
            item("notices", "Show Notices"),
            item("quit", "Quit Huterm"),
        ])
    }

    #[expect(clippy::unnecessary_wraps, reason = "matches the API's results")]
    fn at(row: usize) -> Option<MenuSelection> {
        Some(MenuSelection { row, button: None })
    }

    #[expect(clippy::unnecessary_wraps, reason = "matches the API's results")]
    fn button(row: usize, button: usize) -> Option<MenuSelection> {
        Some(MenuSelection {
            row,
            button: Some(button),
        })
    }

    #[test]
    fn selections_follow_their_command_when_rows_move() {
        let model = model();
        // Rows 5 and 6 hold the Edit buttons and Toggle Fullscreen.
        assert_eq!(model.selection_of("fullscreen"), at(6));
        assert_eq!(model.selection_of("paste"), button(5, 1));
        assert_eq!(model.selection_of("copy"), None, "disabled button");
        assert_eq!(model.selection_of("copy_dir"), None, "disabled item");
        assert_eq!(model.selection_of("missing"), None);
        // Removing a row above moves the command, not the selection's row.
        let mut rows = model.rows.clone();
        rows.remove(0);
        let shifted = MenuModel::new(rows);
        assert_eq!(remap_selection(&model, at(6), &shifted), at(5));
        assert_eq!(
            remap_selection(&model, button(5, 1), &shifted),
            button(4, 1),
            "a selected button follows its row"
        );
        // A selection whose command disappears or becomes disabled clears.
        let mut rows = model.rows.clone();
        rows.retain(|row| {
            !matches!(row, MenuRow::Item(item) if item.id == "fullscreen")
        });
        assert_eq!(remap_selection(&model, at(6), &MenuModel::new(rows)), None);
        assert_eq!(remap_selection(&model, None, &shifted), None);
    }

    #[test]
    fn keyboard_open_selects_the_first_enabled_item() {
        assert_eq!(model().first(), at(0));
        let disabled_first = MenuModel::new(vec![
            MenuRow::Separator,
            MenuRow::Item(MenuItem::new("a", "A").disabled(None)),
            item("b", "B"),
        ]);
        assert_eq!(disabled_first.first(), at(2));
        assert_eq!(MenuModel::default().first(), None);
    }

    #[test]
    fn next_and_previous_skip_separators_and_disabled_rows_and_wrap() {
        let model = model();
        assert_eq!(model.next(None), at(0));
        assert_eq!(model.next(at(0)), at(2), "skips the separator");
        assert_eq!(model.next(at(2)), button(5, 1), "skips the disabled item");
        assert_eq!(model.next(at(9)), at(0), "wraps from the last row");
        assert_eq!(model.previous(None), at(9));
        assert_eq!(model.previous(at(0)), at(9), "wraps from the first row");
        assert_eq!(model.previous(at(6)), button(5, 1));
        assert_eq!(model.previous(button(5, 2)), at(2));
    }

    #[test]
    fn a_button_row_is_one_row_that_enters_on_its_first_enabled_button() {
        let model = model();
        assert_eq!(model.next(at(2)), button(5, 1));
        assert_eq!(model.next(button(5, 2)), at(6), "leaves as one row");
        assert_eq!(model.id_at(button(5, 1).unwrap()), Some("paste"));
        assert_eq!(model.id_at(button(5, 0).unwrap()), None);
    }

    #[test]
    fn left_and_right_move_within_a_button_row_and_wrap() {
        let model = model();
        assert_eq!(model.right(button(5, 1)), button(5, 2));
        assert_eq!(model.right(button(5, 2)), button(5, 1), "wraps");
        assert_eq!(model.left(button(5, 1)), button(5, 2), "skips disabled");
        assert_eq!(model.right(at(2)), at(2), "items stay put");
        assert_eq!(model.right(None), None);
    }

    #[test]
    fn first_and_last_jump_to_the_ends() {
        let model = model();
        assert_eq!(model.first(), at(0));
        assert_eq!(model.last(), at(9));
    }

    #[test]
    fn hover_selects_only_enabled_targets() {
        let model = model();
        assert_eq!(model.hover(2, None), at(2));
        assert_eq!(model.hover(3, None), None, "disabled item");
        assert_eq!(model.hover(1, None), None, "separator");
        assert_eq!(model.hover(5, Some(0)), None, "disabled button");
        assert_eq!(model.hover(5, Some(1)), button(5, 1));
        assert_eq!(model.hover(5, None), None, "the row itself is no target");
    }

    #[test]
    fn type_ahead_jumps_to_the_next_matching_item_and_wraps() {
        let model = model();
        assert_eq!(model.type_ahead(None, "n"), at(2));
        assert_eq!(model.type_ahead(at(2), "c"), at(0), "wraps past disabled");
        assert_eq!(model.type_ahead(at(0), "C"), at(0), "current row last");
        assert_eq!(model.type_ahead(at(6), "q"), at(9));
        assert_eq!(model.type_ahead(button(5, 1), "to"), at(6));
        assert_eq!(model.type_ahead(at(0), "zzz"), None);
        assert_eq!(model.type_ahead(at(0), " "), None);
    }

    fn window() -> Size<Pixels> {
        size(px(1000.0), px(600.0))
    }

    #[test]
    fn the_model_height_sums_fixed_row_heights_and_panel_chrome() {
        // Six items, three separators, one button row, plus 5pt padding and
        // a 1pt border on both sides.
        assert_eq!(model().height(), px(6.0 * 28.0 + 3.0 * 9.0 + 30.0 + 12.0));
        assert_eq!(MenuModel::default().height(), px(12.0));
    }

    #[test]
    fn a_button_in_the_upper_left_opens_down_and_left_aligned() {
        let anchor =
            Bounds::new(point(px(20.0), px(4.0)), size(px(26.0), px(26.0)));
        let placement = place_menu(
            MenuAnchor::Button(anchor),
            size(px(264.0), px(300.0)),
            window(),
        );
        assert!(!placement.opens_up);
        assert!(!placement.align_right);
        assert_eq!(placement.origin, point(px(20.0), px(34.0)));
        assert_eq!(placement.max_height, px(600.0 - 34.0 - 6.0));
    }

    #[test]
    fn a_button_in_the_lower_right_opens_up_and_right_aligned() {
        let anchor =
            Bounds::new(point(px(960.0), px(570.0)), size(px(26.0), px(26.0)));
        let placement = place_menu(
            MenuAnchor::Button(anchor),
            size(px(264.0), px(300.0)),
            window(),
        );
        assert!(placement.opens_up);
        assert!(placement.align_right);
        assert_eq!(
            placement.origin,
            point(px(986.0 - 264.0), px(566.0 - 300.0))
        );
        assert_eq!(placement.max_height, px(560.0));
    }

    #[test]
    fn a_tall_menu_scrolls_within_the_available_space() {
        let anchor =
            Bounds::new(point(px(20.0), px(4.0)), size(px(26.0), px(26.0)));
        let placement = place_menu(
            MenuAnchor::Button(anchor),
            size(px(264.0), px(900.0)),
            window(),
        );
        assert_eq!(placement.max_height, px(560.0));
        assert_eq!(placement.origin.y, px(34.0));
        let below = place_menu(
            MenuAnchor::Button(Bounds::new(
                point(px(20.0), px(570.0)),
                size(px(26.0), px(26.0)),
            )),
            size(px(264.0), px(900.0)),
            window(),
        );
        assert!(below.opens_up);
        assert_eq!(below.max_height, px(560.0));
        assert_eq!(below.origin.y, px(6.0));
    }

    #[test]
    fn a_pointer_menu_clamps_to_the_window_margin() {
        let placement = place_menu(
            MenuAnchor::Pointer(point(px(990.0), px(590.0))),
            size(px(264.0), px(200.0)),
            window(),
        );
        assert!(placement.opens_up);
        assert!(!placement.align_right);
        assert_eq!(
            placement.origin,
            point(px(1000.0 - 264.0 - 6.0), px(390.0))
        );
        let top_left = place_menu(
            MenuAnchor::Pointer(point(px(2.0), px(100.0))),
            size(px(264.0), px(200.0)),
            window(),
        );
        assert!(!top_left.opens_up);
        assert_eq!(top_left.origin, point(px(6.0), px(100.0)));
        assert_eq!(top_left.max_height, px(588.0));
    }

    #[test]
    fn a_pointer_menu_slides_to_fit_rather_than_scroll() {
        // From the upper half it would pass the bottom margin, so it rises
        // above the pointer just far enough to show every row.
        let fitted = place_menu(
            MenuAnchor::Pointer(point(px(300.0), px(250.0))),
            size(px(264.0), px(400.0)),
            window(),
        );
        assert!(!fitted.opens_up);
        assert_eq!(fitted.origin.y, px(600.0 - 6.0 - 400.0));
        assert!(fitted.max_height >= px(400.0), "{fitted:?}");
        // From the lower half it would pass the top margin, so it drops.
        let dropped = place_menu(
            MenuAnchor::Pointer(point(px(300.0), px(350.0))),
            size(px(264.0), px(400.0)),
            window(),
        );
        assert!(dropped.opens_up);
        assert_eq!(dropped.origin.y, px(6.0));
        assert!(dropped.max_height >= px(400.0), "{dropped:?}");
        // Only a menu taller than the window scrolls, filling it.
        let tall = place_menu(
            MenuAnchor::Pointer(point(px(300.0), px(250.0))),
            size(px(264.0), px(900.0)),
            window(),
        );
        assert_eq!(tall.origin.y, px(6.0));
        assert_eq!(tall.max_height, px(588.0));
    }
}
