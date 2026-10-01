//! The window menu: where its button goes, how a vertical column's
//! new-tab row shares its width with it, the rows it lists, and the
//! platform shortcut text beside them. Everything here is pure so the
//! placement and model can be tested without a window.

use gpui::{KeyContext, Pixels, px};
use huterm_protocol::{CommandId, ids, lookup};

use super::super::key_hint::KeyHint;
use super::super::menu::{MenuButton, MenuItem, MenuModel, MenuRow};
use super::{CONTROL_SIZE, TabPosition};
use crate::keymap::{InstalledKeymap, Origin, Platform};

/// Where the menu button is drawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MenuButtonPlacement {
    /// `window.menu_button = false`, or the tab bar that would hold it is
    /// hidden; `open_menu` still works from a fallback anchor.
    Hidden,
    /// The rightmost slot of the macOS title strip, in every tab position.
    TitleStrip,
    /// The far right end of a top or bottom tab bar.
    BarEnd,
    /// The right end of a vertical column's new-tab row, beside `+`.
    SplitRow,
}

/// The plan's placement rules. `title_strip` is a drawn title bar the
/// button can join (macOS windowed); `bar_shown` is whether the tab bar is
/// currently visible, so an auto-hidden bar hides the button with it.
/// `position` is the window's resolved position: `Titlebar` only arrives
/// with a title strip, whose row the shown tab bar then draws, so the
/// button moves to that bar's end and returns to the strip when the bar
/// hides.
pub(super) fn menu_button_placement(
    enabled: bool,
    title_strip: bool,
    position: TabPosition,
    bar_shown: bool,
) -> MenuButtonPlacement {
    if !enabled {
        MenuButtonPlacement::Hidden
    } else if position == TabPosition::Titlebar && bar_shown {
        MenuButtonPlacement::BarEnd
    } else if title_strip {
        MenuButtonPlacement::TitleStrip
    } else if !bar_shown {
        MenuButtonPlacement::Hidden
    } else if position.vertical() {
        MenuButtonPlacement::SplitRow
    } else {
        MenuButtonPlacement::BarEnd
    }
}

/// Gap between the narrowed `+` target and the square menu control.
const ROW_GAP: Pixels = px(4.0);

/// How a vertical column's new-tab row divides its width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct NewTabRow {
    /// Width of the `+` target from the row's left edge.
    pub(super) plus_width: Pixels,
    /// Left offset of the square menu control within the row, when present.
    pub(super) menu_x: Option<Pixels>,
}

/// Splits a row of `row_width` (the column width less its margins). Without
/// the button, `+` keeps the whole row; with it, `+` gives up the control's
/// width and a small gap at its right end.
pub(super) fn split_new_tab_row(
    row_width: Pixels,
    with_menu: bool,
) -> NewTabRow {
    let row_width = row_width.max(px(0.0));
    if !with_menu {
        return NewTabRow {
            plus_width: row_width,
            menu_x: None,
        };
    }
    let menu_x = (row_width - CONTROL_SIZE).max(px(0.0));
    NewTabRow {
        plus_width: (menu_x - ROW_GAP).max(px(0.0)),
        menu_x: Some(menu_x),
    }
}

/// The state the window menu's rows depend on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct WindowMenuInput {
    pub(super) platform: Platform,
    /// Built with the macOS updater, which adds Check for Updates.
    pub(super) updater: bool,
    /// The active terminal has selected text, which enables Copy.
    pub(super) selection: bool,
    /// Waiting notices; Show Notices appears with this badge while non-zero.
    pub(super) notices: usize,
}

impl WindowMenuInput {
    /// The build's platform and updater facts.
    pub(super) fn for_build(selection: bool, notices: usize) -> Self {
        Self {
            platform: Platform::current(),
            updater: cfg!(all(target_os = "macos", feature = "macos-updater")),
            selection,
            notices,
        }
    }
}

/// `id`'s shortcut as seen from `contexts`, the key-context stack captured
/// when a menu opened. A user binding is shown ahead of a default for the
/// same command, as the macOS menu bar shows it; bindings that carry
/// arguments run something else and are skipped.
pub(super) fn shortcut_hint(
    keymap: &InstalledKeymap,
    contexts: &[KeyContext],
    platform: Platform,
    id: CommandId,
) -> Option<KeyHint> {
    keymap
        .shortcuts(id, contexts, None)
        .into_iter()
        .filter(|binding| binding.args.is_empty())
        .min_by_key(|binding| binding.origin != Origin::User)
        .map(|binding| KeyHint::parse(&binding.key, platform))
}

/// Labels the plan words differently from the catalog: prompting items end
/// in an ellipsis, the application items name Huterm, and the tab menu's
/// copy item names the path.
pub(super) fn label(id: CommandId) -> String {
    match id {
        ids::OPEN_COMMAND_PALETTE => "Command Palette…".to_owned(),
        ids::RENAME_TAB => "Rename Tab…".to_owned(),
        ids::COPY_TAB_DIRECTORY => "Copy Directory Path".to_owned(),
        ids::CHECK_FOR_UPDATES => "Check for Updates…".to_owned(),
        ids::QUIT => "Quit Huterm".to_owned(),
        ids::FOCUS_NOTICES => "Show Notices".to_owned(),
        other => lookup(other.as_str()).map_or_else(
            || other.as_str().to_owned(),
            |spec| spec.title.to_owned(),
        ),
    }
}

/// The window menu's rows for `input`, with shortcut text from `keymap` as
/// seen from `contexts` (the stack captured when the menu opened). A user
/// binding is shown ahead of a default for the same command, as the macOS
/// menu bar shows it; items with no binding show none.
pub(super) fn window_menu_model(
    input: &WindowMenuInput,
    keymap: &InstalledKeymap,
    contexts: &[KeyContext],
) -> MenuModel {
    let shortcut =
        |id: CommandId| shortcut_hint(keymap, contexts, input.platform, id);
    let item = |id: CommandId| {
        MenuRow::Item(
            MenuItem::new(id.as_str(), label(id)).shortcut(shortcut(id)),
        )
    };
    let mut rows = vec![
        item(ids::OPEN_COMMAND_PALETTE),
        MenuRow::Separator,
        item(ids::NEW_TAB),
        item(ids::NEW_WINDOW),
        MenuRow::Separator,
        item(ids::RENAME_TAB),
        item(ids::CLOSE_TAB),
        item(ids::CLOSE_WINDOW),
        MenuRow::Separator,
        MenuRow::Buttons {
            label: "Edit".to_owned(),
            buttons: vec![
                {
                    let copy =
                        MenuButton::new(ids::COPY.as_str(), label(ids::COPY));
                    if input.selection {
                        copy
                    } else {
                        copy.disabled(Some("Nothing selected".to_owned()))
                    }
                },
                MenuButton::new(ids::PASTE.as_str(), label(ids::PASTE)),
            ],
        },
        item(ids::TOGGLE_FULLSCREEN),
        MenuRow::Separator,
        item(ids::OPEN_SETTINGS),
        item(ids::RELOAD_CONFIG),
    ];
    if input.notices > 0 {
        rows.push(MenuRow::Item(
            MenuItem::new(
                ids::FOCUS_NOTICES.as_str(),
                label(ids::FOCUS_NOTICES),
            )
            .badge(input.notices),
        ));
    }
    rows.push(MenuRow::Separator);
    rows.push(item(ids::ABOUT));
    if input.updater {
        rows.push(item(ids::CHECK_FOR_UPDATES));
    }
    rows.push(MenuRow::Separator);
    rows.push(item(ids::QUIT));
    MenuModel::new(rows)
}

#[cfg(test)]
mod tests;
