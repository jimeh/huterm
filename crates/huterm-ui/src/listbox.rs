//! The selection model behind menus, selects, and later suggestion lists:
//! rows of items, separators, button rows, and submenus, and a selection
//! that moves over the enabled targets.
//!
//! The model is pure, so its navigation is testable without a window.
//! Keyboard navigation wraps and skips separators and disabled targets;
//! type-ahead jumps to the next item whose label starts with the typed text;
//! a pointer selects only enabled targets. A selection names a row and, in a
//! button row, the button within it. When rows change, [`remap_selection`]
//! keeps the selection on the same item rather than the same position.

// User input picks positions here, so a stale index must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use gpui::SharedString;

use crate::components::kbd::Glyph;

/// Identifies an item or button to its owner.
pub type ItemId = SharedString;

/// One selectable line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// What the owner receives when the item is picked.
    pub id: ItemId,
    /// The visible text, also matched by type-ahead.
    pub label: String,
    /// The right-aligned shortcut, drawn as key-cap glyphs.
    pub shortcut: Vec<Glyph>,
    /// Whether the item can be selected and picked.
    pub enabled: bool,
    /// Shown in place of the shortcut while disabled.
    pub disabled_hint: Option<String>,
    /// A count badge in place of the shortcut.
    pub badge: Option<usize>,
    /// A checkable row, checked when true; None is a plain row.
    pub checked: Option<bool>,
}

impl Item {
    /// An enabled item `label` named `id`.
    #[must_use]
    pub fn new(id: impl Into<ItemId>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            shortcut: Vec::new(),
            enabled: true,
            disabled_hint: None,
            badge: None,
            checked: None,
        }
    }

    /// The item with `shortcut` drawn on its right.
    #[must_use]
    pub fn shortcut(mut self, shortcut: Vec<Glyph>) -> Self {
        self.shortcut = shortcut;
        self
    }

    /// The item disabled, with `hint` shown in place of its shortcut.
    #[must_use]
    pub fn disabled(mut self, hint: Option<String>) -> Self {
        self.enabled = false;
        self.disabled_hint = hint;
        self
    }

    /// The item with a count badge in place of its shortcut.
    #[must_use]
    pub fn badge(mut self, count: usize) -> Self {
        self.badge = Some(count);
        self
    }

    /// Makes this row checkable with the given state.
    #[must_use]
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }
}

/// One button in a [`Row::Buttons`] row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    /// What the owner receives when the button is picked.
    pub id: ItemId,
    /// The visible text.
    pub label: String,
    /// Whether the button can be selected and picked.
    pub enabled: bool,
    /// Explains a disabled button on hover.
    pub tooltip: Option<String>,
}

impl Button {
    /// An enabled button `label` named `id`.
    #[must_use]
    pub fn new(id: impl Into<ItemId>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            enabled: true,
            tooltip: None,
        }
    }

    /// The button disabled, explained on hover by `tooltip`.
    #[must_use]
    pub fn disabled(mut self, tooltip: Option<String>) -> Self {
        self.enabled = false;
        self.tooltip = tooltip;
        self
    }
}

/// One row of a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// A selectable item.
    Item(Item),
    /// A hairline between groups; never selected.
    Separator,
    /// A label followed by inline buttons, such as the Edit row.
    Buttons {
        /// The row's label, shown before its buttons.
        label: String,
        /// The buttons, selected one at a time.
        buttons: Vec<Button>,
    },
    /// An item that opens `rows` in its place rather than being picked.
    /// It is selected, matched, and enabled as an item is; its `rows` are
    /// one level deep.
    Submenu {
        /// The row itself; its shortcut is not drawn.
        item: Item,
        /// The rows it opens.
        rows: Vec<Row>,
    },
}

/// A list's rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Model {
    /// The rows, top to bottom.
    pub rows: Vec<Row>,
}

/// A selected row, plus the button within a button row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The selected row.
    pub row: usize,
    /// The selected button within a button row.
    pub button: Option<usize>,
}

impl Model {
    /// A model over `rows`.
    #[must_use]
    pub fn new(rows: Vec<Row>) -> Self {
        Self { rows }
    }

