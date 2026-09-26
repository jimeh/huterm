//! The `⋯` window menu: where its button goes, how a vertical column's
//! new-tab row shares its width with it, the rows it lists, and the
//! platform shortcut text beside them. Everything here is pure so the
//! placement and model can be tested without a window.

use gpui::{KeyContext, Keystroke, Pixels, px};
use huterm_protocol::{CommandId, ids, lookup};

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
pub(super) fn menu_button_placement(
    enabled: bool,
    title_strip: bool,
    position: TabPosition,
    bar_shown: bool,
) -> MenuButtonPlacement {
    if !enabled {
        MenuButtonPlacement::Hidden
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

/// A binding key in the platform's shortcut spelling: `⇧⌘P` on macOS,
/// `Ctrl+Shift+P` on Linux. Chord strokes are separated by spaces, and a
/// key that does not parse is shown as written.
pub(super) fn format_shortcut(key: &str, platform: Platform) -> String {
    key.split_whitespace()
        .map(|stroke| format_keystroke(stroke, platform))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_keystroke(source: &str, platform: Platform) -> String {
    let Ok(keystroke) = Keystroke::parse(source) else {
        return source.to_owned();
    };
    let modifiers = keystroke.modifiers;
    let key = key_name(&keystroke.key, platform);
    match platform {
        Platform::MacOs => {
            let mut text = String::new();
            if modifiers.function {
                text.push_str("fn");
            }
            if modifiers.control {
                text.push('⌃');
            }
            if modifiers.alt {
                text.push('⌥');
            }
            if modifiers.shift {
                text.push('⇧');
            }
            if modifiers.platform {
                text.push('⌘');
            }
            text.push_str(&key);
            text
        }
        Platform::Linux => {
            let mut parts = Vec::new();
            if modifiers.function {
                parts.push("Fn".to_owned());
            }
            if modifiers.control {
                parts.push("Ctrl".to_owned());
            }
            if modifiers.alt {
                parts.push("Alt".to_owned());
            }
            if modifiers.shift {
                parts.push("Shift".to_owned());
            }
            if modifiers.platform {
                parts.push("Super".to_owned());
            }
            parts.push(key);
            parts.join("+")
        }
    }
}

fn key_name(key: &str, platform: Platform) -> String {
    let mac = platform == Platform::MacOs;
    let named = match key {
        "enter" if mac => "↩",
        "enter" => "Enter",
        "escape" if mac => "esc",
        "escape" => "Esc",
        "tab" if mac => "⇥",
        "tab" => "Tab",
        "space" => "Space",
        "backspace" if mac => "⌫",
        "backspace" => "Backspace",
        "delete" if mac => "⌦",
        "delete" => "Delete",
        "up" if mac => "↑",
        "up" => "Up",
        "down" if mac => "↓",
        "down" => "Down",
        "left" if mac => "←",
        "left" => "Left",
        "right" if mac => "→",
        "right" => "Right",
        "pageup" if mac => "⇞",
        "pageup" => "PageUp",
        "pagedown" if mac => "⇟",
        "pagedown" => "PageDown",
        "home" => "Home",
        "end" => "End",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_owned();
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), None) => letter.to_uppercase().collect(),
        (Some('f'), Some(digit))
            if digit.is_ascii_digit()
                && key[1..].bytes().all(|b| b.is_ascii_digit()) =>
        {
            format!("F{}", &key[1..])
        }
        _ => key.to_owned(),
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
/// in an ellipsis and the application items name Huterm.
fn label(id: CommandId) -> String {
    match id {
        ids::OPEN_COMMAND_PALETTE => "Command Palette…".to_owned(),
        ids::RENAME_TAB => "Rename Tab…".to_owned(),
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
            .map(|binding| format_shortcut(&binding.key, input.platform))
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
                            position,
                            bar_shown
                        ),
                        Hidden
                    );
                }
            }
        }
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

    #[test]
    fn shortcuts_use_platform_symbols_and_names() {
        let mac = Platform::MacOs;
        let linux = Platform::Linux;
        assert_eq!(format_shortcut("cmd-shift-p", mac), "⇧⌘P");
        assert_eq!(format_shortcut("ctrl-shift-p", linux), "Ctrl+Shift+P");
        assert_eq!(format_shortcut("cmd-enter", mac), "⌘↩");
        assert_eq!(format_shortcut("f11", linux), "F11");
        assert_eq!(format_shortcut("shift-end", mac), "⇧End");
        assert_eq!(format_shortcut("shift-end", linux), "Shift+End");
        assert_eq!(format_shortcut("cmd-,", mac), "⌘,");
        assert_eq!(format_shortcut("ctrl-,", linux), "Ctrl+,");
        assert_eq!(format_shortcut("cmd-<", mac), "⌘<");
        assert_eq!(
            format_shortcut("ctrl-alt-delete", linux),
            "Ctrl+Alt+Delete"
        );
        assert_eq!(format_shortcut("alt-backspace", mac), "⌥⌫");
        assert_eq!(format_shortcut("ctrl-k ctrl-t", linux), "Ctrl+K Ctrl+T");
        assert_eq!(format_shortcut("escape", mac), "esc");
        assert_eq!(format_shortcut("escape", linux), "Esc");
        assert_eq!(format_shortcut("", linux), "");
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

    fn items(model: &MenuModel) -> Vec<(&str, Option<&str>)> {
        model
            .rows
            .iter()
            .filter_map(|row| match row {
                MenuRow::Item(item) => {
                    Some((item.id, item.shortcut.as_deref()))
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
                ("open_command_palette", Some("⇧⌘P")),
                ("new_tab", Some("⌘T")),
                ("new_window", Some("⌘N")),
                ("rename_tab", None),
                ("close_tab", Some("⌘W")),
                ("close_window", Some("⇧⌘W")),
                ("toggle_fullscreen", Some("⌘↩")),
                ("open_settings", Some("⌘,")),
                ("reload_config", Some("⌘<")),
                ("about", None),
                ("quit", Some("⌘Q")),
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
            ("open_command_palette", Some("Ctrl+Shift+P"))
        );
        assert_eq!(linux_items[6], ("toggle_fullscreen", Some("F11")));
        assert_eq!(linux_items[7], ("open_settings", Some("Ctrl+,")));
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
        assert_eq!(shortcuts[1], ("new_tab", Some("⌃⌥T")));
        assert_eq!(shortcuts[5], ("close_window", None), "unbound shows none");
    }
}
