//! The tab context menu: the rows a right-clicked tab offers, with their
//! enablement decided from the tab's position, directory, and panes.
//! Everything here is pure so it can be tested without a window.

use super::menu_spec::{Item as MenuItem, MenuSpec, Row as MenuRow};
use super::pane_menu::input_sync_item;
use super::window_menu::{label, shortcut_key};
use crate::keymap::{InstalledKeymap, Platform};
use gpui::KeyContext;
use huterm_protocol::{CommandId, ids};

/// The state the tab menu's rows depend on.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent fact about the tab"
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TabMenuInput {
    pub(super) platform: Platform,
    /// The targeted tab's position in the strip.
    pub(super) index: usize,
    /// How many tabs the window has.
    pub(super) count: usize,
    /// The targeted tab is the active one, which shows the shortcuts of
    /// Close Tab and the sync row, since those act on the active tab.
    pub(super) active: bool,
    /// Tabs run down a column, which words the last item "Close Tabs Below".
    pub(super) vertical: bool,
    /// The tab has reported a working directory Copy Directory Path can copy.
    pub(super) directory_known: bool,
    /// The panes the tab holds. With more than one, it offers to sync
    /// their input.
    pub(super) panes: usize,
    /// The tab syncs input, so its sync row shows, worded to stop.
    pub(super) input_sync: bool,
}

