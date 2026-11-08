//! The workspace rows the sidebar and the workspace switcher show, and a
//! row's context menu. Everything here is pure so the rows and the menu
//! can be tested without a window.

use huterm_protocol::{CommandId, HierarchyState, SessionId, WorkspaceId, ids};

use super::menu_spec::{Item as MenuItem, MenuSpec, Row as MenuRow};
use crate::assets::Icon;

/// One workspace of the window's session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WorkspaceRow {
    pub(super) workspace: WorkspaceId,
    pub(super) name: String,
    pub(super) automatic: bool,
    pub(super) tabs: usize,
}

impl WorkspaceRow {
    /// What assistive technology reads for the row.
    pub(super) fn accessible_name(&self) -> String {
        format!("Workspace: {}, {}", self.name, tabs_text(self.tabs))
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
const CLOSE_PREFIX: &str = "workspace-close:";

/// What a row menu pick runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WorkspacePick {
    /// Rename, clear the name of, or close the workspace with this raw
    /// identity: a row menu acts on its own row.
    Rename(u64),
    ClearName(u64),
    Close(u64),
    /// A catalog command, such as New Workspace.
    Command(CommandId),
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
    if let Some(raw) = raw(CLOSE_PREFIX) {
        return Some(WorkspacePick::Close(raw));
    }
    (id == ids::NEW_WORKSPACE.as_str())
        .then_some(WorkspacePick::Command(ids::NEW_WORKSPACE))
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

fn action(id: CommandId, label: &str, icon: Icon) -> MenuRow {
    MenuRow::Item(MenuItem::new(id.as_str(), label).icon(icon.asset_path()))
}

/// A row's context menu, acting on that row's workspace: Rename…,
/// Clear Name for a custom name, New Workspace, and Close Workspace… while
/// the session has another workspace.
pub(super) fn row_menu(row: &WorkspaceRow, workspaces: usize) -> MenuSpec {
    let raw = row.workspace.get();
    let item = |id: String, label: &str, icon: Icon| {
        MenuRow::Item(MenuItem::new(id, label).icon(icon.asset_path()))
    };
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
    rows.push(MenuRow::Separator);
    rows.push(action(ids::NEW_WORKSPACE, "New Workspace", Icon::Plus));
    if workspaces > 1 {
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
