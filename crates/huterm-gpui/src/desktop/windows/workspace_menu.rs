//! The workspace rows the sidebar and the workspace switcher show, and a
//! row's context menu. Everything here is pure so the rows and the menu
//! can be tested without a window.

use huterm_protocol::{HierarchyState, SessionId, WorkspaceId};

use super::menu_spec::{Item as MenuItem, MenuSpec, Row as MenuRow};
use crate::assets::Icon;

/// One workspace of the window's session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WorkspaceRow {
    pub(super) workspace: WorkspaceId,
    pub(super) name: String,
    pub(super) automatic: bool,
    pub(super) tabs: usize,
    /// The workspace stays when its last tab closes.
    pub(super) pinned: bool,
}

impl WorkspaceRow {
    /// The row's second line: its tab count, for now.
    pub(super) fn detail(&self) -> String {
        tabs_text(self.tabs)
    }

    /// What assistive technology reads for the row.
    pub(super) fn accessible_name(&self) -> String {
        let pinned = if self.pinned { ", pinned" } else { "" };
        format!("Workspace: {}{pinned}, {}", self.name, self.detail())
    }
}

/// The rows for `session`'s workspaces in session order.
pub(super) fn workspace_rows(
    state: &HierarchyState,
    session: Option<SessionId>,
) -> Vec<WorkspaceRow> {
    let Some(workspaces) = session.and_then(|s| state.session_workspaces(s))
    else {
        return Vec::new();
    };
    workspaces
        .iter()
        .filter_map(|&workspace| {
            let info = state.workspace(workspace)?;
            Some(WorkspaceRow {
                workspace,
                name: info.display_name().to_owned(),
                automatic: info.custom_name.is_none(),
                tabs: state.workspace_tabs(workspace).map_or(0, <[_]>::len),
                pinned: info.pinned,
            })
        })
        .collect()
}

pub(super) fn tabs_text(count: usize) -> String {
    if count == 1 {
        "1 tab".to_owned()
    } else {
        format!("{count} tabs")
    }
}

const RENAME_PREFIX: &str = "workspace-rename:";
const CLEAR_PREFIX: &str = "workspace-clear:";
const PIN_PREFIX: &str = "workspace-pin:";
const WINDOW_PREFIX: &str = "workspace-window:";
const UP_PREFIX: &str = "workspace-up:";
const DOWN_PREFIX: &str = "workspace-down:";
const SESSION_PREFIX: &str = "workspace-session:";
const CLOSE_PREFIX: &str = "workspace-close:";

/// What a row menu pick runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WorkspacePick {
    /// Rename, clear the name of, pin or unpin, or close the workspace with
    /// this raw identity: a row menu acts on its own row.
    Rename(u64),
    ClearName(u64),
    TogglePin(u64),
    /// Moves the workspace into a new window and session.
    NewWindow(u64),
    /// Moves the workspace one place up or down its session.
    MoveUp(u64),
    MoveDown(u64),
    /// Moves the workspace to another session, which the palette asks for.
    ToSession(u64),
    Close(u64),
}

/// The pick an item id names; `None` for an id no menu lists.
pub(super) fn parse_pick(id: &str) -> Option<WorkspacePick> {
    let raw = |prefix: &str| id.strip_prefix(prefix)?.parse().ok();
    if let Some(raw) = raw(RENAME_PREFIX) {
        return Some(WorkspacePick::Rename(raw));
    }
    if let Some(raw) = raw(CLEAR_PREFIX) {
        return Some(WorkspacePick::ClearName(raw));
    }
    if let Some(raw) = raw(PIN_PREFIX) {
        return Some(WorkspacePick::TogglePin(raw));
    }
    if let Some(raw) = raw(WINDOW_PREFIX) {
        return Some(WorkspacePick::NewWindow(raw));
    }
    if let Some(raw) = raw(UP_PREFIX) {
        return Some(WorkspacePick::MoveUp(raw));
    }
    if let Some(raw) = raw(DOWN_PREFIX) {
        return Some(WorkspacePick::MoveDown(raw));
    }
    if let Some(raw) = raw(SESSION_PREFIX) {
        return Some(WorkspacePick::ToSession(raw));
    }
    raw(CLOSE_PREFIX).map(WorkspacePick::Close)
}

/// The live workspace among `state`'s whose raw identity is `raw`.
pub(super) fn resolve(state: &HierarchyState, raw: u64) -> Option<WorkspaceId> {
    state
        .sessions()
        .iter()
        .filter_map(|&session| state.session_workspaces(session))
        .flatten()
        .copied()
        .find(|workspace| workspace.get() == raw)
}

/// Where a row sits for its menu: its index among `count` workspace rows,
/// and whether another session exists to move it to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RowPlace {
    pub(super) index: usize,
    pub(super) count: usize,
    pub(super) other_session: bool,
}

/// A row's context menu, acting on that row's workspace: Rename…,
/// Clear Name for a custom name, Pin Workspace or Unpin Workspace, Move
/// Workspace to Session… while another session exists, and, while the
/// session has another workspace, Move Workspace Up and Down (disabled at
/// their ends), Move to New Window, and Close Workspace…, as the session
/// rows' menu has them.
pub(super) fn row_menu(row: &WorkspaceRow, place: RowPlace) -> MenuSpec {
    let raw = row.workspace.get();
    let entry = |id: String, label: &str, icon: Icon| {
        MenuItem::new(id, label).icon(icon.asset_path())
    };
    let item = |id: String, label: &str, icon: Icon| {
        MenuRow::Item(entry(id, label, icon))
    };
    let workspaces = place.count;
    let mut rows = vec![item(
        format!("{RENAME_PREFIX}{raw}"),
        "Rename Workspace…",
        Icon::Pencil,
    )];
    if !row.automatic {
        rows.push(item(
            format!("{CLEAR_PREFIX}{raw}"),
            "Clear Name",
            Icon::Eraser,
        ));
    }
    rows.push(if row.pinned {
        item(
            format!("{PIN_PREFIX}{raw}"),
            "Unpin Workspace",
            Icon::PinOff,
        )
    } else {
        item(format!("{PIN_PREFIX}{raw}"), "Pin Workspace", Icon::Pin)
    });
    if place.other_session {
        rows.push(item(
            format!("{SESSION_PREFIX}{raw}"),
            "Move Workspace to Session…",
            Icon::Layers,
        ));
    }
    if workspaces > 1 {
        let up = entry(
            format!("{UP_PREFIX}{raw}"),
            "Move Workspace Up",
            Icon::ArrowUp,
        );
        rows.push(MenuRow::Item(if place.index == 0 {
            up.disabled(None)
        } else {
            up
        }));
        let down = entry(
            format!("{DOWN_PREFIX}{raw}"),
            "Move Workspace Down",
            Icon::ArrowDown,
        );
        rows.push(MenuRow::Item(if place.index + 1 >= workspaces {
            down.disabled(None)
        } else {
            down
        }));
        rows.push(item(
            format!("{WINDOW_PREFIX}{raw}"),
            "Move Workspace to New Window",
            Icon::AppWindow,
        ));
        rows.push(MenuRow::Separator);
        rows.push(item(
            format!("{CLOSE_PREFIX}{raw}"),
            "Close Workspace…",
            Icon::Trash,
        ));
    }
    MenuSpec::new(rows)
}

#[cfg(test)]
mod tests;
