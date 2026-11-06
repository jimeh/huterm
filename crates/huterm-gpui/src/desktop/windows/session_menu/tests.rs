use std::fmt::Write as _;

use super::*;
use crate::desktop::windows::menu_spec::NativeItem;
use crate::keymap::Platform;
use huterm_ui::components::kbd;
use huterm_ui::listbox::{Row as ListRow, TagTone};

fn row(
    raw: u64,
    name: &str,
    automatic: bool,
    tabs: usize,
    windows: usize,
    exited: bool,
) -> SessionRow {
    SessionRow {
        session: SessionId::new(raw),
        name: name.to_owned(),
        automatic,
        tabs,
        windows,
        exited,
    }
}

fn input(rows: Vec<SessionRow>, current: u64) -> SessionMenuInput {
    SessionMenuInput {
        rows,
        current: Some(SessionId::new(current)),
        alternate: Some("cmd-enter".to_owned()),
    }
}

/// The mockup's four sessions: this window's custom-named one, an
/// automatic one, one another window shows, and one whose shells exited.
fn four() -> Vec<SessionRow> {
    vec![
        row(1, "huterm", false, 3, 0, false),
        row(2, "Session 2", true, 1, 0, false),
        row(3, "infra", false, 2, 1, false),
        row(4, "Session 4", true, 1, 0, true),
    ]
}

/// Each row as `id label` with marks: `*` checked, `~` muted, `[detail]`,
/// `<tag>` per tag, `!` warning tone, and `(hint)`.
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
                    let warning = if tag.tone == TagTone::Warning {
                        "!"
                    } else {
                        ""
                    };
                    let _ =
                        write!(text, " <{}{warning}>", kbd::text(&tag.glyphs));
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
fn the_menu_lists_every_session_then_the_session_actions() {
    let spec = session_menu(&input(four(), 1));
    assert_eq!(
        drawn(&spec),
        [
            "session:1 huterm* [3 tabs]",
            "session:2 Session 2~ [1 tab]",
            "session:3 infra [2 tabs] <1>",
            "session:4 Session 4~ [1 tab] <exited!>",
            "-",
            "new_session New Session",
            "rename_session Rename Session…",
            "clear_session_name Clear Name",
            "-",
            "close_session Close Session…",
        ]
    );
    // The window tag names its count of other windows.
    let model = spec.to_listbox(Platform::MacOs);
    let ListRow::Item(infra) = &model.rows[2] else {
        panic!("a session row");
    };
    assert_eq!(infra.tags[0].label, "Shown in 1 other window");
    // Every session row offers Open in New Window with the platform key.
    assert_eq!(
        infra.action.as_ref().map(|action| (
            action.id.to_string(),
            action.label.as_str(),
            kbd::text(&action.shortcut)
        )),
        Some((
            "session-window:3".to_owned(),
            "Open in New Window",
            "⌘↩".to_owned()
        ))
    );
    // Action rows carry icons in the leading column.
    let ListRow::Item(new) = &model.rows[5] else {
        panic!("New Session");
    };
    assert_eq!(new.icon.as_deref(), Some(Icon::Plus.asset_path()));
}

#[test]
fn an_automatic_name_offers_no_clear_and_a_lone_session_shows_a_hint() {
    let automatic = session_menu(&input(four(), 2));
    assert!(
        !drawn(&automatic)
            .iter()
            .any(|row| row.contains("clear_session_name")),
        "an automatic name has nothing to clear"
    );
    let lone =
        session_menu(&input(vec![row(1, "huterm", false, 3, 0, false)], 1));
    assert_eq!(
        drawn(&lone),
        [
            "session:1 huterm* [3 tabs]",
            "(No other sessions)",
            "-",
            "new_session New Session",
            "rename_session Rename Session…",
            "clear_session_name Clear Name",
            "-",
            "close_session Close Session…",
        ]
    );
    // Nothing in the menu mentions workspaces.
    for spec in [automatic, lone] {
        assert!(
            drawn(&spec)
                .iter()
                .all(|row| !row.to_lowercase().contains("workspace")),
            "{:?}",
            drawn(&spec)
        );
    }
}

#[test]
fn picks_name_a_session_by_its_raw_identity_or_a_session_command() {
    assert_eq!(parse_pick("session:7"), Some(SessionPick::Select(7)));
    assert_eq!(
        parse_pick("session-window:7"),
        Some(SessionPick::OpenInNewWindow(7))
    );
    for command in [
        ids::NEW_SESSION,
        ids::RENAME_SESSION,
        ids::CLEAR_SESSION_NAME,
        ids::CLOSE_SESSION,
    ] {
        assert_eq!(
            parse_pick(command.as_str()),
            Some(SessionPick::Command(command))
        );
    }
    assert_eq!(parse_pick("session:x"), None);
    assert_eq!(parse_pick("new_tab"), None, "only the menu's own actions");
}

#[test]
fn the_native_menu_keeps_check_and_name_with_the_detail_as_a_subtitle() {
    let native = session_menu(&input(four(), 1)).to_native(true);
    let shown: Vec<String> = native
        .items
        .iter()
        .map(|item| match item {
            NativeItem::Item {
                id,
                title,
                enabled,
                checked,
                subtitle,
                alternate,
                ..
            } => format!(
                "{}{id}={title}{}{}{}",
                if *alternate { "~" } else { "" },
                if *enabled { "" } else { "!" },
                if *checked == Some(true) { "*" } else { "" },
                subtitle
                    .as_deref()
                    .map(|text| format!(" ({text})"))
                    .unwrap_or_default(),
            ),
            NativeItem::Separator => "-".to_owned(),
            NativeItem::Submenu { title, .. } => format!("{title}>"),
        })
        .collect();
    assert_eq!(
        shown,
        [
            "session:1=huterm* (3 tabs)",
            "~session-window:1=Open \"huterm\" in New Window",
            "session:2=Session 2 (1 tab)",
            "~session-window:2=Open \"Session 2\" in New Window",
            "session:3=infra (2 tabs · Shown in 1 other window)",
            "~session-window:3=Open \"infra\" in New Window",
            "session:4=Session 4 (1 tab · Every shell exited)",
            "~session-window:4=Open \"Session 4\" in New Window",
            "-",
            "new_session=New Session",
            "rename_session=Rename Session…",
            "clear_session_name=Clear Name",
            "-",
            "close_session=Close Session…",
        ]
    );
    // An alternate is picked by its own tag.
    assert_eq!(native.id_at(1).as_deref(), Some("session-window:1"));
    // The lone-session hint is a disabled item that is never picked.
    let lone =
        session_menu(&input(vec![row(1, "huterm", false, 3, 0, false)], 1))
            .to_native(false);
    assert!(matches!(
        &lone.items[2],
        NativeItem::Item { title, enabled: false, .. } if title == "No other sessions"
    ));
    assert_eq!(lone.id_at(2), None);
}
