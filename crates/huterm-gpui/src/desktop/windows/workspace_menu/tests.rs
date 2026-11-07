use std::fmt::Write as _;

use super::*;
use crate::keymap::Platform;
use huterm_ui::components::kbd;
use huterm_ui::listbox::Row as ListRow;

fn row(
    raw: u64,
    name: &str,
    automatic: bool,
    tabs: usize,
    windows: usize,
) -> WorkspaceRow {
    WorkspaceRow {
        workspace: WorkspaceId::new(raw),
        name: name.to_owned(),
        automatic,
        tabs,
        windows,
    }
}

/// The mockup's workspaces: the shown custom-named one, one another window
/// shows, and an automatic one.
fn three() -> Vec<WorkspaceRow> {
    vec![
        row(1, "dev", false, 3, 0),
        row(2, "release", false, 2, 1),
        row(3, "Workspace 3", true, 1, 0),
    ]
}

/// Each row as `id label` with marks: `*` checked, `~` muted, `[detail]`,
/// `<tag>` per tag, and `(hint)`.
fn drawn(spec: &MenuSpec) -> Vec<String> {
    spec.to_listbox(Platform::MacOs)
        .rows
        .iter()
        .map(|row| match row {
            ListRow::Item(item) => {
                let mut text = format!("{} {}", item.id, item.label);
                if item.checked == Some(true) {
                    text.push('*');
                }
                if item.muted {
                    text.push('~');
                }
                if let Some(detail) = &item.detail {
                    let _ = write!(text, " [{detail}]");
                }
                for tag in &item.tags {
                    let _ = write!(text, " <{}>", kbd::text(&tag.glyphs));
                }
                text
            }
            ListRow::Separator => "-".to_owned(),
            ListRow::Hint(hint) => format!("({hint})"),
            ListRow::Buttons { .. } | ListRow::Submenu { .. } => {
                "other".to_owned()
            }
        })
        .collect()
}

#[test]
fn the_menu_lists_every_workspace_then_the_workspace_actions() {
    let spec = workspace_menu(&WorkspaceMenuInput {
        rows: three(),
        current: Some(WorkspaceId::new(1)),
        panel: PanelRow::Show,
    });
    assert_eq!(
        drawn(&spec),
        [
            "workspace:1 dev* [3 tabs]",
            "workspace:2 release [2 tabs] <1>",
            "workspace:3 Workspace 3~ [1 tab]",
            "-",
            "new_workspace New Workspace",
            "rename_workspace Rename Workspace…",
            "clear_workspace_name Clear Name",
            "close_workspace Close Workspace…",
            "-",
            "toggle_workspace_panel Show Workspace Panel",
        ]
    );
}

#[test]
fn one_automatic_workspace_offers_no_clear_close_or_panel_row() {
    let spec = workspace_menu(&WorkspaceMenuInput {
        rows: vec![row(1, "Workspace 1", true, 3, 0)],
        current: Some(WorkspaceId::new(1)),
        panel: PanelRow::None,
    });
    assert_eq!(
        drawn(&spec),
        [
            "workspace:1 Workspace 1*~ [3 tabs]",
            "(No other workspaces)",
            "-",
            "new_workspace New Workspace",
            "rename_workspace Rename Workspace…",
        ]
    );
    let shown = workspace_menu(&WorkspaceMenuInput {
        rows: three(),
        current: Some(WorkspaceId::new(3)),
        panel: PanelRow::Collapse,
    });
    let rows = drawn(&shown);
    assert!(!rows.iter().any(|row| row.contains("Clear Name")));
    assert_eq!(
        rows.last().map(String::as_str),
        Some("toggle_workspace_panel Collapse Workspace Panel")
    );
}

#[test]
fn a_row_menu_acts_on_its_own_workspace() {
    assert_eq!(
        drawn(&row_menu(&row(2, "release", false, 2, 0), 3)),
        [
            "workspace-rename:2 Rename Workspace…",
            "workspace-clear:2 Clear Name",
            "-",
            "new_workspace New Workspace",
            "-",
            "workspace-close:2 Close Workspace…",
        ]
    );
    // An automatic name has nothing to clear; a lone workspace cannot close.
    assert_eq!(
        drawn(&row_menu(&row(1, "Workspace 1", true, 1, 0), 1)),
        [
            "workspace-rename:1 Rename Workspace…",
            "-",
            "new_workspace New Workspace",
        ]
    );
}

#[test]
fn picks_round_trip_through_their_ids() {
    assert_eq!(parse_pick("workspace:7"), Some(WorkspacePick::Select(7)));
    assert_eq!(
        parse_pick("workspace-rename:7"),
        Some(WorkspacePick::Rename(7))
    );
    assert_eq!(
        parse_pick("workspace-clear:7"),
        Some(WorkspacePick::ClearName(7))
    );
    assert_eq!(
        parse_pick("workspace-close:7"),
        Some(WorkspacePick::Close(7))
    );
    assert_eq!(
        parse_pick("toggle_workspace_panel"),
        Some(WorkspacePick::Command(ids::TOGGLE_WORKSPACE_PANEL))
    );
    assert_eq!(parse_pick("close_session"), None);
    assert_eq!(parse_pick("workspace:x"), None);
}

#[test]
fn rows_name_their_workspace_and_tab_count() {
    assert_eq!(
        row(2, "release", false, 2, 0).accessible_name(),
        "Workspace: release, 2 tabs"
    );
    assert_eq!(
        row(1, "dev", false, 1, 0).accessible_name(),
        "Workspace: dev, 1 tab"
    );
}
