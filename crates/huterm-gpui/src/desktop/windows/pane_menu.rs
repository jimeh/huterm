//! The pane title bar's context menu: the rows a right-clicked pane bar
//! offers. Opening it focuses the pane, so the focused-pane commands act on
//! it and their shortcuts apply. Everything here is pure so it can be tested
//! without a window.

use super::menu_spec::{Item as MenuItem, MenuSpec, Row as MenuRow};
use super::window_menu::{label, shortcut_key};
use crate::keymap::{InstalledKeymap, Platform};
use gpui::KeyContext;
use huterm_config::PaneTitleBarMode;
use huterm_protocol::{CommandId, ids};

/// The Auto-hide Title Bars row's id. It switches `[panes] title_bar`
/// between `visible` and `hover` in the config file; it is not a catalog
/// command.
pub(super) const AUTO_HIDE_TITLE_BARS: &str = "auto_hide_title_bars";

/// The state the pane menu's rows depend on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PaneMenuInput {
    pub(super) platform: Platform,
    /// How many panes the tab has; a lone pane cannot zoom or move to a
    /// new tab of its own.
    pub(super) panes: usize,
    /// The configured bar mode, which checks the auto-hide row.
    pub(super) title_bar: PaneTitleBarMode,
    /// The pane shows across its whole tab, so the zoom row unzooms.
    pub(super) zoomed: bool,
    /// The tab syncs input, so the sync row stops it.
    pub(super) input_sync: bool,
    /// The pane has reported a working directory Copy Directory Path can
    /// copy.
    pub(super) directory_known: bool,
}

/// The rows for `input`, with shortcuts from `keymap` as seen from
/// `contexts`, the stack captured when the menu opened.
pub(super) fn pane_menu(
    input: PaneMenuInput,
    keymap: &InstalledKeymap,
    contexts: &[KeyContext],
) -> MenuSpec {
    let item = |id: CommandId, label: String| {
        MenuItem::new(id.as_str(), label)
            .shortcut(shortcut_key(keymap, contexts, id))
    };
    let catalog = |id: CommandId| item(id, label(id));
    let copy_directory = catalog(ids::COPY_TAB_DIRECTORY);
    let lone = input.panes == 1;
    let available = |item: MenuItem| {
        if lone {
            item.disabled(Some("Tab has one pane".to_owned()))
        } else {
            item
        }
    };
    let mut rows = vec![
        MenuRow::Item(item(ids::RENAME_PANE, "Rename Pane…".to_owned())),
        MenuRow::Item(if input.directory_known {
            copy_directory
        } else {
            copy_directory.disabled(Some("Directory unknown".to_owned()))
        }),
        MenuRow::Separator,
        MenuRow::Item(catalog(ids::SPLIT_RIGHT)),
        MenuRow::Item(catalog(ids::SPLIT_DOWN)),
        MenuRow::Separator,
        MenuRow::Item(available(item(
            ids::TOGGLE_PANE_ZOOM,
            if input.zoomed {
                "Unzoom Pane"
            } else {
                "Zoom Pane"
            }
            .to_owned(),
        ))),
    ];
    // A lone pane offers sync only to stop it, as the tab menu does.
    if !lone || input.input_sync {
        rows.push(MenuRow::Item(input_sync_item(input.input_sync, item)));
    }
    rows.extend([
        MenuRow::Item(available(catalog(ids::MOVE_PANE_TO_NEW_TAB))),
        MenuRow::Item(item(
            ids::MOVE_PANE_TO_PANE,
            "Move Pane to Another Pane…".to_owned(),
        )),
        MenuRow::Separator,
    ]);
    // Hidden bars have no menu to show this in.
    if input.title_bar != PaneTitleBarMode::Hidden {
        rows.extend([
            MenuRow::Item(
                MenuItem::new(AUTO_HIDE_TITLE_BARS, "Auto-hide Title Bars")
                    .checked(input.title_bar == PaneTitleBarMode::Hover),
            ),
            MenuRow::Separator,
        ]);
    }
    rows.push(MenuRow::Item(catalog(ids::CLOSE_PANE)));
    MenuSpec::new(rows)
}

/// The row that toggles the tab's input sync, worded for what a pick
/// does, built by `item` from its command and label.
pub(super) fn input_sync_item(
    input_sync: bool,
    item: impl Fn(CommandId, String) -> MenuItem,
) -> MenuItem {
    item(
        ids::TOGGLE_PANE_INPUT_SYNC,
        input_sync_label(input_sync).to_owned(),
    )
}

