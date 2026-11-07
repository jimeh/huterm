use super::*;
use crate::config::KeybindingEntry;
use crate::keymap::compile;
use huterm_config::WindowConfig;
use huterm_ui::components::kbd;
use huterm_ui::listbox::{Model as MenuModel, Row as MenuRow};

/// The rows Huterm's own menu draws for `input`.
fn window_menu_model(
    input: &WindowMenuInput,
    keymap: &InstalledKeymap,
    contexts: &[KeyContext],
) -> MenuModel {
    window_menu(input, keymap, contexts).to_listbox(input.platform)
}

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
        let full = split_new_tab_row(px(row), false, false);
        assert_eq!(full.plus_width, px(row), "sidebar {sidebar}");
        assert_eq!((full.menu_x, full.session_x), (None, None));
        let split = split_new_tab_row(px(row), true, false);
        assert_eq!(split.menu_x, Some(px(row - 26.0)), "sidebar {sidebar}");
        assert_eq!(split.session_x, None);
        assert_eq!(split.plus_width, px(row - 26.0 - 4.0));
        assert_eq!(
            split.plus_width + ROW_GAP + CONTROL_SIZE,
            px(row),
            "the row keeps its width"
        );
        // The session control sits between `+` and the menu control.
        let both = split_new_tab_row(px(row), true, true);
        assert_eq!(both.menu_x, Some(px(row - 26.0)));
        assert_eq!(both.session_x, Some(px(row - 26.0 - 4.0 - 26.0)));
        assert_eq!(both.plus_width, px(row - 2.0 * (26.0 + 4.0)));
        // Without the menu button the session control takes its square.
        let session = split_new_tab_row(px(row), false, true);
        assert_eq!(session.menu_x, None);
        assert_eq!(session.session_x, Some(px(row - 26.0)));
        assert_eq!(session.plus_width, split.plus_width);
    }
    let tiny = split_new_tab_row(px(20.0), true, false);
    assert_eq!(tiny.menu_x, Some(px(0.0)));
    assert_eq!(tiny.plus_width, px(0.0));
    let tiny = split_new_tab_row(px(20.0), true, true);
    assert_eq!(
        (tiny.session_x, tiny.menu_x),
        (Some(px(0.0)), Some(px(0.0)))
    );
    assert_eq!(tiny.plus_width, px(0.0));
}

#[test]
fn the_session_control_follows_the_menu_button_in_every_window_shape() {
    use SessionControlHost::{BarEnd, Hidden, SplitRow, TitleStrip};
    for position in POSITIONS {
        let bar = if position.vertical() {
            SplitRow
        } else {
            BarEnd
        };
        // macOS windowed: the title strip in every position, including
        // under a shown bar and beside a left or right column.
        assert_eq!(
            session_control_host(true, true, position, true),
            TitleStrip,
            "{position:?} windowed"
        );
        assert_eq!(
            session_control_host(true, true, position, false),
            TitleStrip,
            "{position:?} windowed, bar hidden"
        );
        // Fullscreen, Quake, and Linux server decorations have no title
        // row: the end of a top or bottom bar, or icon-only in a column's
        // new-tab row.
        assert_eq!(
            session_control_host(true, false, position, true),
            bar,
            "{position:?} without a title row"
        );
        // A hidden bar hides it with the bar, like the menu button.
        assert_eq!(
            session_control_host(true, false, position, false),
            Hidden,
            "{position:?} auto-hidden"
        );
        // `session_button = false` hides it everywhere.
        for title_strip in [false, true] {
            for shown in [false, true] {
                assert_eq!(
                    session_control_host(false, title_strip, position, shown),
                    Hidden,
                    "{position:?} disabled"
                );
            }
        }
        // It never depends on the menu button: with the button off the
        // control takes the button's slot in the same host.
        for title_strip in [false, true] {
            for shown in [false, true] {
                let menu =
                    menu_button_placement(true, title_strip, position, shown);
                let session =
                    session_control_host(true, title_strip, position, shown);
                assert_eq!(
                    format!("{menu:?}"),
                    format!("{session:?}"),
                    "{position:?}"
                );
                assert_eq!(
                    menu_button_placement(false, title_strip, position, shown),
                    MenuButtonPlacement::Hidden
                );
            }
        }
    }
    // Tabs in the title bar: the end of the shown bar, which is the strip.
    assert_eq!(
        session_control_host(true, true, TabPosition::Titlebar, true),
        BarEnd
    );
    assert_eq!(
        session_control_host(true, true, TabPosition::Titlebar, false),
        TitleStrip
    );
    assert!(WindowConfig::default().session_button);
}

