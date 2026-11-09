use super::*;
use crate::keymap::Platform;
use huterm_ui::listbox::Row as ListRow;

fn row(raw: u64, name: &str, automatic: bool, tabs: usize) -> WorkspaceRow {
    WorkspaceRow {
        workspace: WorkspaceId::new(raw),
        name: name.to_owned(),
        automatic,
        tabs,
    }
}

/// Each row as `id label`, separators as `-`.
fn drawn(spec: &MenuSpec) -> Vec<String> {
    spec.to_listbox(Platform::MacOs)
        .rows
        .iter()
        .map(|row| match row {
            ListRow::Item(item) => format!("{} {}", item.id, item.label),
            ListRow::Separator => "-".to_owned(),
            _ => "other".to_owned(),
        })
        .collect()
}

#[test]
fn a_row_menu_acts_on_its_own_workspace() {
    assert_eq!(
        drawn(&row_menu(&row(2, "release", false, 2), 3)),
        [
            "workspace-rename:2 Rename Workspace…",
            "workspace-clear:2 Clear Name",
            "-",
            "workspace-close:2 Close Workspace…",
        ]
    );
    // An automatic name has nothing to clear; a lone workspace cannot close.
    assert_eq!(
        drawn(&row_menu(&row(1, "Workspace 1", true, 1), 1)),
        ["workspace-rename:1 Rename Workspace…"]
    );
}

#[test]
fn picks_round_trip_through_their_ids() {
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
    assert_eq!(parse_pick("new_workspace"), None);
    assert_eq!(parse_pick("close_session"), None);
    assert_eq!(parse_pick("workspace-close:x"), None);
}

#[test]
fn rows_name_their_workspace_and_tab_count() {
    assert_eq!(
        row(2, "release", false, 2).accessible_name(),
        "Workspace: release, 2 tabs"
    );
    assert_eq!(
        row(1, "dev", false, 1).accessible_name(),
        "Workspace: dev, 1 tab"
    );
}
