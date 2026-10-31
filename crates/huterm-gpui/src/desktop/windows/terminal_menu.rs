//! The terminal context menu: the rows a right-click on the terminal
//! offers. Rows that only apply in the moment, such as the link under the
//! pointer and Scroll to Bottom, appear only then; the rest stay in place
//! and disable with a hint. Everything here is pure so it can be tested
//! without a window.

use super::menu_spec::{Item as MenuItem, MenuSpec, Row as MenuRow};
use super::pane_layouts::holds_at_most;
use super::pane_menu::input_sync_item;
use super::window_menu::{label, shortcut_key};
use crate::keymap::{InstalledKeymap, Platform};
use gpui::KeyContext;
use huterm_protocol::{CommandId, NamedLayout, ids};

/// Opens the link under the pointer. Link rows are not catalog commands:
/// they act on the destination captured when the menu opened.
pub(super) const OPEN_LINK: &str = "open_link";
/// Copies the link under the pointer.
pub(super) const COPY_LINK: &str = "copy_link";
/// The Arrange Panes submenu's row.
pub(super) const ARRANGE_PANES_MENU: &str = "arrange_panes_menu";
/// What starts an Arrange Panes item's id, before its layout's name.
pub(super) const ARRANGE_PANES_PREFIX: &str = "arrange_panes:";

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
    /// The panes the tab holds. With more than one, its panes can balance
    /// and arrange, and its pane can move to a tab of its own.
    pub(super) panes: usize,
    /// The tab syncs input. Its sync row shows while this holds or the tab
    /// is split.
    pub(super) input_sync: bool,
    pub(super) directory: DirectoryState,
}

/// The rows for `input`, with shortcuts from `keymap` as seen from
/// `contexts`, the stack captured when the menu opened. A split tab's
/// Arrange Panes submenu lists `layouts`, in order.
pub(super) fn terminal_menu(
    input: TerminalMenuInput,
    layouts: &[NamedLayout],
    keymap: &InstalledKeymap,
    contexts: &[KeyContext],
) -> MenuSpec {
    let item = |id: CommandId| {
        MenuItem::new(id.as_str(), label(id))
            .shortcut(shortcut_key(keymap, contexts, id))
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
    .shortcut(shortcut_key(keymap, contexts, ids::OPEN_TAB_DIRECTORY));
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
        MenuRow::Item(item(ids::SPLIT_RIGHT)),
        MenuRow::Item(item(ids::SPLIT_DOWN)),
        MenuRow::Item(item(ids::CLOSE_PANE)),
    ]);
    if input.panes > 1 {
        rows.extend([
            MenuRow::Item(item(ids::BALANCE_PANES)),
            arrange_panes(layouts, input.panes),
            MenuRow::Item(item(ids::MOVE_PANE_TO_NEW_TAB)),
        ]);
    }
    if input.panes > 1 || input.input_sync {
        rows.push(MenuRow::Item(input_sync_item(
            input.input_sync,
            |id, label| {
                MenuItem::new(id.as_str(), label)
                    .shortcut(shortcut_key(keymap, contexts, id))
            },
        )));
    }
    rows.extend([
        MenuRow::Separator,
        MenuRow::Item(item(ids::NEW_TAB)),
        MenuRow::Item(item(ids::RENAME_TAB)),
        MenuRow::Item(item(ids::CLOSE_TAB)),
    ]);
    MenuSpec::new(rows)
}

