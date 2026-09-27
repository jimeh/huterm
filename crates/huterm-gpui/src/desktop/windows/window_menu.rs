//! The `⋯` window menu: where its button goes, how a vertical column's
//! new-tab row shares its width with it, the rows it lists, and the
//! platform shortcut text beside them. Everything here is pure so the
//! placement and model can be tested without a window.

use gpui::{KeyContext, Pixels, px};
use huterm_protocol::{CommandId, ids, lookup};

use super::super::key_hint::KeyHint;
use super::super::menu::{MenuButton, MenuItem, MenuModel, MenuRow};
use super::{CONTROL_SIZE, TabPosition};
use crate::keymap::{InstalledKeymap, Origin, Platform};

/// Where the `⋯` button is drawn.
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

/// Gap between the narrowed `+` target and the square `⋯` control.
const ROW_GAP: Pixels = px(4.0);

/// How a vertical column's new-tab row divides its width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct NewTabRow {
    /// Width of the `+` target from the row's left edge.
    pub(super) plus_width: Pixels,
    /// Left offset of the square `⋯` control within the row, when present.
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
    let shortcut = |id: CommandId| {
        keymap
            .shortcuts(id, contexts, None)
            .into_iter()
            .filter(|binding| binding.args.is_empty())
            .min_by_key(|binding| binding.origin != Origin::User)
            .map(|binding| KeyHint::parse(&binding.key, input.platform))
    };
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
mod tests {
    use super::*;
    use crate::config::KeybindingEntry;
    use crate::keymap::compile;
    use huterm_config::WindowConfig;

    const POSITIONS: [TabPosition; 4] = [
        TabPosition::Top,
        TabPosition::Bottom,
        TabPosition::Left,
        TabPosition::Right,
    ];

    #[test]
    fn placement_follows_platform_position_fullscreen_and_config() {
        use MenuButtonPlacement::{BarEnd, Hidden, SplitRow, TitleStrip};
        for position in POSITIONS {
            let horizontal = if position.vertical() {
                SplitRow
            } else {
                BarEnd
            };
            // macOS windowed keeps the title strip in every position.
            assert_eq!(
                menu_button_placement(true, true, position, true),
                TitleStrip
            );
            assert_eq!(
                menu_button_placement(true, true, position, false),
                TitleStrip
            );
            // Fullscreen on either platform, or Linux, joins the bar.
            assert_eq!(
                menu_button_placement(true, false, position, true),
                horizontal
            );
            // A hidden bar hides the button with it.
            assert_eq!(
                menu_button_placement(true, false, position, false),
                Hidden
            );
            // The config switch removes it everywhere.
            for title_strip in [true, false] {
                for bar_shown in [true, false] {
                    assert_eq!(
                        menu_button_placement(
                            false,
                            title_strip,
                            TabPosition::Titlebar,
                            bar_shown
                        ),
                        Hidden
                    );
                    assert_eq!(
                        menu_button_placement(
                            false,
                            title_strip,
                            position,
                            bar_shown
                        ),
                        Hidden
                    );
                }
            }
        }
        // The merged title-bar row holds the button at its bar end; the
        // strip takes it back while that bar is hidden.
        assert_eq!(
            menu_button_placement(true, true, TabPosition::Titlebar, true),
            BarEnd
        );
        assert_eq!(
            menu_button_placement(true, true, TabPosition::Titlebar, false),
            TitleStrip
        );
        assert!(WindowConfig::default().menu_button);
    }

    #[test]
    fn the_split_row_narrows_plus_and_keeps_the_square_control_at_the_end() {
        // Column widths less the 6pt margins on both sides.
        for (sidebar, row) in [(140.0, 128.0), (180.0, 168.0)] {
            let full = split_new_tab_row(px(row), false);
            assert_eq!(full.plus_width, px(row), "sidebar {sidebar}");
            assert_eq!(full.menu_x, None);
            let split = split_new_tab_row(px(row), true);
            assert_eq!(split.menu_x, Some(px(row - 26.0)), "sidebar {sidebar}");
            assert_eq!(split.plus_width, px(row - 26.0 - 4.0));
            assert_eq!(
                split.plus_width + ROW_GAP + CONTROL_SIZE,
                px(row),
                "the row keeps its width"
            );
        }
        let tiny = split_new_tab_row(px(20.0), true);
        assert_eq!(tiny.menu_x, Some(px(0.0)));
        assert_eq!(tiny.plus_width, px(0.0));
    }

    fn keymap(platform: Platform, user: &[KeybindingEntry]) -> InstalledKeymap {
        let (bindings, installed) =
            compile(platform, user).unwrap().install_parts();
        drop(bindings);
        installed
    }

    fn contexts() -> Vec<KeyContext> {
        vec![
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ]
    }

