//! The pane title bar's context menu: the rows a right-clicked pane bar
//! offers. Opening it focuses the pane, so the focused-pane commands act on
//! it and their shortcuts apply. Everything here is pure so it can be tested
//! without a window.

use super::menu_spec::{Item as MenuItem, MenuSpec, Row as MenuRow};
use super::window_menu::{label, shortcut_key};
use crate::keymap::{InstalledKeymap, Platform};
use gpui::KeyContext;
use huterm_protocol::{CommandId, ids};

/// The state the pane menu's rows depend on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PaneMenuInput {
    pub(super) platform: Platform,
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
    MenuSpec::new(vec![
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
        MenuRow::Item(item(
            ids::TOGGLE_PANE_ZOOM,
            if input.zoomed {
                "Unzoom Pane"
            } else {
                "Zoom Pane"
            }
            .to_owned(),
        )),
        MenuRow::Item(input_sync_item(input.input_sync, item)),
        MenuRow::Item(catalog(ids::MOVE_PANE_TO_NEW_TAB)),
        MenuRow::Item(item(
            ids::MOVE_PANE_TO_PANE,
            "Move Pane to Another Pane…".to_owned(),
        )),
        MenuRow::Separator,
        MenuRow::Item(catalog(ids::CLOSE_PANE)),
    ])
}

/// The row that toggles the tab's input sync, worded for what a pick
/// does, built by `item` from its command and label.
pub(super) fn input_sync_item(
    input_sync: bool,
    item: impl Fn(CommandId, String) -> MenuItem,
) -> MenuItem {
    item(
        ids::TOGGLE_PANE_INPUT_SYNC,
        if input_sync {
            "Stop Syncing Pane Input"
        } else {
            "Sync Pane Input"
        }
        .to_owned(),
    )
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
        assert_eq!(rows[6], "toggle_pane_zoom=Unzoom Pane");
    }

    #[test]
    fn a_syncing_tab_offers_to_stop_syncing() {
        let rows = rows(PaneMenuInput {
            input_sync: true,
            ..input()
        });
        assert_eq!(rows[7], "toggle_pane_input_sync=Stop Syncing Pane Input");
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
        let MenuRow::Item(copy) = &model.rows[1] else {
            panic!("the second row is an item");
        };
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
        let shortcut = |index: usize| match &model.rows[index] {
            MenuRow::Item(item) => kbd::text(&item.shortcut),
            _ => panic!("row {index} is an item"),
        };
        assert_eq!(shortcut(0), "⌃⌥N");
        assert_eq!(shortcut(7), "⌥⌘I");
        assert_ne!(shortcut(11), "", "Close Pane has a default shortcut");
    }
}
