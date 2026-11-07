//! Restorable runtime structure: capture without processes, and a restore
//! that keeps every saved identity.
//!
//! A restore runs in two steps so the Mux lock covers only insertion:
//! [`SpawnedTerminals::spawn`] starts every captured terminal outside the
//! lock, then [`Mux::restore`] validates the capture and inserts the
//! structure with the spawned runtimes. This is distinct from
//! [`Mux::restore_tabs`], which undoes a layout change.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use huterm_protocol::{
    HierarchyEvent, MAX_HIERARCHY_ITEMS, MAX_NAME_BYTES, PaneId, PaneInfo,
    PaneLayout, PaneNode, RuntimeCapture, RuntimeId, SessionCapture, SessionId,
    SessionInfo, TabId, TabInfo, TerminalCapture, TerminalCommand, TerminalId,
    TerminalLaunch, WorkspaceCapture, WorkspaceId, WorkspaceInfo,
};

use super::{
    Mux, MuxError, SESSION_NAME, Session, Tab, WORKSPACE_NAME, Workspace,
    automatic_ordinal, fallback_name, layout_items, position,
};
use crate::{RuntimeError, TerminalRuntime};

/// Captures carry unscoped identities; a restore scopes them.
const UNSCOPED: RuntimeId = RuntimeId::new(0);

/// Terminals started for a [`Mux::restore`], with the ones that failed.
///
/// Dropping the set stops and joins every runtime it still holds, as
/// [`Self::shutdown`] does.
#[derive(Debug)]
pub struct SpawnedTerminals {
    runtime: RuntimeId,
    terminals: BTreeMap<TerminalId, Spawned>,
    failures: Vec<SpawnFailure>,
}

#[derive(Debug)]
struct Spawned {
    runtime: TerminalRuntime,
    fallback_name: String,
}

/// A captured terminal that failed to start.
#[derive(Clone, Debug)]
pub struct SpawnFailure {
    /// The terminal.
    pub terminal: TerminalId,
    /// Why it failed.
    pub error: RuntimeError,
}

/// What a [`Mux::restore`] left out.
#[derive(Clone, Debug, Default)]
pub struct RestoreReport {
    /// Panes whose terminal failed to start, which left their layouts.
    pub failed: Vec<FailedPane>,
    /// Tabs removed because none of their panes started.
    pub removed_tabs: Vec<TabId>,
    /// Restored sessions that hold no tab.
    pub empty_sessions: Vec<SessionId>,
}

/// A pane a restore left out because its terminal failed to start.
#[derive(Clone, Debug)]
pub struct FailedPane {
    /// The terminal that failed.
    pub terminal: TerminalId,
    /// The pane that showed it.
    pub pane: PaneId,
    /// The pane's tab.
    pub tab: TabId,
    /// Why the terminal failed.
    pub error: RuntimeError,
}

impl SpawnedTerminals {
    /// Starts every terminal a valid `capture`'s panes show, in record order,
    /// with the command `command` returns for it, in `runtime`'s viewer
    /// scope. Each spawn blocks until its PTY and child start, so call this
    /// on a worker, never with the Mux locked. A spawn that fails is
    /// recorded and the rest continue.
    ///
    /// `cancelled` runs before each spawn; once it returns true, every
    /// terminal already started is stopped and joined.
    ///
    /// # Errors
    /// Rejects an invalid capture before spawning anything. Returns
    /// [`MuxError::RestoreCancelled`] after cancellation, or the cleanup
    /// failure if stopping a started terminal failed.
    pub fn spawn(
        runtime: RuntimeId,
        capture: &RuntimeCapture,
        mut command: impl FnMut(&TerminalCapture) -> TerminalCommand,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Self, MuxError> {
        capture.validate().map_err(MuxError::InvalidCapture)?;
        within_limits(capture)?;
        let mut spawned = Self {
            runtime,
            terminals: BTreeMap::new(),
            failures: Vec::new(),
        };
        // A valid capture holds one record per pane terminal.
        for record in &capture.terminals {
            if cancelled() {
                spawned.shutdown()?;
                return Err(MuxError::RestoreCancelled);
            }
            let id = TerminalId::in_runtime(runtime, record.id.get());
            let command = command(record);
            match TerminalRuntime::spawn_in(runtime, id, &command) {
                Ok(started) => {
                    spawned.terminals.insert(
                        id,
                        Spawned {
                            runtime: started,
                            fallback_name: fallback_name(&command),
                        },
                    );
                }
                Err(error) => spawned.failures.push(SpawnFailure {
                    terminal: id,
                    error,
                }),
            }
        }
        Ok(spawned)
    }

    /// The runtime whose viewer scope the terminals use.
    #[must_use]
    pub fn runtime(&self) -> RuntimeId {
        self.runtime
    }

    /// The terminals that started, in identity order.
    pub fn started(&self) -> impl Iterator<Item = TerminalId> + '_ {
        self.terminals.keys().copied()
    }

