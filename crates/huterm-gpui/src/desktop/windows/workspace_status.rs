//! The status dot on workspace rows: each workspace rolled up to its most
//! urgent tab, from statuses the runtime's viewer-free feed publishes, so
//! a window learns about workspaces it does not show.
//!
//! The desktop keeps the latest read ([`Statuses`]); each window keeps its
//! own bell baselines ([`BellBaselines`]), so a bell marks a workspace in a
//! window until that window shows it, and two windows can differ. Rows of
//! the shown workspace show no bell: its tabs already do.

use std::collections::HashMap;
use std::sync::Arc;

use huterm_core::TerminalStatuses;
use huterm_protocol::{
    HierarchyState, SessionId, TabId, TerminalId, TerminalLifecycle,
    TerminalStatus, WorkspaceId,
};

/// The latest statuses read from the feed.
#[derive(Debug, Default)]
pub(super) struct Statuses {
    /// The feed revision of the read applied last.
    revision: Option<u64>,
    /// Counts the reads applied, so a window can tell which terminals came
    /// after it started following.
    reads: u64,
    terminals: HashMap<TerminalId, Arc<TerminalStatus>>,
    /// For each terminal, the read that first listed it and the bells
    /// counted as seen then: all of them in the first read, which lists the
    /// terminals that ran before the desktop looked, else none, since the
    /// terminal started after the previous read.
    introduced: HashMap<TerminalId, (u64, u64)>,
}

impl Statuses {
    /// Applies `read` unless one newer already applied, and returns whether
    /// it did.
    pub(super) fn apply(&mut self, read: TerminalStatuses) -> bool {
        if self.revision.is_some_and(|applied| read.revision < applied) {
            return false;
        }
        let first = self.revision.is_none();
        self.revision = Some(read.revision);
        self.reads += 1;
        self.terminals = read.statuses.into_iter().collect();
        let reads = self.reads;
        self.introduced
            .retain(|terminal, _| self.terminals.contains_key(terminal));
        for (terminal, status) in &self.terminals {
            self.introduced.entry(*terminal).or_insert_with(|| {
                (reads, if first { status.bells } else { 0 })
            });
        }
        true
    }

    pub(super) fn get(&self, terminal: TerminalId) -> Option<&TerminalStatus> {
        self.terminals.get(&terminal).map(Arc::as_ref)
    }

    /// Whether `terminal`'s status differs from `before`'s, or it came or
    /// went.
    pub(super) fn changed_since(
        &self,
        before: &HashMap<TerminalId, Arc<TerminalStatus>>,
        terminal: TerminalId,
    ) -> bool {
        match (self.terminals.get(&terminal), before.get(&terminal)) {
            (Some(now), Some(then)) => !Arc::ptr_eq(now, then),
            (now, then) => now.is_some() != then.is_some(),
        }
    }

    /// A copy of the statuses, to compare a later read against.
    pub(super) fn snapshot(&self) -> HashMap<TerminalId, Arc<TerminalStatus>> {
        self.terminals.clone()
    }
}

/// A window's bell count per terminal: the count when it last showed the
/// terminal's workspace, or when it first saw the terminal.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct BellBaselines {
    seen: HashMap<TerminalId, u64>,
    /// The read the window first followed.
    since: Option<u64>,
}

impl BellBaselines {
    /// Follows the statuses and the projection for a window showing `shown`
    /// of `session`, after a read or a hierarchy change: the shown
    /// workspace's terminals are seen now. Another workspace's terminal the
    /// window has no count for takes the count seen when it started, if it
    /// started after the window began following, so a bell it rang before
    /// the projection listed it still counts; else its current count, as a
    /// first sight. Terminals that went are dropped.
    pub(super) fn follow(
        &mut self,
        state: &HierarchyState,
        session: Option<SessionId>,
        shown: Option<WorkspaceId>,
        statuses: &Statuses,
    ) {
        if statuses.reads == 0 {
            return;
        }
        let since = *self.since.get_or_insert(statuses.reads);
        self.seen
            .retain(|terminal, _| statuses.get(*terminal).is_some());
        let workspaces = session
            .and_then(|session| state.session_workspaces(session))
            .unwrap_or_default();
        for &workspace in workspaces {
            for terminal in workspace_terminals(state, workspace) {
                let Some(status) = statuses.get(terminal) else {
                    continue;
                };
                if Some(workspace) == shown {
                    self.seen.insert(terminal, status.bells);
                    continue;
                }
                let introduced = statuses
                    .introduced
                    .get(&terminal)
                    .filter(|(read, _)| *read > since)
                    .map(|(_, bells)| *bells);
                self.seen
                    .entry(terminal)
                    .or_insert(introduced.unwrap_or(status.bells));
            }
        }
    }

