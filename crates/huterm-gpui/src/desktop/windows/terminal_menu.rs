//! The terminal context menu: the rows a right-click on the terminal
//! offers. Rows that only apply in the moment, such as the link under the
//! pointer and Scroll to Bottom, appear only then; the rest stay in place
//! and disable with a hint. Everything here is pure so it can be tested
//! without a window.

use gpui::KeyContext;
use huterm_protocol::{CommandId, ids};

use super::super::menu::{MenuItem, MenuItemId, MenuModel, MenuRow};
use super::window_menu::{label, shortcut_hint};
use crate::keymap::{InstalledKeymap, Platform};

/// Opens the link under the pointer. Link rows are not catalog commands:
/// they act on the destination captured when the menu opened.
pub(super) const OPEN_LINK: MenuItemId = "open_link";
/// Copies the link under the pointer.
pub(super) const COPY_LINK: MenuItemId = "copy_link";

/// What the tab's working directory allows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DirectoryState {
    Unknown,
    /// Reported by another host: its path can be copied but not opened.
    Remote,
    Local,
}

/// The state the terminal menu's rows depend on.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent fact about the terminal"
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TerminalMenuInput {
    pub(super) platform: Platform,
    /// A link lies under the pointer.
    pub(super) link: bool,
    /// Text is selected for Copy.
    pub(super) selection: bool,
    /// The root shell exited, so Paste has nowhere to go.
    pub(super) exited: bool,
    /// The view shows history rather than live output.
    pub(super) scrolled_back: bool,
    pub(super) directory: DirectoryState,
}

/// The rows for `input`, with shortcuts from `keymap` as seen from
/// `contexts`, the stack captured when the menu opened.
pub(super) fn terminal_menu_model(
    input: TerminalMenuInput,
    keymap: &InstalledKeymap,
    contexts: &[KeyContext],
) -> MenuModel {
    let item = |id: CommandId| {
        MenuItem::new(id.as_str(), label(id)).shortcut(shortcut_hint(
            keymap,
            contexts,
            input.platform,
            id,
        ))
    };
    let mut rows = Vec::new();
    if input.link {
        rows.extend([
            MenuRow::Item(MenuItem::new(OPEN_LINK, "Open Link")),
            MenuRow::Item(MenuItem::new(COPY_LINK, "Copy Link Address")),
            MenuRow::Separator,
        ]);
    }
    let copy = item(ids::COPY);
    let paste = item(ids::PASTE);
    rows.extend([
        MenuRow::Item(if input.selection {
            copy
        } else {
            copy.disabled(Some("Nothing selected".to_owned()))
        }),
        MenuRow::Item(if input.exited {
            paste.disabled(Some("Shell exited".to_owned()))
        } else {
            paste
        }),
        MenuRow::Item(item(ids::SELECT_ALL)),
        MenuRow::Separator,
        MenuRow::Item(item(ids::CLEAR_SCROLLBACK)),
        MenuRow::Item(item(ids::RESET_TERMINAL)),
    ]);
    if input.scrolled_back {
        rows.push(MenuRow::Item(item(ids::SCROLL_TO_BOTTOM)));
    }
    let copy_directory = item(ids::COPY_TAB_DIRECTORY);
    let open_directory = MenuItem::new(
        ids::OPEN_TAB_DIRECTORY.as_str(),
        match input.platform {
            Platform::MacOs => "Show in Finder",
            Platform::Linux => "Open in File Manager",
        },
    )
    .shortcut(shortcut_hint(
        keymap,
        contexts,
        input.platform,
        ids::OPEN_TAB_DIRECTORY,
    ));
    let unknown = || Some("Directory unknown".to_owned());
    rows.extend([
        MenuRow::Separator,
        MenuRow::Item(match input.directory {
            DirectoryState::Unknown => copy_directory.disabled(unknown()),
            DirectoryState::Remote | DirectoryState::Local => copy_directory,
        }),
        MenuRow::Item(match input.directory {
            DirectoryState::Unknown => open_directory.disabled(unknown()),
            DirectoryState::Remote => {
                open_directory.disabled(Some("Remote directory".to_owned()))
            }
            DirectoryState::Local => open_directory,
        }),
        MenuRow::Separator,
        MenuRow::Item(item(ids::NEW_TAB)),
        MenuRow::Item(item(ids::RENAME_TAB)),
        MenuRow::Item(item(ids::CLOSE_TAB)),
    ]);
    MenuModel::new(rows)
}

