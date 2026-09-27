//! The tab context menu: the rows a right-clicked tab offers, with their
//! enablement decided from the tab's position and directory. Everything
//! here is pure so it can be tested without a window.

use gpui::KeyContext;
use huterm_protocol::{CommandId, ids};

use super::super::key_hint::KeyHint;
use super::super::menu::{MenuItem, MenuModel, MenuRow};
use super::window_menu::label;
use crate::keymap::{InstalledKeymap, Origin, Platform};

/// The state the tab menu's rows depend on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TabMenuInput {
    pub(super) platform: Platform,
    /// The targeted tab's position in the strip.
    pub(super) index: usize,
    /// How many tabs the window has.
    pub(super) count: usize,
    /// The targeted tab is the active one, which shows Close Tab's shortcut.
    pub(super) active: bool,
    /// Tabs run down a column, which words the last item "Close Tabs Below".
    pub(super) vertical: bool,
    /// The tab has reported a working directory Copy Directory Path can copy.
    pub(super) directory_known: bool,
}

/// The rows for `input`, with Close Tab's shortcut from `keymap` as seen from
/// `contexts` (the stack captured when the menu opened), preferring a user
/// binding as the window menu does.
pub(super) fn tab_menu_model(
    input: &TabMenuInput,
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
    let item = |id: CommandId| MenuItem::new(id.as_str(), label(id));
    let last = input.count == 0 || input.index + 1 >= input.count;
    let copy_directory = item(ids::COPY_TAB_DIRECTORY);
    let copy_directory = if input.directory_known {
        copy_directory
    } else {
        copy_directory.disabled(Some("Directory unknown".to_owned()))
    };
    let close_tab = item(ids::CLOSE_TAB)
        .shortcut(input.active.then(|| shortcut(ids::CLOSE_TAB)).flatten());
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
    MenuModel::new(vec![
        MenuRow::Item(item(ids::RENAME_TAB)),
        MenuRow::Item(copy_directory),
        MenuRow::Separator,
        MenuRow::Item(close_tab),
        MenuRow::Item(close_others),
        MenuRow::Item(close_after),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KeybindingEntry;
    use crate::keymap::compile;

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
            .map(|item| (item.id, item.label.as_str(), item.enabled))
            .collect();
        assert_eq!(
            labels,
            vec![
                ("rename_tab", "Rename Tab…", true),
                ("copy_tab_directory", "Copy Directory Path", true),
                ("close_tab", "Close Tab", true),
                ("close_other_tabs", "Close Other Tabs", true),
                ("close_tabs_after", "Close Tabs to the Right", true),
            ]
        );
        assert!(
            matches!(model.rows[2], MenuRow::Separator),
            "one separator after the second item"
        );
        assert_eq!(model.rows.len(), 6);
    }

    #[test]
    fn enablement_follows_the_tab_position_and_count() {
        let enabled = |input: &TabMenuInput| -> Vec<bool> {
            items(&model(input))
                .iter()
                .map(|item| item.enabled)
                .collect()
        };
        assert_eq!(enabled(&input(0, 3)), [true, true, true, true, true]);
        assert_eq!(enabled(&input(1, 3)), [true, true, true, true, true]);
        assert_eq!(
            enabled(&input(2, 3)),
            [true, true, true, true, false],
            "the last tab has nothing to its right"
        );
        assert_eq!(
            enabled(&input(0, 1)),
            [true, true, true, false, false],
            "a single tab has no others and nothing after it"
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

    #[test]
    fn close_tab_shows_its_shortcut_only_on_the_active_tab() {
        let active = TabMenuInput {
            active: true,
            ..input(0, 2)
        };
        assert_eq!(
            items(&model(&active))[2]
                .shortcut
                .as_ref()
                .map(KeyHint::text)
                .as_deref(),
            Some("⌘W")
        );
        assert_eq!(items(&model(&input(0, 2)))[2].shortcut, None);
        let linux = TabMenuInput {
            platform: Platform::Linux,
            ..active
        };
        assert_eq!(
            items(&model(&linux))[2]
                .shortcut
                .as_ref()
                .map(KeyHint::text)
                .as_deref(),
            Some("Ctrl+Shift+W")
        );
        let user = [KeybindingEntry {
            key: "ctrl-alt-w".to_owned(),
            command: "close_tab".to_owned(),
            args: None,
            when: None,
            description: None,
        }];
        let model = tab_menu_model(
            &active,
            &keymap(Platform::MacOs, &user),
            &contexts(),
        );
        assert_eq!(
            items(&model)[2]
                .shortcut
                .as_ref()
                .map(KeyHint::text)
                .as_deref(),
            Some("⌃⌥W")
        );
    }
}