    /// Marks every bell `workspace`'s terminals rang as seen, as a window
    /// leaving the workspace it showed does.
    pub(super) fn see(
        &mut self,
        state: &HierarchyState,
        workspace: WorkspaceId,
        statuses: &Statuses,
    ) {
        for terminal in workspace_terminals(state, workspace) {
            if let Some(status) = statuses.get(terminal) {
                self.seen.insert(terminal, status.bells);
            }
        }
    }

    /// Whether `terminal` rang a bell this window has not seen. A terminal
    /// without a baseline yet has none.
    pub(super) fn unseen(&self, terminal: TerminalId, bells: u64) -> bool {
        self.seen.get(&terminal).is_some_and(|seen| bells > *seen)
    }
}

/// A workspace row's dot, in the tab bar's order from the least to the
/// most urgent.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum Dot {
    /// A program holds the foreground or reports progress.
    Running,
    /// A bell rang that this window has not seen.
    Bell,
    /// A shell exited and its tab stays.
    Exited,
    /// A terminal failed.
    Failed,
}

impl Dot {
    fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Bell => "Bell",
            Self::Exited => "Exited",
            Self::Failed => "Failed",
        }
    }
}

/// A workspace's rolled-up dot and the text naming its tabs by state, most
/// urgent first, such as "Bell: vim · Running: cargo, make".
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RowStatus {
    pub(super) dot: Dot,
    pub(super) text: String,
}

/// One terminal's dot; `bell` says whether it rang unseen.
fn terminal_dot(status: &TerminalStatus, bell: bool) -> Option<Dot> {
    if status.last_failure() > 0 {
        Some(Dot::Failed)
    } else if status.lifecycle != TerminalLifecycle::Running {
        Some(Dot::Exited)
    } else if bell {
        Some(Dot::Bell)
    } else if status.progress.is_some()
        || status
            .metadata
            .foreground_process()
            .is_some_and(|process| !process.is_empty())
    {
        Some(Dot::Running)
    } else {
        None
    }
}

/// `workspace`'s terminals, in tab and traversal order.
fn workspace_terminals(
    state: &HierarchyState,
    workspace: WorkspaceId,
) -> impl Iterator<Item = TerminalId> + '_ {
    state
        .workspace_tabs(workspace)
        .unwrap_or_default()
        .iter()
        .filter_map(|tab| state.tab(*tab))
        .flat_map(|tab| tab.layout.panes())
        .map(|pane| pane.terminal_id)
}

/// `workspace`'s row status: each tab at its most urgent pane, the row at
/// its most urgent tab. `bells` is the window's baselines, or `None` for
/// the workspace it shows, whose tabs show their own bells.
pub(super) fn workspace_status(
    state: &HierarchyState,
    workspace: WorkspaceId,
    statuses: &Statuses,
    bells: Option<&BellBaselines>,
) -> Option<RowStatus> {
    let mut tabs: Vec<(Dot, String)> = Vec::new();
    for &tab in state.workspace_tabs(workspace).unwrap_or_default() {
        if let Some(entry) = tab_status(state, tab, statuses, bells) {
            tabs.push(entry);
        }
    }
    let dot = tabs.iter().map(|(dot, _)| *dot).max()?;
    let mut groups: Vec<String> = Vec::new();
    for kind in [Dot::Failed, Dot::Exited, Dot::Bell, Dot::Running] {
        let names: Vec<&str> = tabs
            .iter()
            .filter(|(dot, _)| *dot == kind)
            .map(|(_, name)| name.as_str())
            .collect();
        if !names.is_empty() {
            groups.push(format!("{}: {}", kind.label(), names.join(", ")));
        }
    }
    Some(RowStatus {
        dot,
        text: groups.join(" · "),
    })
}

/// A tab's dot and name: its custom name, else its first pane's title,
/// else that pane's fallback name.
fn tab_status(
    state: &HierarchyState,
    tab: TabId,
    statuses: &Statuses,
    bells: Option<&BellBaselines>,
) -> Option<(Dot, String)> {
    let info = state.tab(tab)?;
    let panes = info.layout.panes();
    let dot = panes
        .iter()
        .filter_map(|pane| {
            let status = statuses.get(pane.terminal_id)?;
            let bell = bells.is_some_and(|bells| {
                bells.unseen(pane.terminal_id, status.bells)
            });
            terminal_dot(status, bell)
        })
        .max()?;
    let first = panes.first()?;
    let name = info.custom_name.clone().unwrap_or_else(|| {
        statuses
            .get(first.terminal_id)
            .map(|status| status.title.trim())
            .filter(|title| !title.is_empty())
            .map_or_else(|| first.fallback_name.clone(), str::to_owned)
    });
    Some((dot, name))
}

#[cfg(test)]
mod tests;