    /// The terminals that failed to start, in spawn order.
    #[must_use]
    pub fn failures(&self) -> &[SpawnFailure] {
        &self.failures
    }

    /// Stops and joins every started terminal: closes them all first, then
    /// joins each.
    ///
    /// # Errors
    /// Returns the last cleanup failure after stopping every terminal.
    pub fn shutdown(&mut self) -> Result<(), RuntimeError> {
        let terminals = std::mem::take(&mut self.terminals);
        for spawned in terminals.values() {
            let _ = spawned.runtime.client().close();
        }
        let mut failure = None;
        for spawned in terminals.into_values() {
            if let Err(error) = spawned.runtime.shutdown() {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// Whether the started and failed terminals are exactly the ones the
    /// capture's panes show, each once.
    fn covers(&self, capture: &RuntimeCapture) -> bool {
        let mut seen = HashSet::new();
        let accounted = self
            .terminals
            .keys()
            .chain(self.failures.iter().map(|failure| &failure.terminal))
            .all(|id| seen.insert(*id));
        accounted
            && seen
                == pane_terminals(capture)
                    .into_iter()
                    .map(|id| TerminalId::in_runtime(self.runtime, id.get()))
                    .collect()
    }
}

impl Drop for SpawnedTerminals {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Refuses a capture that holds a name longer than [`MAX_NAME_BYTES`] or
/// more than [`MAX_HIERARCHY_ITEMS`] items, which a restore could not
/// recreate without breaking the invariants every other path holds. Saved
/// state keeps its own, looser bounds, so the check lives here.
fn within_limits(capture: &RuntimeCapture) -> Result<(), MuxError> {
    let long = |name: &str| name.len() > MAX_NAME_BYTES;
    let mut items = 0;
    for session in &capture.sessions {
        items += 1;
        let info = &session.info;
        let mut names = vec![
            Some(info.automatic_name.as_str()),
            info.custom_name.as_deref(),
        ];
        for workspace in &session.workspaces {
            items += 1;
            let info = &workspace.info;
            names.extend([
                Some(info.automatic_name.as_str()),
                info.custom_name.as_deref(),
            ]);
            for tab in &workspace.tabs {
                items += 1 + layout_items(&tab.layout);
                names.push(tab.custom_name.as_deref());
                names.extend(
                    tab.layout
                        .panes()
                        .into_iter()
                        .map(|pane| pane.custom_name.as_deref()),
                );
            }
        }
        if names.into_iter().flatten().any(long) {
            return Err(MuxError::InvalidName);
        }
    }
    if items > MAX_HIERARCHY_ITEMS {
        return Err(MuxError::LimitExceeded);
    }
    Ok(())
}

/// Every terminal the capture's panes show, in pane traversal order.
/// The highest creation ordinal among automatic names of one kind, or
/// zero when none carries one.
fn resumed_ordinal<'a>(
    kind: &str,
    names: impl Iterator<Item = &'a str>,
) -> u64 {
    names
        .filter_map(|name| automatic_ordinal(kind, name))
        .max()
        .unwrap_or(0)
}

fn pane_terminals(capture: &RuntimeCapture) -> Vec<TerminalId> {
    capture
        .tabs()
        .flat_map(|tab| tab.layout.panes())
        .map(|pane| pane.terminal_id)
        .collect()
}

impl Mux {
    /// Captures every session that holds a tab or an attachment, with its
    /// workspaces, tabs, and layouts in order, the allocator watermark,
    /// the creation ordinals, and each shown terminal's directory
    /// candidates, read from its published status without waiting on its
    /// runtime. A tabless session a window still shows, such as one whose
    /// last tab closed beside an empty workspace, is kept; a tabless
    /// session nothing holds, which a transfer leaves in flight, is left
    /// out.
    /// Attachments, viewers, layout history, and process state are never
    /// captured. Identities come out unscoped.
    #[must_use]
    pub fn capture_runtime(&self) -> RuntimeCapture {
        let mut sessions = Vec::new();
        let mut terminals = Vec::new();
        for session in &self.sessions {
            let workspaces: Vec<&Workspace> = session
                .workspaces
                .iter()
                .map(|id| &self.workspaces[id])
                .collect();
            if workspaces.iter().all(|workspace| workspace.tabs.is_empty())
                && !self.attachments.values().any(|held| *held == session.id)
            {
                continue;
            }
            let mut captured = Vec::new();
            for workspace in workspaces {
                let mut tabs = Vec::new();
                for tab in &workspace.tabs {
                    terminals.extend(
                        tab.layout.panes().into_iter().map(|pane| {
                            self.capture_terminal(pane.terminal_id)
                        }),
                    );
                    tabs.push(unscoped_tab(tab));
                }
                captured.push(WorkspaceCapture {
                    info: WorkspaceInfo {
                        id: WorkspaceId::new(workspace.id.get()),
                        ..workspace.info()
                    },
                    tabs,
                });
            }
            sessions.push(SessionCapture {
                info: SessionInfo {
                    id: SessionId::new(session.id.get()),
                    ..session.info()
                },
                workspaces: captured,
            });
        }
        RuntimeCapture {
            socket_name: self.socket_name.clone(),
            next_id: self.next_id,
            session_ordinal: self.session_ordinal,
            workspace_ordinal: self.workspace_ordinal,
            sessions,
            terminals,
        }
    }

    /// A terminal's directory candidates, most preferred first: the
    /// effective directory when it is local, the root shell's own last
    /// local report, then the launch directory, without repeats.
    fn capture_terminal(&self, id: TerminalId) -> TerminalCapture {
        let mut directories: Vec<PathBuf> = Vec::new();
        if let Some(runtime) = self.terminals.get(&id) {
            let status = runtime.registry().status();
            let metadata = &status.metadata;
            let reported = [metadata.directory(), metadata.shell_directory()]
                .into_iter()
                .flatten()
                .filter(|directory| directory.is_local())
                .map(|directory| PathBuf::from(directory.path()));
            let launched = Some(runtime.launch_directory().to_path_buf());
            for directory in reported.chain(launched) {
                if !directory.as_os_str().is_empty()
                    && !directories.contains(&directory)
                {
                    directories.push(directory);
                }
            }
        }
        TerminalCapture {
            id: TerminalId::new(id.get()),
            directories,
            launch: TerminalLaunch::DefaultShell,
        }
    }

    /// Recreates a captured structure in this fresh runtime with its saved
    /// numeric identities, now in this runtime's scope, and the terminals
    /// [`SpawnedTerminals::spawn`] started for it. Sets the allocator to
    /// the capture's watermark, so new allocations never collide, and
    /// resumes each creation ordinal from the highest automatic name of
    /// its kind restored, counting records with a custom name too, so
    /// increasing numbering repeats no restored name and does not keep
    /// counting records closed before the capture. Emits the events
    /// creation emits, so subscribers stay current.
    ///
    /// A pane whose terminal failed leaves its layout, and a tab left with
    /// no pane is removed; the report names both, and each session left
    /// without a tab.
    ///
    /// # Errors
    /// Refuses, changing nothing, unless the runtime is fresh (it holds no
    /// session, workspace, or terminal and has allocated no identity or
    /// creation ordinal, so no restored identity can repeat one it handed
    /// out), the capture names this runtime's socket, the terminals were
    /// spawned in this runtime's scope for exactly this capture, and the
    /// capture validates. A refusal stops and joins every spawned terminal.
    pub fn restore(
        &mut self,
        capture: &RuntimeCapture,
        mut spawned: SpawnedTerminals,
    ) -> Result<RestoreReport, MuxError> {
        let refusal = if !self.sessions.is_empty()
            || !self.workspaces.is_empty()
            || !self.terminals.is_empty()
            || self.next_id != 0
            || self.session_ordinal != 0
            || self.workspace_ordinal != 0
        {
            Some(MuxError::RestoreNotFresh)
        } else if capture.socket_name != self.socket_name {
            Some(MuxError::RestoreForeignSocket)
        } else if spawned.runtime != self.runtime_id {
            Some(MuxError::ForeignRuntime(spawned.runtime))
        } else if let Err(error) = capture.validate() {
            Some(MuxError::InvalidCapture(error))
        } else if let Err(error) = within_limits(capture) {
            Some(error)
        } else if !spawned.covers(capture) {
            Some(MuxError::RestoreMismatch)
        } else {
            None
        };
        if let Some(error) = refusal {
            let _ = spawned.shutdown();
            return Err(error);
        }
        self.changed();
        self.next_id = capture.next_id;
        // Automatic names resume after the highest one restored, not the
        // saved ordinals, so records closed before the capture stop
        // pushing the count up across restarts.
        self.session_ordinal = resumed_ordinal(
            SESSION_NAME,
            capture
                .sessions
                .iter()
                .map(|session| session.info.automatic_name.as_str()),
        );
        self.workspace_ordinal = resumed_ordinal(
            WORKSPACE_NAME,
            capture
                .sessions
                .iter()
                .flat_map(|session| &session.workspaces)
                .map(|workspace| workspace.info.automatic_name.as_str()),
        );
        let failed: HashMap<TerminalId, RuntimeError> = spawned
            .failures
            .drain(..)
            .map(|failure| (failure.terminal, failure.error))
            .collect();
        let mut report = RestoreReport::default();
        for session in &capture.sessions {
            let id = self.restore_session(session);
            let mut holds_tab = false;
            for workspace in &session.workspaces {
                let workspace_id = self.restore_workspace(id, workspace);
                for tab in &workspace.tabs {
                    holds_tab |= self.restore_tab(
                        workspace_id,
                        tab,
                        &failed,
                        &mut spawned,
                        &mut report,
                    );
                }
            }
            if !holds_tab {
                report.empty_sessions.push(id);
            }
        }
        Ok(report)
    }

    fn restore_session(&mut self, session: &SessionCapture) -> SessionId {
        let id = SessionId::in_runtime(self.runtime_id, session.info.id.get());
        self.sessions.push(Session {
            id,
            workspaces: Vec::new(),
            custom_name: session.info.custom_name.clone(),
            automatic_name: session.info.automatic_name.clone(),
        });
        let index = position(self.sessions.len() - 1);
        self.emit_hierarchy(|mux| HierarchyEvent::SessionCreated {
            session: mux.sessions[mux.sessions.len() - 1].info(),
            index,
        });
        id
    }

    fn restore_workspace(
        &mut self,
        session: SessionId,
        workspace: &WorkspaceCapture,
    ) -> WorkspaceId {
        let id =
            WorkspaceId::in_runtime(self.runtime_id, workspace.info.id.get());
        self.workspaces.insert(
            id,
            Workspace {
                id,
                session_id: session,
                tabs: Vec::new(),
                custom_name: workspace.info.custom_name.clone(),
                automatic_name: workspace.info.automatic_name.clone(),
            },
        );
        let siblings = &mut self
            .sessions
            .last_mut()
            .expect("a restored workspace's session was just inserted")
            .workspaces;
        siblings.push(id);
        let index = position(siblings.len() - 1);
        self.emit_hierarchy(|mux| HierarchyEvent::WorkspaceCreated {
            session,
            workspace: mux.workspaces[&id].info(),
            index,
        });
        id
    }

    /// Inserts a captured tab without the panes whose terminals failed;
    /// returns whether any pane remained.
    fn restore_tab(
        &mut self,
        workspace: WorkspaceId,
        captured: &TabInfo,
        failed: &HashMap<TerminalId, RuntimeError>,
        spawned: &mut SpawnedTerminals,
        report: &mut RestoreReport,
    ) -> bool {
        let runtime = self.runtime_id;
        let id = TabId::in_runtime(runtime, captured.id.get());
        let mut layout = captured.layout.in_runtime(runtime);
        let mut zoomed = captured
            .zoomed
            .map(|pane| PaneId::in_runtime(runtime, pane.get()));
        let lost: Vec<(PaneId, TerminalId)> = layout
            .panes()
            .into_iter()
            .filter(|pane| failed.contains_key(&pane.terminal_id))
            .map(|pane| (pane.id, pane.terminal_id))
            .collect();
        for &(pane, terminal) in &lost {
            report.failed.push(FailedPane {
                terminal,
                pane,
                tab: id,
                error: failed[&terminal].clone(),
            });
        }
        if lost.len() == layout.pane_count() {
            report.removed_tabs.push(id);
            return false;
        }
        for &(pane, _) in &lost {
            layout = layout
                .without_pane(pane)
                .expect("a pane of a layout with others can leave it");
            if zoomed == Some(pane) {
                zoomed = None;
            }
        }
        if layout.pane_count() < 2 {
            zoomed = None;
        }
        let layout = with_fallback_names(&layout, |pane| {
            spawned
                .terminals
                .get(&pane.terminal_id)
                .map_or_else(String::new, |started| {
                    started.fallback_name.clone()
                })
        });
        for pane in layout.panes() {
            let started = spawned
                .terminals
                .remove(&pane.terminal_id)
                .expect("a covered capture started every surviving pane");
            self.terminals.insert(pane.terminal_id, started.runtime);
        }
        let tab = Tab {
            id,
            custom_name: captured.custom_name.clone(),
            layout,
            zoomed,
            input_sync: captured.input_sync,
        };
        let tabs = &mut self
            .workspaces
            .get_mut(&workspace)
            .expect("a restored tab's workspace was just inserted")
            .tabs;
        tabs.push(tab);
        let slot = tabs.len() - 1;
        self.emit_hierarchy(|mux| HierarchyEvent::TabOpened {
            workspace,
            tab: mux.workspaces[&workspace].tabs[slot].info(),
            index: position(slot),
        });
        true
    }
}

/// A tab's record with unscoped identities and no fallback pane names,
/// which are process labels a capture never carries.
fn unscoped_tab(tab: &Tab) -> TabInfo {
    TabInfo {
        id: TabId::new(tab.id.get()),
        layout: with_fallback_names(&tab.layout.in_runtime(UNSCOPED), |_| {
            String::new()
        }),
        zoomed: tab.zoomed.map(|pane| PaneId::new(pane.get())),
        ..tab.info()
    }
}

/// The layout with each pane's fallback name replaced by `name`'s.
fn with_fallback_names(
    layout: &PaneLayout,
    name: impl Fn(&PaneInfo) -> String,
) -> PaneLayout {
    fn rename(node: &mut PaneNode, name: &impl Fn(&PaneInfo) -> String) {
        match node {
            PaneNode::Pane(pane) => pane.fallback_name = name(pane),
            PaneNode::Split(split) => {
                for child in &mut split.children {
                    rename(&mut child.node, name);
                }
            }
        }
    }
    let mut root = layout.root().clone();
    rename(&mut root, &name);
    PaneLayout::new(root).expect("renaming panes keeps a layout valid")
}

#[cfg(test)]
mod tests;
