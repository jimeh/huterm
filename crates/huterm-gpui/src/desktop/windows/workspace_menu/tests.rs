use super::*;
use crate::keymap::Platform;
use huterm_ui::listbox::Row as ListRow;

fn row(raw: u64, name: &str, automatic: bool, tabs: usize) -> WorkspaceRow {
    WorkspaceRow {
        workspace: WorkspaceId::new(raw),
        name: name.to_owned(),
        automatic,
        tabs,
        pinned: false,
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

fn place(index: usize, count: usize, other_session: bool) -> RowPlace {
    RowPlace {
        index,
        count,
        other_session,
    }
}

/// Each item's id with whether it is enabled.
fn enabled(spec: &MenuSpec) -> Vec<(String, bool)> {
    spec.to_listbox(Platform::MacOs)
        .rows
        .iter()
        .filter_map(|row| match row {
            ListRow::Item(item) => Some((item.id.to_string(), item.enabled)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_row_menu_acts_on_its_own_workspace() {
    assert_eq!(
        drawn(&row_menu(&row(2, "release", false, 2), place(1, 3, true))),
        [
            "workspace-rename:2 Rename Workspace…",
            "workspace-clear:2 Clear Name",
            "workspace-pin:2 Pin Workspace",
            "workspace-session:2 Move Workspace to Session…",
            "workspace-up:2 Move Workspace Up",
            "workspace-down:2 Move Workspace Down",
            "workspace-window:2 Move Workspace to New Window",
            "-",
            "workspace-close:2 Close Workspace…",
        ]
    );
    // An automatic name has nothing to clear; a lone workspace cannot close,
    // reorder, or tear off, but can be pinned and can join another session,
    // and a pinned one offers Unpin.
    let pinned = WorkspaceRow {
        pinned: true,
        ..row(1, "Workspace 1", true, 1)
    };
    assert_eq!(
        drawn(&row_menu(&pinned, place(0, 1, true))),
        [
            "workspace-rename:1 Rename Workspace…",
            "workspace-pin:1 Unpin Workspace",
            "workspace-session:1 Move Workspace to Session…",
        ]
    );
    assert_eq!(
        drawn(&row_menu(&pinned, place(0, 1, false))),
        [
            "workspace-rename:1 Rename Workspace…",
            "workspace-pin:1 Unpin Workspace",
        ]
    );
}

#[test]
fn moves_follow_the_rows_place_and_the_other_sessions() {
    let moves = |place: RowPlace| -> Vec<(String, bool)> {
        enabled(&row_menu(&row(4, "dev", false, 1), place))
            .into_iter()
            .filter(|(id, _)| {
                id.starts_with("workspace-up")
                    || id.starts_with("workspace-down")
                    || id.starts_with("workspace-session")
            })
            .collect()
    };
    let at = |up: bool, down: bool, session: Option<bool>| {
        let mut rows: Vec<(String, bool)> = session
            .map(|session| ("workspace-session:4".to_owned(), session))
            .into_iter()
            .collect();
        rows.push(("workspace-up:4".to_owned(), up));
        rows.push(("workspace-down:4".to_owned(), down));
        rows
    };
    assert_eq!(moves(place(0, 3, true)), at(false, true, Some(true)));
    assert_eq!(moves(place(2, 3, true)), at(true, false, Some(true)));
    assert_eq!(
        moves(place(1, 3, false)),
        at(true, true, None),
        "with no other session there is nowhere to move it"
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
        parse_pick("workspace-pin:7"),
        Some(WorkspacePick::TogglePin(7))
    );
    assert_eq!(
        parse_pick("workspace-window:7"),
        Some(WorkspacePick::NewWindow(7))
    );
    assert_eq!(parse_pick("workspace-up:7"), Some(WorkspacePick::MoveUp(7)));
    assert_eq!(
        parse_pick("workspace-down:7"),
        Some(WorkspacePick::MoveDown(7))
    );
    assert_eq!(
        parse_pick("workspace-session:7"),
        Some(WorkspacePick::ToSession(7))
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
    let pinned = WorkspaceRow {
        pinned: true,
        ..row(3, "notes", false, 0)
    };
    assert_eq!(pinned.accessible_name(), "Workspace: notes, pinned, 0 tabs");
}