    fn items(model: &MenuModel) -> Vec<(&str, Option<String>)> {
        model
            .rows
            .iter()
            .filter_map(|row| match row {
                MenuRow::Item(item) => {
                    Some((item.id, item.shortcut.as_ref().map(KeyHint::text)))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_window_menu_lists_the_plan_items_with_platform_shortcuts() {
        let input = WindowMenuInput {
            platform: Platform::MacOs,
            updater: false,
            selection: false,
            notices: 0,
        };
        let model = window_menu_model(
            &input,
            &keymap(Platform::MacOs, &[]),
            &contexts(),
        );
        assert_eq!(
            items(&model),
            vec![
                ("open_command_palette", Some("⇧⌘P".to_owned())),
                ("new_tab", Some("⌘T".to_owned())),
                ("new_window", Some("⌘N".to_owned())),
                ("rename_tab", None),
                ("close_tab", Some("⌘W".to_owned())),
                ("close_window", Some("⇧⌘W".to_owned())),
                ("toggle_fullscreen", Some("⌘↩".to_owned())),
                ("open_settings", Some("⌘,".to_owned())),
                ("reload_config", Some("⌘<".to_owned())),
                ("about", None),
                ("quit", Some("⌘Q".to_owned())),
            ]
        );
        let labels: Vec<&str> = model
            .rows
            .iter()
            .filter_map(|row| match row {
                MenuRow::Item(item) => Some(item.label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(labels[0], "Command Palette…");
        assert_eq!(labels[3], "Rename Tab…");
        assert_eq!(labels[9], "About Huterm");
        assert_eq!(labels[10], "Quit Huterm");
        let separators = model
            .rows
            .iter()
            .filter(|row| matches!(row, MenuRow::Separator))
            .count();
        assert_eq!(separators, 6);
        let linux = WindowMenuInput {
            platform: Platform::Linux,
            ..input
        };
        let model = window_menu_model(
            &linux,
            &keymap(Platform::Linux, &[]),
            &contexts(),
        );
        let linux_items = items(&model);
        assert_eq!(
            linux_items[0],
            ("open_command_palette", Some("Ctrl+Shift+P".to_owned()))
        );
        assert_eq!(
            linux_items[6],
            ("toggle_fullscreen", Some("F11".to_owned()))
        );
        assert_eq!(
            linux_items[7],
            ("open_settings", Some("Ctrl+,".to_owned()))
        );
        assert_eq!(
            linux_items[10],
            ("quit", None),
            "Linux has no quit binding"
        );
    }

    #[test]
    fn updater_notices_and_selection_change_the_rows() {
        let base = WindowMenuInput {
            platform: Platform::MacOs,
            updater: true,
            selection: true,
            notices: 3,
        };
        let model = window_menu_model(
            &base,
            &keymap(Platform::MacOs, &[]),
            &contexts(),
        );
        let ids: Vec<&str> =
            items(&model).into_iter().map(|(id, _)| id).collect();
        assert!(ids.contains(&"check_for_updates"));
        let position =
            |id: &str| ids.iter().position(|item| *item == id).unwrap();
        assert_eq!(position("check_for_updates"), position("about") + 1);
        assert_eq!(position("focus_notices"), position("reload_config") + 1);
        let notices = model
            .rows
            .iter()
            .find_map(|row| match row {
                MenuRow::Item(item) if item.id == "focus_notices" => Some(item),
                _ => None,
            })
            .unwrap();
        assert_eq!(notices.badge, Some(3));
        assert_eq!(notices.label, "Show Notices");
        let edit = model
            .rows
            .iter()
            .find_map(|row| match row {
                MenuRow::Buttons { label, buttons } if label == "Edit" => {
                    Some(buttons)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(edit.len(), 2);
        assert!(edit[0].enabled, "Copy is enabled with a selection");
        assert_eq!(edit[0].id, "copy");
        assert!(edit[1].enabled);
        assert_eq!(edit[1].id, "paste");

        let quiet = WindowMenuInput {
            updater: false,
            selection: false,
            notices: 0,
            ..base
        };
        let model = window_menu_model(
            &quiet,
            &keymap(Platform::MacOs, &[]),
            &contexts(),
        );
        let ids: Vec<&str> =
            items(&model).into_iter().map(|(id, _)| id).collect();
        assert!(!ids.contains(&"check_for_updates"));
        assert!(!ids.contains(&"focus_notices"));
        let edit = model
            .rows
            .iter()
            .find_map(|row| match row {
                MenuRow::Buttons { buttons, .. } => Some(buttons),
                _ => None,
            })
            .unwrap();
        assert!(!edit[0].enabled);
        assert_eq!(edit[0].tooltip.as_deref(), Some("Nothing selected"));
    }

    #[test]
    fn user_bindings_replace_default_shortcut_text() {
        let user = [
            KeybindingEntry {
                key: "ctrl-alt-t".to_owned(),
                command: "new_tab".to_owned(),
                args: None,
                when: None,
                description: None,
            },
            KeybindingEntry {
                key: "cmd-shift-w".to_owned(),
                command: "unbind".to_owned(),
                args: None,
                when: None,
                description: None,
            },
        ];
        let input = WindowMenuInput {
            platform: Platform::MacOs,
            updater: false,
            selection: false,
            notices: 0,
        };
        let model = window_menu_model(
            &input,
            &keymap(Platform::MacOs, &user),
            &contexts(),
        );
        let shortcuts = items(&model);
        assert_eq!(shortcuts[1], ("new_tab", Some("⌃⌥T".to_owned())));
        assert_eq!(shortcuts[5], ("close_window", None), "unbound shows none");
    }
}