/// What toggling a tab's input sync does from its current state, for menu
/// rows and the palette.
pub(super) fn input_sync_label(input_sync: bool) -> &'static str {
    if input_sync {
        "Stop Syncing Pane Input"
    } else {
        "Sync Pane Input"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KeybindingEntry;
    use crate::keymap::compile;
    use huterm_ui::components::kbd;
    use huterm_ui::listbox::{Model as MenuModel, Row as MenuRow};

    fn model(input: PaneMenuInput, user: &[KeybindingEntry]) -> MenuModel {
        let (bindings, keymap) =
            compile(input.platform, user).unwrap().install_parts();
        drop(bindings);
        let contexts = vec![
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        pane_menu(input, &keymap, &contexts).to_listbox(input.platform)
    }

    fn input() -> PaneMenuInput {
        PaneMenuInput {
            platform: Platform::MacOs,
            panes: 2,
            title_bar: PaneTitleBarMode::Visible,
            zoomed: false,
            input_sync: false,
            directory_known: true,
        }
    }

    /// Item ids and labels in order, `!` marking disabled ones, `-`
    /// separators.
    fn rows(input: PaneMenuInput) -> Vec<String> {
        model(input, &[])
            .rows
            .iter()
            .map(|row| match row {
                MenuRow::Item(item) if item.enabled => {
                    format!("{}={}", item.id, item.label)
                }
                MenuRow::Item(item) => format!("{}={}!", item.id, item.label),
                MenuRow::Separator => "-".to_owned(),
                MenuRow::Buttons { .. } => "buttons".to_owned(),
                MenuRow::Submenu { .. } => "submenu".to_owned(),
            })
            .collect()
    }

    #[test]
    fn lone_pane_menu_disables_noops_and_only_shows_active_sync() {
        let model = model(
            PaneMenuInput {
                panes: 1,
                ..input()
            },
            &[],
        );
        let items: Vec<_> = model
            .rows
            .iter()
            .filter_map(|row| {
                if let MenuRow::Item(item) = row {
                    Some(item)
                } else {
                    None
                }
            })
            .collect();
        for id in [ids::TOGGLE_PANE_ZOOM, ids::MOVE_PANE_TO_NEW_TAB] {
            let item =
                items.iter().find(|item| item.id == id.as_str()).unwrap();
            assert!(!item.enabled);
            assert_eq!(item.disabled_hint.as_deref(), Some("Tab has one pane"));
        }
        assert!(
            !items
                .iter()
                .any(|item| item.id == ids::TOGGLE_PANE_INPUT_SYNC.as_str())
        );
        assert!(
            rows(PaneMenuInput {
                panes: 1,
                input_sync: true,
                ..input()
            })
            .iter()
            .any(|row| row == "toggle_pane_input_sync=Stop Syncing Pane Input")
        );
    }

    #[test]
    fn auto_hide_row_is_checked_only_for_hover() {
        for mode in [
            PaneTitleBarMode::Visible,
            PaneTitleBarMode::Hover,
            PaneTitleBarMode::Hidden,
        ] {
            let model = model(
                PaneMenuInput {
                    title_bar: mode,
                    ..input()
                },
                &[],
            );
            let state = model.rows.iter().find_map(|row| match row {
                MenuRow::Item(item) if item.id == AUTO_HIDE_TITLE_BARS => {
                    item.checked
                }
                _ => None,
            });
            assert_eq!(
                state,
                match mode {
                    PaneTitleBarMode::Visible => Some(false),
                    PaneTitleBarMode::Hover => Some(true),
                    PaneTitleBarMode::Hidden => None,
                }
            );
        }
    }

    #[test]
    fn the_menu_lists_the_pane_actions_in_order() {
        assert_eq!(
            rows(input()),
            [
                "rename_pane=Rename Pane…",
                "copy_tab_directory=Copy Directory Path",
                "-",
                "split_right=Split Right",
                "split_down=Split Down",
                "-",
                "toggle_pane_zoom=Zoom Pane",
                "toggle_pane_input_sync=Sync Pane Input",
                "move_pane_to_new_tab=Move Pane to New Tab",
                "move_pane_to_pane=Move Pane to Another Pane…",
                "-",
                "auto_hide_title_bars=Auto-hide Title Bars",
                "-",
                "close_pane=Close Pane",
            ]
        );
    }

    #[test]
    fn a_zoomed_pane_offers_unzoom() {
        let rows = rows(PaneMenuInput {
            zoomed: true,
            ..input()
        });
        assert!(rows.iter().any(|row| row == "toggle_pane_zoom=Unzoom Pane"));
    }

    #[test]
    fn a_syncing_tab_offers_to_stop_syncing() {
        let rows = rows(PaneMenuInput {
            input_sync: true,
            ..input()
        });
        assert!(
            rows.iter()
                .any(|row| row
                    == "toggle_pane_input_sync=Stop Syncing Pane Input")
        );
    }

    #[test]
    fn an_unknown_directory_disables_copy_with_a_hint() {
        let model = model(
            PaneMenuInput {
                directory_known: false,
                ..input()
            },
            &[],
        );
        let copy = model
            .rows
            .iter()
            .find_map(|row| match row {
                MenuRow::Item(item)
                    if item.id == ids::COPY_TAB_DIRECTORY.as_str() =>
                {
                    Some(item)
                }
                _ => None,
            })
            .unwrap();
        assert!(!copy.enabled);
        assert_eq!(copy.disabled_hint.as_deref(), Some("Directory unknown"));
    }

    #[test]
    fn rows_show_the_shortcuts_that_run_them() {
        let user = [KeybindingEntry {
            key: "ctrl-alt-n".to_owned(),
            command: "rename_pane".to_owned(),
            args: None,
            when: None,
            description: None,
            disabled: None,
        }];
        let model = model(input(), &user);
        let shortcut = |id: CommandId| {
            model
                .rows
                .iter()
                .find_map(|row| match row {
                    MenuRow::Item(item) if item.id == id.as_str() => {
                        Some(kbd::text(&item.shortcut))
                    }
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(shortcut(ids::RENAME_PANE), "⌃⌥N");
        assert_eq!(shortcut(ids::TOGGLE_PANE_INPUT_SYNC), "⌥⌘I");
        assert_ne!(
            shortcut(ids::CLOSE_PANE),
            "",
            "Close Pane has a default shortcut"
        );
    }
}