/// The rows for `input`, with the active tab's shortcuts from `keymap` as
/// seen from `contexts` (the stack captured when the menu opened),
/// preferring a user binding as the window menu does.
pub(super) fn tab_menu(
    input: &TabMenuInput,
    keymap: &InstalledKeymap,
    contexts: &[KeyContext],
) -> MenuSpec {
    // A shortcut acts on the active tab, so only its menu shows them.
    let shortcut = |id: CommandId| {
        if input.active {
            shortcut_key(keymap, contexts, id)
        } else {
            None
        }
    };
    let item = |id: CommandId| MenuItem::new(id.as_str(), label(id));
    let last = input.count == 0 || input.index + 1 >= input.count;
    let copy_directory = item(ids::COPY_TAB_DIRECTORY);
    let copy_directory = if input.directory_known {
        copy_directory
    } else {
        copy_directory.disabled(Some("Directory unknown".to_owned()))
    };
    let close_tab = item(ids::CLOSE_TAB).shortcut(shortcut(ids::CLOSE_TAB));
    let close_others = item(ids::CLOSE_OTHER_TABS);
    let close_others = if input.count > 1 {
        close_others
    } else {
        close_others.disabled(None)
    };
    let close_after = MenuItem::new(
        ids::CLOSE_TABS_AFTER.as_str(),
        if input.vertical {
            "Close Tabs Below"
        } else {
            "Close Tabs to the Right"
        },
    );
    let close_after = if last {
        close_after.disabled(None)
    } else {
        close_after
    };
    let mut rows = vec![
        MenuRow::Item(item(ids::RENAME_TAB)),
        MenuRow::Item(copy_directory),
        MenuRow::Separator,
        MenuRow::Item(close_tab),
        MenuRow::Item(close_others),
        MenuRow::Item(close_after),
        MenuRow::Separator,
    ];
    // As in the terminal menu: a split tab, or one that syncs, so a tab
    // that lost its other panes can still stop.
    if input.panes > 1 || input.input_sync {
        rows.push(MenuRow::Item(input_sync_item(
            input.input_sync,
            |id, label| {
                MenuItem::new(id.as_str(), label).shortcut(shortcut(id))
            },
        )));
    }
    rows.push(MenuRow::Item(MenuItem::new(
        ids::MOVE_TAB_TO_PANE.as_str(),
        "Move Tab into Pane…",
    )));
    MenuSpec::new(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KeybindingEntry;
    use crate::keymap::compile;
    use huterm_ui::components::kbd;
    use huterm_ui::listbox::{
        Item as MenuItem, Model as MenuModel, Row as MenuRow,
    };

    /// The rows Huterm's own menu draws for `input`.
    fn tab_menu_model(
        input: &TabMenuInput,
        keymap: &InstalledKeymap,
        contexts: &[KeyContext],
    ) -> MenuModel {
        tab_menu(input, keymap, contexts).to_listbox(input.platform)
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

    fn input(index: usize, count: usize) -> TabMenuInput {
        TabMenuInput {
            platform: Platform::MacOs,
            index,
            count,
            active: false,
            vertical: false,
            directory_known: true,
            panes: 1,
            input_sync: false,
        }
    }

    fn items(model: &MenuModel) -> Vec<&MenuItem> {
        model
            .rows
            .iter()
            .filter_map(|row| match row {
                MenuRow::Item(item) => Some(item),
                _ => None,
            })
            .collect()
    }

    fn model(input: &TabMenuInput) -> MenuModel {
        tab_menu_model(input, &keymap(input.platform, &[]), &contexts())
    }

    #[test]
    fn the_tab_menu_lists_the_plan_items_in_order() {
        let model = model(&input(1, 3));
        let labels: Vec<(&str, &str, bool)> = items(&model)
            .iter()
            .map(|item| (item.id.as_ref(), item.label.as_str(), item.enabled))
            .collect();
        assert_eq!(
            labels,
            vec![
                ("rename_tab", "Rename Tab…", true),
                ("copy_tab_directory", "Copy Directory Path", true),
                ("close_tab", "Close Tab", true),
                ("close_other_tabs", "Close Other Tabs", true),
                ("close_tabs_after", "Close Tabs to the Right", true),
                ("move_tab_to_pane", "Move Tab into Pane…", true),
            ]
        );
        assert!(
            matches!(model.rows[2], MenuRow::Separator),
            "one separator after the second item"
        );
        assert!(
            matches!(model.rows[6], MenuRow::Separator),
            "one separator before the move"
        );
        assert_eq!(model.rows.len(), 8);
    }

    #[test]
    fn enablement_follows_the_tab_position_and_count() {
        let enabled = |input: &TabMenuInput| -> Vec<bool> {
            items(&model(input))
                .iter()
                .map(|item| item.enabled)
                .collect()
        };
        assert_eq!(enabled(&input(0, 3)), [true, true, true, true, true, true]);
        assert_eq!(enabled(&input(1, 3)), [true, true, true, true, true, true]);
        assert_eq!(
            enabled(&input(2, 3)),
            [true, true, true, true, false, true],
            "the last tab has nothing to its right"
        );
        assert_eq!(
            enabled(&input(0, 1)),
            [true, true, true, false, false, true],
            "a single tab has no others and nothing after it, and can still \
             move into a pane of another workspace"
        );
        let single = model(&input(0, 1));
        let closers = items(&single);
        assert_eq!(closers[3].disabled_hint, None);
        assert_eq!(closers[4].disabled_hint, None);
    }

    #[test]
    fn vertical_strips_word_the_last_item_as_below() {
        let vertical = TabMenuInput {
            vertical: true,
            ..input(0, 2)
        };
        assert_eq!(items(&model(&vertical))[4].label, "Close Tabs Below");
        assert_eq!(
            items(&model(&input(0, 2)))[4].label,
            "Close Tabs to the Right"
        );
    }

    #[test]
    fn an_unknown_directory_disables_copy_with_a_hint() {
        let unknown = TabMenuInput {
            directory_known: false,
            ..input(0, 2)
        };
        let copy = items(&model(&unknown))[1].clone();
        assert!(!copy.enabled);
        assert_eq!(copy.disabled_hint.as_deref(), Some("Directory unknown"));
        assert!(items(&model(&input(0, 2)))[1].enabled);
    }

    /// The sync row's label, or `None` while the menu leaves it out.
    fn sync_row(input: &TabMenuInput) -> Option<String> {
        items(&model(input))
            .iter()
            .find(|item| item.id.as_ref() == "toggle_pane_input_sync")
            .map(|item| item.label.clone())
    }

    #[test]
    fn a_split_or_syncing_tab_offers_its_sync_toggle_before_the_move() {
        assert_eq!(
            sync_row(&input(0, 2)),
            None,
            "one pane has nothing to sync"
        );
        let split = TabMenuInput {
            panes: 2,
            ..input(0, 2)
        };
        assert_eq!(sync_row(&split).as_deref(), Some("Sync Pane Input"));
        let syncing = TabMenuInput {
            input_sync: true,
            ..input(0, 2)
        };
        assert_eq!(
            sync_row(&syncing).as_deref(),
            Some("Stop Syncing Pane Input"),
            "a syncing tab that lost its other panes can still stop"
        );
        let model = model(&split);
        assert!(matches!(model.rows[6], MenuRow::Separator));
        let last: Vec<&str> = items(&model)[5..]
            .iter()
            .map(|item| item.id.as_ref())
            .collect();
        assert_eq!(last, ["toggle_pane_input_sync", "move_tab_to_pane"]);
    }

    #[test]
    fn the_sync_row_shows_its_shortcut_only_on_the_active_tab() {
        let shortcut = |input: &TabMenuInput| {
            items(&model(input))
                .iter()
                .find(|item| item.id.as_ref() == "toggle_pane_input_sync")
                .map(|item| kbd::text(&item.shortcut))
        };
        let split = TabMenuInput {
            panes: 2,
            ..input(0, 2)
        };
        let active = TabMenuInput {
            active: true,
            ..split
        };
        assert_eq!(shortcut(&active).as_deref(), Some("⌥⌘I"));
        assert_eq!(shortcut(&split).as_deref(), Some(""));
    }

    #[test]
    fn close_tab_shows_its_shortcut_only_on_the_active_tab() {
        let active = TabMenuInput {
            active: true,
            ..input(0, 2)
        };
        // The default close shortcut closes the pane, so Close Tab shows
        // one only when the user binds it.
        assert_eq!(kbd::text(&items(&model(&active))[2].shortcut), "");
        let linux = TabMenuInput {
            platform: Platform::Linux,
            ..active
        };
        assert_eq!(kbd::text(&items(&model(&linux))[2].shortcut), "");
        let user = [KeybindingEntry {
            key: "ctrl-alt-w".to_owned(),
            command: "close_tab".to_owned(),
            args: None,
            when: None,
            description: None,
            disabled: None,
        }];
        let model = tab_menu_model(
            &active,
            &keymap(Platform::MacOs, &user),
            &contexts(),
        );
        assert_eq!(kbd::text(&items(&model)[2].shortcut), "⌃⌥W");
        let inactive = tab_menu_model(
            &input(0, 2),
            &keymap(Platform::MacOs, &user),
            &contexts(),
        );
        assert_eq!(kbd::text(&items(&inactive)[2].shortcut), "");
    }
}
