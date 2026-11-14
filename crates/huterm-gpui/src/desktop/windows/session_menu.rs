//! The session switcher's state, the session list's rows, and a row's
//! context menu.
//!
//! The switcher and the list read the projection and the window model and
//! never depend on where the switcher is drawn: beside the sidebar button
//! or in the sidebar. Everything here is pure so the rows and the menu can
//! be tested without a window.

use std::collections::HashSet;

use gpui::WindowId;
use huterm_protocol::{HierarchyState, SessionId};

use super::menu_spec::{Item as MenuItem, MenuSpec, Row as MenuRow};
use super::model::WindowModel;
use crate::assets::Icon;

/// What the session switcher shows: the window's session, its display
/// name, whether that name is the automatic one, and whether the session
/// list is open.
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

    /// What assistive technology reads, whatever the switcher shows.
    pub(super) fn accessible_name(&self) -> String {
        format!("Session: {}", self.name)
    }
}

/// One session the list shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SessionRow {
    pub(super) session: SessionId,
    pub(super) name: String,
    pub(super) automatic: bool,
    pub(super) tabs: usize,
    pub(super) workspaces: usize,
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
            let workspaces =
                state.session_workspaces(session).unwrap_or_default();
            let tabs = workspaces
                .iter()
                .filter_map(|workspace| state.workspace_tabs(*workspace))
                .map(<[_]>::len)
                .sum();
            Some(SessionRow {
                session,
                name: info.display_name().to_owned(),
                automatic: info.custom_name.is_none(),
                tabs,
                workspaces: workspaces.len(),
                windows: windows.other_windows_showing(session, window),
                exited: exited.contains(&session),
            })
        })
        .collect()
}

impl SessionRow {
    /// The row's second line: its workspace and tab counts.
    pub(super) fn detail(&self) -> String {
        format!(
            "{} · {}",
            plural(self.workspaces, "workspace"),
            plural(self.tabs, "tab")
        )
    }

    /// What assistive technology reads for the row, its tags included.
    pub(super) fn accessible_name(&self) -> String {
        let mut name = format!("Session: {}, {}", self.name, self.detail());
        if self.windows > 0 {
            name.push_str(", ");
            name.push_str(&windows_text(self.windows).to_lowercase());
        }
        if self.exited {
            name.push_str(", every shell exited");
        }
        name
    }
}

/// `count` and `noun`, plural unless one.
pub(super) fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The windows tag's label.
pub(super) fn windows_text(count: usize) -> String {
    if count == 1 {
        "Shown in 1 other window".to_owned()
    } else {
        format!("Shown in {count} other windows")
    }
}

const RENAME_PREFIX: &str = "session-rename:";
const CLEAR_PREFIX: &str = "session-clear:";
const WINDOW_PREFIX: &str = "session-window:";
const UP_PREFIX: &str = "session-up:";
const DOWN_PREFIX: &str = "session-down:";
const CLOSE_PREFIX: &str = "session-close:";

/// What a row menu pick runs, on the session with this raw identity: a row
/// menu acts on its own row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SessionPick {
    Rename(u64),
    ClearName(u64),
    OpenInNewWindow(u64),
    /// Moves the session one place up or down the session order.
    MoveUp(u64),
    MoveDown(u64),
    Close(u64),
}

/// The pick an item id names; `None` for an id no menu lists.
pub(super) fn parse_pick(id: &str) -> Option<SessionPick> {
    let raw = |prefix: &str| id.strip_prefix(prefix)?.parse().ok();
    if let Some(raw) = raw(RENAME_PREFIX) {
        return Some(SessionPick::Rename(raw));
    }
    if let Some(raw) = raw(CLEAR_PREFIX) {
        return Some(SessionPick::ClearName(raw));
    }
    if let Some(raw) = raw(WINDOW_PREFIX) {
        return Some(SessionPick::OpenInNewWindow(raw));
    }
    if let Some(raw) = raw(UP_PREFIX) {
        return Some(SessionPick::MoveUp(raw));
    }
    if let Some(raw) = raw(DOWN_PREFIX) {
        return Some(SessionPick::MoveDown(raw));
    }
    raw(CLOSE_PREFIX).map(SessionPick::Close)
}

/// The live session among `state`'s whose raw identity is `raw`.
pub(super) fn resolve(state: &HierarchyState, raw: u64) -> Option<SessionId> {
    state
        .sessions()
        .iter()
        .copied()
        .find(|session| session.get() == raw)
}

/// A row's context menu, acting on that row's session: Rename…, Clear
/// Name for a custom name, Open in New Window, Move Session Up and Down
/// while there is another session (disabled at their ends; the row is at
/// `index` of `count`), and Close Session…, as the workspace rows' menu has
/// them.
pub(super) fn row_menu(
    row: &SessionRow,
    index: usize,
    count: usize,
) -> MenuSpec {
    let raw = row.session.get();
    let entry = |id: String, label: &str, icon: Icon| {
        MenuItem::new(id, label).icon(icon.asset_path())
    };
    let item = |id: String, label: &str, icon: Icon| {
        MenuRow::Item(entry(id, label, icon))
    };
    let mut rows = vec![item(
        format!("{RENAME_PREFIX}{raw}"),
        "Rename Session…",
        Icon::Pencil,
    )];
    if !row.automatic {
        rows.push(item(
            format!("{CLEAR_PREFIX}{raw}"),
            "Clear Name",
            Icon::Eraser,
        ));
    }
    rows.push(item(
        format!("{WINDOW_PREFIX}{raw}"),
        "Open in New Window",
        Icon::ExternalLink,
    ));
    if count > 1 {
        let up = entry(
            format!("{UP_PREFIX}{raw}"),
            "Move Session Up",
            Icon::ArrowUp,
        );
        rows.push(MenuRow::Item(if index == 0 {
            up.disabled(None)
        } else {
            up
        }));
        let down = entry(
            format!("{DOWN_PREFIX}{raw}"),
            "Move Session Down",
            Icon::ArrowDown,
        );
        rows.push(MenuRow::Item(if index + 1 >= count {
            down.disabled(None)
        } else {
            down
        }));
    }
    rows.push(MenuRow::Separator);
    rows.push(item(
        format!("{CLOSE_PREFIX}{raw}"),
        "Close Session…",
        Icon::Trash,
    ));
    MenuSpec::new(rows)
}

#[cfg(test)]
mod tests;