#[cfg(test)]
mod tests {
    use super::super::super::key_hint::KeyHint;
    use super::*;
    use crate::keymap::compile;

    fn model(input: TerminalMenuInput) -> MenuModel {
        let (bindings, keymap) =
            compile(input.platform, &[]).unwrap().install_parts();
        drop(bindings);
        let contexts = vec![
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        terminal_menu_model(input, &keymap, &contexts)
    }

    fn input() -> TerminalMenuInput {
        TerminalMenuInput {
            platform: Platform::MacOs,
            link: false,
            selection: true,
            exited: false,
            scrolled_back: false,
            directory: DirectoryState::Local,
        }
    }

    /// Item ids in order, `!` marking disabled ones, `-` separators.
    fn rows(input: TerminalMenuInput) -> Vec<String> {
        model(input)
            .rows
            .iter()
            .map(|row| match row {
                MenuRow::Item(item) if item.enabled => item.id.to_owned(),
                MenuRow::Item(item) => format!("{}!", item.id),
                MenuRow::Separator => "-".to_owned(),
                MenuRow::Buttons { .. } => "buttons".to_owned(),
            })
            .collect()
    }

    #[test]
    fn the_menu_lists_editing_terminal_directory_and_tab_items() {
        assert_eq!(
            rows(input()),
            [
                "copy",
                "paste",
                "select_all",
                "-",
                "clear_scrollback",
                "reset_terminal",
                "-",
                "copy_tab_directory",
                "open_tab_directory",
                "-",
                "new_tab",
                "rename_tab",
                "close_tab",
            ]
        );
    }

    #[test]
    fn momentary_rows_appear_only_when_they_apply() {
        let rows = rows(TerminalMenuInput {
            link: true,
            scrolled_back: true,
            ..input()
        });
        assert_eq!(rows[..3], ["open_link", "copy_link", "-"]);
        assert!(rows.contains(&"scroll_to_bottom".to_owned()));
    }

    #[test]
    fn copy_needs_a_selection_and_paste_a_live_shell() {
        let model = model(TerminalMenuInput {
            selection: false,
            exited: true,
            ..input()
        });
        let hints: Vec<_> = model
            .rows
            .iter()
            .filter_map(|row| match row {
                MenuRow::Item(item) if !item.enabled => {
                    Some((item.id, item.disabled_hint.as_deref()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            hints,
            [
                ("copy", Some("Nothing selected")),
                ("paste", Some("Shell exited")),
            ]
        );
    }

    #[test]
    fn directory_items_follow_where_the_directory_lives() {
        for (directory, copy, open) in [
            (
                DirectoryState::Local,
                "copy_tab_directory",
                "open_tab_directory",
            ),
            (
                DirectoryState::Remote,
                "copy_tab_directory",
                "open_tab_directory!",
            ),
            (
                DirectoryState::Unknown,
                "copy_tab_directory!",
                "open_tab_directory!",
            ),
        ] {
            let rows = rows(TerminalMenuInput {
                directory,
                ..input()
            });
            assert_eq!(rows[7..9], [copy, open], "{directory:?}");
        }
    }

    #[test]
    fn items_carry_platform_labels_and_shortcuts() {
        let item = |platform: Platform, id: &str| {
            model(TerminalMenuInput {
                platform,
                ..input()
            })
            .rows
            .into_iter()
            .find_map(|row| match row {
                MenuRow::Item(item) if item.id == id => Some(item),
                _ => None,
            })
            .unwrap()
        };
        let hint = |item: MenuItem| item.shortcut.as_ref().map(KeyHint::text);
        assert_eq!(
            item(Platform::MacOs, "open_tab_directory").label,
            "Show in Finder"
        );
        assert_eq!(
            item(Platform::Linux, "open_tab_directory").label,
            "Open in File Manager"
        );
        assert_eq!(hint(item(Platform::MacOs, "copy")).as_deref(), Some("⌘C"));
        assert_eq!(
            hint(item(Platform::Linux, "paste")).as_deref(),
            Some("Ctrl+Shift+V")
        );
        assert_eq!(
            hint(item(Platform::MacOs, "select_all")).as_deref(),
            Some("⌘A")
        );
        assert_eq!(item(Platform::MacOs, "rename_tab").label, "Rename Tab…");
    }
}