#[test]
fn the_session_control_truncates_long_names_and_collapses_in_narrow_hosts() {
    use SessionControlHost::{BarEnd, Hidden, SplitRow, TitleStrip};
    let chrome = huterm_ui::components::menu_trigger::width_for_label(px(0.0));
    // A short name fits whole in a roomy bar or strip.
    for host in [TitleStrip, BarEnd] {
        let short =
            SessionControlPlacement::resolve(host, px(900.0), px(40.0), false);
        assert_eq!(short.label_width, px(40.0), "{host:?}");
        assert_eq!(short.width, chrome + px(40.0));
        assert!(!short.truncated && !short.collapsed);
        assert_eq!(short.slot(), short.width + px(6.0));
    }
    // A long name truncates at the widest look.
    let long =
        SessionControlPlacement::resolve(BarEnd, px(900.0), px(400.0), false);
    assert_eq!(long.width, SESSION_CONTROL_MAX);
    assert_eq!(long.label_width, SESSION_CONTROL_MAX - chrome);
    assert!(long.truncated && !long.collapsed);
    // A narrower host shrinks it further before tabs or the title crowd.
    let narrow =
        SessionControlPlacement::resolve(BarEnd, px(270.0), px(400.0), false);
    // 35% of 270 points: the 52-point chrome and a 42.5-point label.
    assert_eq!(narrow.width, px(94.5), "{narrow:?}");
    assert_eq!(narrow.label_width, px(42.5));
    assert!(narrow.truncated && !narrow.collapsed);
    // Too little room for a readable label collapses to the icon, which
    // still names the session in its tooltip.
    let icon = SessionControlPlacement::resolve(
        TitleStrip,
        px(190.0),
        px(40.0),
        false,
    );
    assert!(icon.collapsed && icon.truncated, "{icon:?}");
    assert_eq!(icon.width, CONTROL_SIZE);
    assert_eq!(icon.label_width, px(0.0));
    // A right column's top strip lends its whole width, so the name
    // truncates to the column rather than collapsing.
    let column = SessionControlPlacement::resolve(
        TitleStrip,
        px(148.0),
        px(400.0),
        true,
    );
    assert_eq!(column.width, px(148.0));
    assert!(column.truncated && !column.collapsed);
    // A column's new-tab row has room for the icon alone.
    let row =
        SessionControlPlacement::resolve(SplitRow, px(900.0), px(10.0), false);
    assert!(row.collapsed);
    assert_eq!(row.width, CONTROL_SIZE);
    // Hidden takes no space.
    let hidden =
        SessionControlPlacement::resolve(Hidden, px(900.0), px(10.0), false);
    assert_eq!(hidden, SessionControlPlacement::HIDDEN);
    assert_eq!(hidden.slot(), px(0.0));
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
            MenuRow::Item(item) => Some((
                item.id.as_ref(),
                (!item.shortcut.is_empty()).then(|| kbd::text(&item.shortcut)),
            )),
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
        restorable: Restorable::default(),
    };
    let model =
        window_menu_model(&input, &keymap(Platform::MacOs, &[]), &contexts());
    assert_eq!(
        items(&model),
        vec![
            ("open_command_palette", Some("⇧⌘P".to_owned())),
            ("new_tab", Some("⌘T".to_owned())),
            ("new_window", Some("⌘N".to_owned())),
            ("split_right", Some("⌘D".to_owned())),
            ("split_down", Some("⇧⌘D".to_owned())),
            ("move_pane_to_new_tab", None),
            ("rename_tab", None),
            ("close_pane", Some("⌘W".to_owned())),
            ("arrange_panes", None),
            ("balance_panes", Some("⌃⌘=".to_owned())),
            ("close_tab", None),
            ("close_window", Some("⇧⌘W".to_owned())),
            ("undo", Some("⌘Z".to_owned())),
            ("redo", Some("⇧⌘Z".to_owned())),
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
    assert_eq!(labels[5], "Move Pane to New Tab");
    assert_eq!(labels[6], "Rename Tab…");
    assert_eq!(labels[8], "Arrange Panes…");
    assert_eq!(labels[9], "Balance Panes");
    assert_eq!(labels[12], "Undo");
    assert_eq!(labels[17], "About Huterm");
    assert_eq!(labels[18], "Quit Huterm");
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
        linux_items[3],
        ("split_right", Some("Ctrl+Shift+O".to_owned()))
    );
    assert_eq!(
        linux_items[7],
        ("close_pane", Some("Ctrl+Shift+W".to_owned()))
    );
    assert_eq!(
        linux_items[9],
        ("balance_panes", Some("Ctrl+Super+=".to_owned()))
    );
    assert_eq!(linux_items[12], ("undo", Some("Ctrl+Shift+Z".to_owned())));
    assert_eq!(linux_items[13], ("redo", Some("Ctrl+Shift+Y".to_owned())));
    assert_eq!(
        linux_items[14],
        ("toggle_fullscreen", Some("F11".to_owned()))
    );
    assert_eq!(
        linux_items[15],
        ("open_settings", Some("Ctrl+,".to_owned()))
    );
    assert_eq!(linux_items[18], ("quit", None), "Linux has no quit binding");
}

