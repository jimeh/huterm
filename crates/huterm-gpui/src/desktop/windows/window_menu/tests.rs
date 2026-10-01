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
        assert_eq!(menu_button_placement(true, false, position, false), Hidden);
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
    let model =
        window_menu_model(&input, &keymap(Platform::MacOs, &[]), &contexts());
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
    let model =
        window_menu_model(&linux, &keymap(Platform::Linux, &[]), &contexts());
    let linux_items = items(&model);
    assert_eq!(
        linux_items[0],
        ("open_command_palette", Some("Ctrl+Shift+P".to_owned()))
    );
    assert_eq!(
        linux_items[6],
        ("toggle_fullscreen", Some("F11".to_owned()))
    );
    assert_eq!(linux_items[7], ("open_settings", Some("Ctrl+,".to_owned())));
    assert_eq!(linux_items[10], ("quit", None), "Linux has no quit binding");
}

#[test]
fn updater_notices_and_selection_change_the_rows() {
    let base = WindowMenuInput {
        platform: Platform::MacOs,
        updater: true,
        selection: true,
        notices: 3,
    };
    let model =
        window_menu_model(&base, &keymap(Platform::MacOs, &[]), &contexts());
    let ids: Vec<&str> = items(&model).into_iter().map(|(id, _)| id).collect();
    assert!(ids.contains(&"check_for_updates"));
    let position = |id: &str| ids.iter().position(|item| *item == id).unwrap();
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
    let model =
        window_menu_model(&quiet, &keymap(Platform::MacOs, &[]), &contexts());
    let ids: Vec<&str> = items(&model).into_iter().map(|(id, _)| id).collect();
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
    let model =
        window_menu_model(&input, &keymap(Platform::MacOs, &user), &contexts());
    let shortcuts = items(&model);
    assert_eq!(shortcuts[1], ("new_tab", Some("⌃⌥T".to_owned())));
    assert_eq!(shortcuts[5], ("close_window", None), "unbound shows none");
}
