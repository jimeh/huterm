//! The workspace rows the panel, bar, and dropdown show, the workspace
//! menu (M1) the dropdown opens, and a row's context menu. Everything here
//! is pure so the rows and menus can be tested without a window.

use gpui::WindowId;
use huterm_protocol::{CommandId, HierarchyState, SessionId, WorkspaceId, ids};

use super::menu_spec::{
    Item as MenuItem, MenuSpec, Row as MenuRow, Tag as MenuTag,
};
use super::model::WindowModel;
use crate::assets::Icon;

/// One workspace of the window's session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WorkspaceRow {
    pub(super) workspace: WorkspaceId,
    pub(super) name: String,
    pub(super) automatic: bool,
    pub(super) tabs: usize,
    /// Open windows other than this one that show the workspace.
    pub(super) windows: usize,
}

impl WorkspaceRow {
    /// What assistive technology reads for the row or chip.
    pub(super) fn accessible_name(&self) -> String {
        format!("Workspace: {}, {}", self.name, tabs_text(self.tabs))
    }
}

/// The rows for `session`'s workspaces in session order, as seen from
/// `window`.
pub(super) fn workspace_rows(
    state: &HierarchyState,
    windows: &WindowModel,
    window: WindowId,
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
                windows: windows
                    .other_windows_showing_workspace(workspace, window),
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

fn windows_text(count: usize) -> String {
    if count == 1 {
        "Shown in 1 other window".to_owned()
    } else {
        format!("Shown in {count} other windows")
    }
}

/// The state the workspace menu depends on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WorkspaceMenuInput {
    pub(super) rows: Vec<WorkspaceRow>,
    /// The workspace the window shows, checked in the list.
    pub(super) current: Option<WorkspaceId>,
    /// The window's collapse state: Show Workspace Panel while collapsed,
    /// Collapse Workspace Panel while a panel or bar shows, and neither
    /// while a small window shows the dropdown on its own.
    pub(super) panel: PanelRow,
}

/// The menu's panel toggle row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PanelRow {
    Show,
    Collapse,
    None,
}

const WORKSPACE_PREFIX: &str = "workspace:";
const RENAME_PREFIX: &str = "workspace-rename:";
const CLEAR_PREFIX: &str = "workspace-clear:";
const CLOSE_PREFIX: &str = "workspace-close:";

/// What a workspace menu or row menu pick runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WorkspacePick {
    /// Show the workspace with this raw identity in the window.
    Select(u64),
    /// Rename, clear the name of, or close the workspace with this raw
    /// identity: a row menu acts on its own row.
    Rename(u64),
    ClearName(u64),
    Close(u64),
    /// One of the menu's actions on the shown workspace, a catalog command.
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
    if let Some(raw) = raw(WORKSPACE_PREFIX) {
        return Some(WorkspacePick::Select(raw));
    }
    [
        ids::NEW_WORKSPACE,
        ids::RENAME_WORKSPACE,
        ids::CLEAR_WORKSPACE_NAME,
        ids::CLOSE_WORKSPACE,
        ids::TOGGLE_WORKSPACE_PANEL,
    ]
    .into_iter()
    .find(|command| command.as_str() == id)
    .map(WorkspacePick::Command)
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

/// The workspace menu: a row per workspace in session order, a check on
/// the shown one, the name muted when automatic, its tab count, and a
/// window tag for workspaces other windows show; a hint with one
/// workspace; then New Workspace, Rename Workspace…, Clear Name while the
/// shown workspace has a custom name, Close Workspace… while the session
/// has another workspace, and the panel toggle.
pub(super) fn workspace_menu(input: &WorkspaceMenuInput) -> MenuSpec {
    let mut rows: Vec<MenuRow> = input
        .rows
        .iter()
        .map(|row| {
            let mut item = MenuItem::new(
                format!("{WORKSPACE_PREFIX}{}", row.workspace.get()),
                &row.name,
            )
            .checked(input.current == Some(row.workspace))
            .muted(row.automatic)
            .detail(tabs_text(row.tabs));
            if row.windows > 0 {
                item = item.tag(MenuTag {
                    icon: Some((Icon::AppWindow.asset_path(), "")),
                    text: row.windows.to_string(),
                    label: windows_text(row.windows),
                    warning: false,
                });
            }
            MenuRow::Item(item)
        })
        .collect();
    if input.rows.len() < 2 {
        rows.push(MenuRow::Hint("No other workspaces".to_owned()));
    }
    let custom = input
        .rows
        .iter()
        .any(|row| Some(row.workspace) == input.current && !row.automatic);
    rows.push(MenuRow::Separator);
    rows.push(action(ids::NEW_WORKSPACE, "New Workspace", Icon::Plus));
    rows.push(action(
        ids::RENAME_WORKSPACE,
        "Rename Workspace…",
        Icon::Pencil,
    ));
    if custom {
        rows.push(action(
            ids::CLEAR_WORKSPACE_NAME,
            "Clear Name",
            Icon::Eraser,
        ));
    }
    if input.rows.len() > 1 {
        rows.push(action(
            ids::CLOSE_WORKSPACE,
            "Close Workspace…",
            Icon::Trash,
        ));
    }
    match input.panel {
        PanelRow::Show => {
            rows.push(MenuRow::Separator);
            rows.push(action(
                ids::TOGGLE_WORKSPACE_PANEL,
                "Show Workspace Panel",
                Icon::PanelLeftOpen,
            ));
        }
        PanelRow::Collapse => {
            rows.push(MenuRow::Separator);
            rows.push(action(
                ids::TOGGLE_WORKSPACE_PANEL,
                "Collapse Workspace Panel",
                Icon::PanelLeftClose,
            ));
        }
        PanelRow::None => {}
    }
    MenuSpec::new(rows)
}

/// A row's or chip's context menu, acting on that row's workspace: Rename…,
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
