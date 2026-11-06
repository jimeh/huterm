//! The session control's state and the session menu's rows.
//!
//! The control and the menu read the projection and the window model and
//! never depend on where the control is drawn, so a second host, such as
//! #27's workspace sidebar, reuses both unchanged. Nothing here mentions
//! workspaces: with one workspace per session the UI talks only about
//! sessions, tabs, and windows. Everything here is pure so the rows can be
//! tested without a window.

use std::collections::HashSet;

use gpui::WindowId;
use huterm_protocol::{CommandId, HierarchyState, SessionId, ids};

use super::menu_spec::{
    Action as MenuAction, Item as MenuItem, MenuSpec, Row as MenuRow,
    Tag as MenuTag,
};
use super::model::WindowModel;
use crate::assets::Icon;

/// What the session control shows: the window's session, its display name,
/// whether that name is the automatic one, and whether its menu is open.
/// Server mode (#45) adds a disconnected flag here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SessionControlState {
    pub(super) session: SessionId,
    pub(super) name: String,
    pub(super) automatic: bool,
    pub(super) open: bool,
}

impl SessionControlState {
    /// The state for a window attached to `session`, or `None` while the
    /// projection does not hold it.
    pub(super) fn derive(
        state: &HierarchyState,
        session: Option<SessionId>,
        open: bool,
    ) -> Option<Self> {
        let info = state.session(session?)?;
        Some(Self {
            session: info.id,
            name: info.display_name().to_owned(),
            automatic: info.custom_name.is_none(),
            open,
        })
    }

    /// What assistive technology reads, whatever the control shows.
    pub(super) fn accessible_name(&self) -> String {
        format!("Session: {}", self.name)
    }
}

/// One session the menu lists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SessionRow {
    pub(super) session: SessionId,
    pub(super) name: String,
    pub(super) automatic: bool,
    pub(super) tabs: usize,
    /// Open windows other than this one that show the session.
    pub(super) windows: usize,
    /// Every terminal in the session has exited.
    pub(super) exited: bool,
}

/// The rows for every session in runtime order, as seen from `window`.
/// `exited` holds the sessions a worker last read as exited.
pub(super) fn session_rows(
    state: &HierarchyState,
    windows: &WindowModel,
    window: WindowId,
    exited: &HashSet<SessionId>,
) -> Vec<SessionRow> {
    state
        .sessions()
        .iter()
        .filter_map(|&session| {
            let info = state.session(session)?;
            let tabs = state
                .session_workspaces(session)
                .unwrap_or_default()
                .iter()
                .filter_map(|workspace| state.workspace_tabs(*workspace))
                .map(<[_]>::len)
                .sum();
            Some(SessionRow {
                session,
                name: info.display_name().to_owned(),
                automatic: info.custom_name.is_none(),
                tabs,
                windows: windows.other_windows_showing(session, window),
                exited: exited.contains(&session),
            })
        })
        .collect()
}

/// The state the session menu's rows depend on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SessionMenuInput {
    pub(super) rows: Vec<SessionRow>,
    /// The window's own session, checked in the list.
    pub(super) current: Option<SessionId>,
    /// The binding that opens the selected session in a new window.
    pub(super) alternate: Option<String>,
}

const SESSION_PREFIX: &str = "session:";
const WINDOW_PREFIX: &str = "session-window:";

/// What a session menu pick runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SessionPick {
    /// Show the session with this raw identity in the window.
    Select(u64),
    /// Open the session with this raw identity in a new window.
    OpenInNewWindow(u64),
    /// One of the menu's actions, a catalog command.
    Command(CommandId),
}

/// The pick an item id names; `None` for an id the menu never lists.
pub(super) fn parse_pick(id: &str) -> Option<SessionPick> {
    if let Some(raw) = id.strip_prefix(WINDOW_PREFIX) {
        return raw.parse().ok().map(SessionPick::OpenInNewWindow);
    }
    if let Some(raw) = id.strip_prefix(SESSION_PREFIX) {
        return raw.parse().ok().map(SessionPick::Select);
    }
    [
        ids::NEW_SESSION,
        ids::RENAME_SESSION,
        ids::CLEAR_SESSION_NAME,
        ids::CLOSE_SESSION,
    ]
    .into_iter()
    .find(|command| command.as_str() == id)
    .map(SessionPick::Command)
}

/// The live session among `state`'s whose raw identity is `raw`.
pub(super) fn resolve(state: &HierarchyState, raw: u64) -> Option<SessionId> {
    state
        .sessions()
        .iter()
        .copied()
        .find(|session| session.get() == raw)
}

fn tabs_text(count: usize) -> String {
    if count == 1 {
        "1 tab".to_owned()
    } else {
        format!("{count} tabs")
    }
}

fn windows_text(count: usize) -> String {
    if count == 1 {
        "Shown in 1 other window".to_owned()
    } else {
        format!("Shown in {count} other windows")
    }
}

/// The session menu: one row per session in runtime order, a check on the
/// window's own, the name muted when automatic, the tab count, a window
/// tag for sessions other windows show, and an "exited" tag; a hint while
/// the window's session is the only one; then New Session, Rename
/// Session…, Clear Name while the window's session has a custom name, and
/// Close Session…. The selected row offers Open in New Window.
pub(super) fn session_menu(input: &SessionMenuInput) -> MenuSpec {
    let mut rows: Vec<MenuRow> = input
        .rows
        .iter()
        .map(|row| {
            let raw = row.session.get();
            let mut item =
                MenuItem::new(format!("{SESSION_PREFIX}{raw}"), &row.name)
                    .checked(input.current == Some(row.session))
                    .muted(row.automatic)
                    .detail(tabs_text(row.tabs))
                    .action(MenuAction {
                        id: format!("{WINDOW_PREFIX}{raw}").into(),
                        label: "Open in New Window".to_owned(),
                        native_title: format!(
                            "Open \"{}\" in New Window",
                            row.name
                        ),
                        icon: Icon::ExternalLink.asset_path(),
                        shortcut: input.alternate.clone(),
                    });
            if row.windows > 0 {
                item = item.tag(MenuTag {
                    icon: Some((Icon::AppWindow.asset_path(), "")),
                    text: row.windows.to_string(),
                    label: windows_text(row.windows),
                    warning: false,
                });
            }
            if row.exited {
                item = item.tag(MenuTag {
                    icon: None,
                    text: "exited".to_owned(),
                    label: "Every shell exited".to_owned(),
                    warning: true,
                });
            }
            MenuRow::Item(item)
        })
        .collect();
    let others = input
        .rows
        .iter()
        .any(|row| Some(row.session) != input.current);
    if !others {
        rows.push(MenuRow::Hint("No other sessions".to_owned()));
    }
    let custom = input
        .rows
        .iter()
        .any(|row| Some(row.session) == input.current && !row.automatic);
    let action = |id: CommandId, label: &str, icon: Icon| {
        MenuRow::Item(MenuItem::new(id.as_str(), label).icon(icon.asset_path()))
    };
    rows.push(MenuRow::Separator);
    rows.push(action(ids::NEW_SESSION, "New Session", Icon::Plus));
    rows.push(action(ids::RENAME_SESSION, "Rename Session…", Icon::Pencil));
    if custom {
        rows.push(action(ids::CLEAR_SESSION_NAME, "Clear Name", Icon::Eraser));
    }
    rows.push(MenuRow::Separator);
    rows.push(action(ids::CLOSE_SESSION, "Close Session…", Icon::Trash));
    MenuSpec::new(rows)
}

#[cfg(test)]
mod tests;