#[test]
fn updater_notices_and_selection_change_the_rows() {
    let base = WindowMenuInput {
        platform: Platform::MacOs,
        updater: true,
        selection: true,
        notices: 3,
        restorable: Restorable::default(),
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
fn undo_and_redo_rows_are_enabled_only_with_an_entry() {
    let state = |undo: bool, redo: bool| {
        let input = WindowMenuInput {
            platform: Platform::Linux,
            updater: false,
            selection: false,
            notices: 0,
            restorable: Restorable { undo, redo },
        };
        let model = window_menu_model(
            &input,
            &keymap(Platform::Linux, &[]),
            &contexts(),
        );
        ["undo", "redo"].map(|id| {
            model
                .rows
                .iter()
                .find_map(|row| match row {
                    MenuRow::Item(item) if item.id == id => {
                        Some((item.enabled, item.disabled_hint.clone()))
                    }
                    _ => None,
                })
                .unwrap()
        })
    };
    assert_eq!(
        state(false, false),
        [
            (false, Some("Nothing to undo".to_owned())),
            (false, Some("Nothing to redo".to_owned())),
        ]
    );
    assert_eq!(
        state(true, false),
        [(true, None), (false, Some("Nothing to redo".to_owned()))]
    );
    assert_eq!(state(false, true)[1], (true, None));
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
            disabled: None,
        },
        KeybindingEntry {
            key: "cmd-shift-w".to_owned(),
            command: "unbind".to_owned(),
            args: None,
            when: None,
            description: None,
            disabled: None,
        },
    ];
    let input = WindowMenuInput {
        platform: Platform::MacOs,
        updater: false,
        selection: false,
        notices: 0,
        restorable: Restorable::default(),
    };
    let model =
        window_menu_model(&input, &keymap(Platform::MacOs, &user), &contexts());
    let shortcuts = items(&model);
    assert_eq!(shortcuts[1], ("new_tab", Some("⌃⌥T".to_owned())));
    assert_eq!(shortcuts[11], ("close_window", None), "unbound shows none");
}
