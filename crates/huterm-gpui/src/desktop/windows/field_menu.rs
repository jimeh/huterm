//! A text field's context menu: Cut, Copy, and Paste, then Select All,
//! with the shortcuts the field's own context binds. Rows stay in place and
//! disable with a reason. The spec is pure so it can be tested without a
//! window. Both the terminal window and Settings open it for the field a
//! right press asked for through `huterm_ui`'s `ContextMenu`: as a native
//! macOS menu when the window's other context menus are native, else in
//! Huterm's own popover.

use gpui::{App, Entity, KeyContext};
use huterm_protocol::{CommandId, CommandInvocation, ids};
use huterm_ui::components::input::Input;

use super::menu_spec::{Item as MenuItem, MenuSpec, Row as MenuRow};
use super::window_menu::shortcut_key;
use crate::keymap::InstalledKeymap;

/// The rows' commands and labels, in order; a separator follows Paste.
const ROWS: [(CommandId, &str); 4] = [
    (ids::TEXT_CUT, "Cut"),
    (ids::TEXT_COPY, "Copy"),
    (ids::TEXT_PASTE, "Paste"),
    (ids::TEXT_SELECT_ALL, "Select All"),
];

/// The state the field menu's rows depend on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FieldMenuInput {
    /// Text is selected for Cut and Copy.
    pub(super) selection: bool,
    /// The field refuses edits, so Cut and Paste have nowhere to go.
    pub(super) read_only: bool,
    /// The field has no text for Select All.
    pub(super) empty: bool,
}

impl FieldMenuInput {
    /// What `field` offers now.
    pub(super) fn of(field: &Input) -> Self {
        Self {
            selection: !field.selection().is_empty(),
            read_only: field.is_read_only(),
            empty: field.is_empty(),
        }
    }
}

/// The rows for `input`, with shortcuts from `keymap` as seen from
/// `contexts`, the field's stack when the menu opened.
pub(super) fn field_menu(
    input: FieldMenuInput,
    keymap: &InstalledKeymap,
    contexts: &[KeyContext],
) -> MenuSpec {
    let refusal = |id: CommandId| match id {
        ids::TEXT_CUT | ids::TEXT_PASTE if input.read_only => Some("Read-only"),
        ids::TEXT_CUT | ids::TEXT_COPY if !input.selection => {
            Some("Nothing selected")
        }
        ids::TEXT_SELECT_ALL if input.empty => Some("Nothing to select"),
        _ => None,
    };
    let mut rows = Vec::new();
    for (id, label) in ROWS {
        if id == ids::TEXT_SELECT_ALL {
            rows.push(MenuRow::Separator);
        }
        let item = MenuItem::new(id.as_str(), label)
            .shortcut(shortcut_key(keymap, contexts, id));
        rows.push(MenuRow::Item(match refusal(id) {
            Some(reason) => item.disabled(Some(reason.to_owned())),
            None => item,
        }));
    }
    MenuSpec::new(rows)
}

/// Runs the picked row `id` on `field`; `false` when the menu offers no
/// such row.
pub(super) fn run_pick(field: &Entity<Input>, id: &str, cx: &mut App) -> bool {
    let Some((command, _)) =
        ROWS.iter().find(|(command, _)| command.as_str() == id)
    else {
        return false;
    };
    let invocation = CommandInvocation::new(*command, Vec::new());
    field
        .update(cx, |field, cx| {
            crate::ui::input::run(field, &invocation, cx)
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{Platform, compile};

    fn input() -> FieldMenuInput {
        FieldMenuInput {
            selection: true,
            read_only: false,
            empty: false,
        }
    }

    fn model(
        input: FieldMenuInput,
        user: &[huterm_config::KeybindingEntry],
    ) -> MenuSpec {
        let (bindings, keymap) =
            compile(Platform::MacOs, user).unwrap().install_parts();
        drop(bindings);
        let contexts = [
            KeyContext::parse("Settings").unwrap(),
            KeyContext::parse("TextField").unwrap(),
        ];
        field_menu(input, &keymap, &contexts)
    }

    /// Item ids in order, with the disabled reason after `!`, and `-` for
    /// separators.
    fn rows(input: FieldMenuInput) -> Vec<String> {
        model(input, &[])
            .rows
            .iter()
            .map(|row| match row {
                MenuRow::Item(item) if item.enabled => item.id.to_string(),
                MenuRow::Item(item) => format!(
                    "{}!{}",
                    item.id,
                    item.disabled_hint.as_deref().unwrap_or_default()
                ),
                MenuRow::Separator => "-".to_owned(),
                MenuRow::Group { .. } => "group".to_owned(),
                MenuRow::Submenu { .. } => "submenu".to_owned(),
            })
            .collect()
    }

    #[test]
    fn the_menu_lists_cut_copy_paste_and_select_all() {
        assert_eq!(
            rows(input()),
            [
                "text_cut",
                "text_copy",
                "text_paste",
                "-",
                "text_select_all"
            ]
        );
    }

    #[test]
    fn rows_disable_with_the_reason_they_cannot_run() {
        assert_eq!(
            rows(FieldMenuInput {
                selection: false,
                ..input()
            }),
            [
                "text_cut!Nothing selected",
                "text_copy!Nothing selected",
                "text_paste",
                "-",
                "text_select_all",
            ]
        );
        // Read-only says why before the selection does.
        assert_eq!(
            rows(FieldMenuInput {
                selection: false,
                read_only: true,
                empty: true,
            }),
            [
                "text_cut!Read-only",
                "text_copy!Nothing selected",
                "text_paste!Read-only",
                "-",
                "text_select_all!Nothing to select",
            ]
        );
    }

    #[test]
    fn shortcuts_follow_the_field_bindings_including_the_users() {
        let shortcuts = |model: &MenuSpec| {
            model
                .rows
                .iter()
                .filter_map(|row| match row {
                    MenuRow::Item(item) => Some(item.shortcut.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let hints =
            |keys: [&str; 4]| keys.map(|key| Some(key.to_owned())).to_vec();
        assert_eq!(
            shortcuts(&model(input(), &[])),
            hints(["cmd-x", "cmd-c", "cmd-v", "cmd-a"])
        );
        // A user binding is shown ahead of the default.
        let user = huterm_config::KeybindingEntry {
            key: "ctrl-alt-shift-x".to_owned(),
            command: "text_cut".to_owned(),
            args: None,
            when: None,
            description: None,
            disabled: None,
        };
        assert_eq!(
            shortcuts(&model(input(), &[user])),
            hints(["ctrl-alt-shift-x", "cmd-c", "cmd-v", "cmd-a"])
        );
    }
}
