use super::*;
use crate::keymap::Platform;
use huterm_ui::listbox::Row as ListRow;

fn row(raw: u64, name: &str, automatic: bool) -> SessionRow {
    SessionRow {
        session: SessionId::new(raw),
        name: name.to_owned(),
        automatic,
        tabs: 3,
        workspaces: 2,
        windows: 0,
        exited: false,
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
fn a_row_menu_acts_on_its_own_session() {
    assert_eq!(
        drawn(&row_menu(&row(2, "infra", false))),
        [
            "session-rename:2 Rename Session…",
            "session-clear:2 Clear Name",
            "session-window:2 Open in New Window",
            "-",
            "session-close:2 Close Session…",
        ]
    );
    // An automatic name has nothing to clear.
    assert_eq!(
        drawn(&row_menu(&row(1, "Session 1", true))),
        [
            "session-rename:1 Rename Session…",
            "session-window:1 Open in New Window",
            "-",
            "session-close:1 Close Session…",
        ]
    );
}

#[test]
fn picks_round_trip_through_their_ids() {
    for (id, pick) in [
        ("session-rename:7", SessionPick::Rename(7)),
        ("session-clear:7", SessionPick::ClearName(7)),
        ("session-window:7", SessionPick::OpenInNewWindow(7)),
        ("session-close:7", SessionPick::Close(7)),
    ] {
        assert_eq!(parse_pick(id), Some(pick), "{id}");
    }
    assert_eq!(parse_pick("new_session"), None);
    assert_eq!(parse_pick("session-close:x"), None);
}

#[test]
fn rows_count_workspaces_and_tabs_and_name_their_tags() {
    let mut session = row(3, "infra", false);
    assert_eq!(session.detail(), "2 workspaces · 3 tabs");
    assert_eq!(
        session.accessible_name(),
        "Session: infra, 2 workspaces · 3 tabs"
    );
    session.workspaces = 1;
    session.tabs = 1;
    session.windows = 2;
    session.exited = true;
    assert_eq!(session.detail(), "1 workspace · 1 tab");
    assert_eq!(
        session.accessible_name(),
        "Session: infra, 1 workspace · 1 tab, shown in 2 other windows, \
         every shell exited"
    );
}