/// The Arrange Panes submenu: one item per layout, named
/// `arrange_panes:<name>` and labelled by its title. A layout that cannot
/// hold the tab's `panes` is disabled with its capacity.
fn arrange_panes(layouts: &[NamedLayout], panes: usize) -> MenuRow {
    let rows = layouts
        .iter()
        .map(|layout| {
            let item = MenuItem::new(
                format!("{ARRANGE_PANES_PREFIX}{}", layout.name),
                layout.title.clone(),
            );
            MenuRow::Item(match holds_at_most(layout, panes) {
                Some(most) => {
                    item.disabled(Some(format!("Holds at most {most} panes")))
                }
                None => item,
            })
        })
        .collect();
    MenuRow::Submenu {
        item: MenuItem::new(ARRANGE_PANES_MENU, "Arrange Panes"),
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::compile;
    use huterm_ui::components::kbd;
    use huterm_ui::listbox::{
        Item as MenuItem, Model as MenuModel, Row as MenuRow,
    };

    fn spec(input: TerminalMenuInput, layouts: &[NamedLayout]) -> MenuSpec {
        let (bindings, keymap) =
            compile(input.platform, &[]).unwrap().install_parts();
        drop(bindings);
        let contexts = vec![
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        terminal_menu(input, layouts, &keymap, &contexts)
    }

    fn model(input: TerminalMenuInput) -> MenuModel {
        let layouts = huterm_config::PanesConfig::default().layouts();
        spec(input, &layouts).to_listbox(input.platform)
    }

    fn input() -> TerminalMenuInput {
        TerminalMenuInput {
            platform: Platform::MacOs,
            link: false,
            selection: true,
            exited: false,
            scrolled_back: false,
            panes: 1,
            input_sync: false,
            directory: DirectoryState::Local,
        }
    }

    /// Item ids in order, `!` marking disabled ones, `-` separators.
    fn rows(input: TerminalMenuInput) -> Vec<String> {
        model(input)
            .rows
            .iter()
            .map(|row| match row {
                MenuRow::Item(item) if item.enabled => item.id.to_string(),
                MenuRow::Item(item) => format!("{}!", item.id),
                MenuRow::Separator => "-".to_owned(),
                MenuRow::Buttons { .. } => "buttons".to_owned(),
                MenuRow::Submenu { item, .. } => format!("{}>", item.id),
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
                "split_right",
                "split_down",
                "close_pane",
                "-",
                "new_tab",
                "rename_tab",
                "close_tab",
            ]
        );
    }

    #[test]
    fn a_split_tab_adds_balance_arrange_move_and_sync_after_close_pane() {
        let rows = rows(TerminalMenuInput {
            panes: 2,
            ..input()
        });
        let close = rows.iter().position(|row| row == "close_pane").unwrap();
        assert_eq!(
            rows[close..close + 6],
            [
                "close_pane",
                "balance_panes",
                "arrange_panes_menu>",
                "move_pane_to_new_tab",
                "toggle_pane_input_sync",
                "-"
            ]
        );
    }

    #[test]
    fn a_syncing_tab_of_one_pane_still_offers_to_stop_syncing() {
        let model = model(TerminalMenuInput {
            input_sync: true,
            ..input()
        });
        let sync = model
            .rows
            .iter()
            .find_map(|row| match row {
                MenuRow::Item(item) if item.id == "toggle_pane_input_sync" => {
                    Some(item)
                }
                _ => None,
            })
            .expect("a syncing tab shows its sync row");
        assert_eq!(sync.label, "Stop Syncing Pane Input");
        assert_eq!(kbd::text(&sync.shortcut), "⌥⌘I");
    }

    #[test]
    fn arrange_panes_lists_every_layout_and_disables_those_too_small() {
        let panes = huterm_config::PanesConfig {
            layouts: [
                ("pair".to_owned(), "columns(pane, pane)".to_owned()),
                ("dev".to_owned(), "columns(pane 55%, rows(*))".to_owned()),
            ]
            .into(),
            ..huterm_config::PanesConfig::default()
        };
        let layouts = panes.layouts();
        let arrange = |count: usize| {
            let input = TerminalMenuInput {
                panes: count,
                ..input()
            };
            spec(input, &layouts)
                .to_listbox(Platform::MacOs)
                .rows
                .into_iter()
                .find_map(|row| match row {
                    MenuRow::Submenu { item, rows } => Some((item, rows)),
                    _ => None,
                })
        };
        let (item, rows) = arrange(3).expect("a split tab arranges");
        assert_eq!(item.label, "Arrange Panes");
        // The built-ins in the default cycle's order, then user layouts by
        // name, as the palette lists them; `pair` holds two panes, not
        // three.
        let shown: Vec<_> = rows
            .iter()
            .map(|row| match row {
                MenuRow::Item(item) => (
                    item.id.to_string(),
                    item.label.clone(),
                    item.disabled_hint.clone(),
                ),
                other => panic!("an item row, not {other:?}"),
            })
            .collect();
        let some = |text: &str| Some(text.to_owned());
        assert_eq!(
            shown,
            [
                (
                    "arrange_panes:columns".to_owned(),
                    "Equal Columns".to_owned(),
                    None
                ),
                (
                    "arrange_panes:rows".to_owned(),
                    "Equal Rows".to_owned(),
                    None
                ),
                (
                    "arrange_panes:main_left".to_owned(),
                    "Main Left".to_owned(),
                    None
                ),
                (
                    "arrange_panes:main_top".to_owned(),
                    "Main Top".to_owned(),
                    None
                ),
                (
                    "arrange_panes:main_centered".to_owned(),
                    "Main Centered".to_owned(),
                    None
                ),
                ("arrange_panes:tiled".to_owned(), "Tiled".to_owned(), None),
                ("arrange_panes:dev".to_owned(), "dev".to_owned(), None),
                (
                    "arrange_panes:pair".to_owned(),
                    "pair".to_owned(),
                    some("Holds at most 2 panes")
                ),
            ]
        );
        assert!(arrange(1).is_none(), "a lone pane has nothing to arrange");
    }

    #[test]
    fn momentary_rows_appear_only_when_they_apply() {
        let rows = rows(TerminalMenuInput {
            link: true,
            scrolled_back: true,
            panes: 1,
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
                    Some((item.id.to_string(), item.disabled_hint.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            hints,
            [
                ("copy".to_owned(), Some("Nothing selected".to_owned())),
                ("paste".to_owned(), Some("Shell exited".to_owned())),
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
        let hint = |item: MenuItem| {
            (!item.shortcut.is_empty()).then(|| kbd::text(&item.shortcut))
        };
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