    /// The selection entering `row` from the keyboard: the item, or the
    /// first enabled button. `None` for separators and disabled rows.
    fn enter_row(&self, row: usize) -> Option<Selection> {
        match self.rows.get(row)? {
            Row::Item(item) | Row::Submenu { item, .. } if item.enabled => {
                Some(Selection { row, button: None })
            }
            Row::Buttons { buttons, .. } => buttons
                .iter()
                .position(|button| button.enabled)
                .map(|button| Selection {
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
    #[must_use]
    pub fn id_at(&self, selection: Selection) -> Option<ItemId> {
        match (self.rows.get(selection.row)?, selection.button) {
            (Row::Item(item) | Row::Submenu { item, .. }, None)
                if item.enabled =>
            {
                Some(item.id.clone())
            }
            (Row::Buttons { buttons, .. }, Some(index)) => buttons
                .get(index)
                .filter(|button| button.enabled)
                .map(|button| button.id.clone()),
            _ => None,
        }
    }

    /// Where the enabled item or button `id` sits, if it does.
    #[must_use]
    pub fn selection_of(&self, id: &str) -> Option<Selection> {
        self.rows.iter().enumerate().find_map(|(row, model_row)| {
            let selection = match model_row {
                Row::Item(item) | Row::Submenu { item, .. }
                    if item.id == id =>
                {
                    Selection { row, button: None }
                }
                Row::Buttons { buttons, .. } => Selection {
                    row,
                    button: Some(
                        buttons.iter().position(|button| button.id == id)?,
                    ),
                },
                Row::Item(_) | Row::Submenu { .. } | Row::Separator => {
                    return None;
                }
            };
            self.id_at(selection).map(|_| selection)
        })
    }

    /// The first enabled target.
    #[must_use]
    pub fn first(&self) -> Option<Selection> {
        self.selectable_rows()
            .first()
            .and_then(|row| self.enter_row(*row))
    }

    /// The last enabled target.
    #[must_use]
    pub fn last(&self) -> Option<Selection> {
        self.selectable_rows()
            .last()
            .and_then(|row| self.enter_row(*row))
    }

    /// The next enabled row below `current`, wrapping; the first row with no
    /// selection.
    #[must_use]
    pub fn next(&self, current: Option<Selection>) -> Option<Selection> {
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
    #[must_use]
    pub fn previous(&self, current: Option<Selection>) -> Option<Selection> {
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
        current: Option<Selection>,
        forward: bool,
    ) -> Option<Selection> {
        let current = current?;
        let (Row::Buttons { buttons, .. }, Some(index)) =
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
        Some(Selection {
            row: current.row,
            button: Some(*enabled.get(next)?),
        })
    }

    /// The next enabled button in the current button row, wrapping. Item
    /// rows and an empty selection are unchanged.
    #[must_use]
    pub fn right(&self, current: Option<Selection>) -> Option<Selection> {
        self.step_button(current, true)
    }

    /// The previous enabled button in the current button row, wrapping.
    #[must_use]
    pub fn left(&self, current: Option<Selection>) -> Option<Selection> {
        self.step_button(current, false)
    }

    /// The selection for a pointer over `row` (and `button` within a button
    /// row); `None` when that target is disabled.
    #[must_use]
    pub fn hover(
        &self,
        row: usize,
        button: Option<usize>,
    ) -> Option<Selection> {
        let selection = Selection { row, button };
        self.id_at(selection).map(|_| selection)
    }

    /// The next enabled item after `current` whose label starts with
    /// `text`, case-insensitively, wrapping round to the current row last.
    #[must_use]
    pub fn type_ahead(
        &self,
        current: Option<Selection>,
        text: &str,
    ) -> Option<Selection> {
        let text = text.trim().to_lowercase();
        if text.is_empty() {
            return None;
        }
        let matches = |row: usize| match self.rows.get(row) {
            Some(Row::Item(item) | Row::Submenu { item, .. })
                if item.enabled =>
            {
                item.label.to_lowercase().starts_with(&text)
            }
            _ => false,
        };
        let start = current.map_or(0, |current| current.row + 1);
        let order =
            (start..self.rows.len()).chain(0..start.min(self.rows.len()));
        order
            .filter(|row| matches(*row))
            .map(|row| Selection { row, button: None })
            .next()
    }

    /// The enabled submenu named `id` among these rows: its item and the
    /// rows it opens.
    #[must_use]
    pub fn submenu(&self, id: &str) -> Option<(&Item, &[Row])> {
        self.rows.iter().find_map(|row| match row {
            Row::Submenu { item, rows } if item.id == id && item.enabled => {
                Some((item, rows.as_slice()))
            }
            _ => None,
        })
    }

    /// Whether `selection` names a submenu row.
    #[must_use]
    pub fn is_submenu(&self, selection: Selection) -> bool {
        matches!(self.rows.get(selection.row), Some(Row::Submenu { .. }))
    }
}

/// Where `selection` in `old` lands in `new`: on the same command, not the
/// same row, since rows that appear or disappear above it would otherwise
/// move it onto a different command. `None` once that command is gone or
/// disabled.
#[must_use]
pub fn remap_selection(
    old: &Model,
    selection: Option<Selection>,
    new: &Model,
) -> Option<Selection> {
    let id = old.id_at(selection?)?;
    new.selection_of(&id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &'static str, label: &str) -> Row {
        Row::Item(Item::new(id, label))
    }

    fn model() -> Model {
        Model::new(vec![
            item("palette", "Command Palette…"),
            Row::Separator,
            item("new_tab", "New Tab"),
            Row::Item(
                Item::new("copy_dir", "Copy Directory Path")
                    .disabled(Some("Directory unknown".to_owned())),
            ),
            Row::Separator,
            Row::Buttons {
                label: "Edit".to_owned(),
                buttons: vec![
                    Button::new("copy", "Copy")
                        .disabled(Some("Nothing selected".to_owned())),
                    Button::new("paste", "Paste"),
                    Button::new("select_all", "Select All"),
                ],
            },
            item("fullscreen", "Toggle Fullscreen"),
            Row::Separator,
            item("notices", "Show Notices"),
            item("quit", "Quit Huterm"),
        ])
    }

    #[expect(clippy::unnecessary_wraps, reason = "matches the API's results")]
    fn at(row: usize) -> Option<Selection> {
        Some(Selection { row, button: None })
    }

    #[expect(clippy::unnecessary_wraps, reason = "matches the API's results")]
    fn button(row: usize, button: usize) -> Option<Selection> {
        Some(Selection {
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
        let shifted = Model::new(rows);
        assert_eq!(remap_selection(&model, at(6), &shifted), at(5));
        assert_eq!(
            remap_selection(&model, button(5, 1), &shifted),
            button(4, 1),
            "a selected button follows its row"
        );
        // A selection whose command disappears or becomes disabled clears.
        let mut rows = model.rows.clone();
        rows.retain(
            |row| !matches!(row, Row::Item(item) if item.id == "fullscreen"),
        );
        assert_eq!(remap_selection(&model, at(6), &Model::new(rows)), None);
        assert_eq!(remap_selection(&model, None, &shifted), None);
    }

    #[test]
    fn keyboard_open_selects_the_first_enabled_item() {
        assert_eq!(model().first(), at(0));
        let disabled_first = Model::new(vec![
            Row::Separator,
            Row::Item(Item::new("a", "A").disabled(None)),
            item("b", "B"),
        ]);
        assert_eq!(disabled_first.first(), at(2));
        assert_eq!(Model::default().first(), None);
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
        assert_eq!(model.id_at(button(5, 1).unwrap()), Some("paste".into()));
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
    fn a_submenu_row_selects_matches_and_resolves_as_an_item() {
        let model = Model::new(vec![
            item("balance", "Balance Panes"),
            Row::Submenu {
                item: Item::new("arrange", "Arrange Panes"),
                rows: vec![item("arrange:rows", "Equal Rows")],
            },
            Row::Submenu {
                item: Item::new("off", "Disabled").disabled(None),
                rows: vec![item("off:a", "A")],
            },
        ]);
        assert_eq!(model.next(at(0)), at(1));
        assert_eq!(model.next(at(1)), at(0), "skips the disabled submenu");
        assert_eq!(model.id_at(at(1).unwrap()), Some("arrange".into()));
        assert_eq!(model.selection_of("arrange"), at(1));
        assert_eq!(model.selection_of("arrange:rows"), None, "one level");
        assert_eq!(model.type_ahead(None, "ar"), at(1));
        assert_eq!(model.hover(1, None), at(1));
        assert!(model.is_submenu(at(1).unwrap()));
        assert!(!model.is_submenu(at(0).unwrap()));
        let (submenu, rows) = model.submenu("arrange").unwrap();
        assert_eq!(submenu.label, "Arrange Panes");
        assert_eq!(rows, [item("arrange:rows", "Equal Rows")]);
        assert!(model.submenu("off").is_none(), "disabled");
        assert!(model.submenu("balance").is_none(), "an item");
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
}
