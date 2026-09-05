use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{RuntimeClient, RuntimeError, TerminalRuntime};
use huterm_protocol::{
    PaneId, RuntimeId, SessionId, TabId, TerminalCommand, TerminalId,
    WorkspaceId,
};
use thiserror::Error;

/// Logical default socket name. No endpoint is opened by the runtime.
pub const DEFAULT_SOCKET_NAME: &str = "default";
static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(1);

/// One tab's canonical single-pane layout. Splits can replace this leaf later.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tab {
    /// Stable tab identity.
    pub id: TabId,
    /// Stable pane identity.
    pub pane_id: PaneId,
    /// Terminal displayed by the pane.
    pub terminal_id: TerminalId,
    custom_name: Option<String>,
    fallback_name: String,
}
impl Tab {
    /// Returns the custom override, if set.
    #[must_use]
    pub fn custom_name(&self) -> Option<&str> {
        self.custom_name.as_deref()
    }
    /// Resolves the override, current terminal title, then launched program name.
    #[must_use]
    pub fn display_name<'a>(
        &'a self,
        current_terminal_title: &'a str,
    ) -> &'a str {
        self.custom_name.as_deref().unwrap_or_else(|| {
            if current_terminal_title.trim().is_empty() {
                &self.fallback_name
            } else {
                current_terminal_title
            }
        })
    }
}

/// Canonical workspace structure, independent of client navigation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Workspace {
    /// Stable workspace identity.
    pub id: WorkspaceId,
    /// Current owning session.
    pub session_id: SessionId,
    /// Tabs in presentation order.
    pub tabs: Vec<Tab>,
    custom_name: Option<String>,
    automatic_name: String,
}
/// Canonical session and its ordered workspace membership.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Session {
    /// Stable session identity.
    pub id: SessionId,
    /// Workspace identities in presentation order.
    pub workspaces: Vec<WorkspaceId>,
    custom_name: Option<String>,
    automatic_name: String,
}
macro_rules! named_record {
    ($kind:ty) => {
        impl $kind {
            /// Returns the custom override, if set.
            #[must_use]
            pub fn custom_name(&self) -> Option<&str> {
                self.custom_name.as_deref()
            }
            /// Returns the custom override or immutable creation-ordinal name.
            #[must_use]
            pub fn display_name(&self) -> &str {
                self.custom_name.as_deref().unwrap_or(&self.automatic_name)
            }
        }
    };
}
named_record!(Session);
named_record!(Workspace);

/// A validated navigation target with its current ownership ancestry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectionTarget {
    /// Owning session.
    pub session: SessionId,
    /// Selected workspace, if any.
    pub workspace: Option<WorkspaceId>,
    /// Selected tab, if any.
    pub tab: Option<TabId>,
}
/// A successfully published tab and its in-process terminal attachment.
#[derive(Debug)]
pub struct OpenedTab {
    /// Canonical tab record.
    pub tab: Tab,
    /// Terminal attachment. Dropping it does not close the terminal.
    pub client: RuntimeClient,
}

/// Canonical session, workspace, and terminal owner.
///
/// Structural operations require exclusive access. Spawn and close block while
/// creating or joining workers; clients must call them on a worker thread.
/// Runtime scopes are unique within this process; a future wire protocol must
/// assign incarnation identity across processes and restarts as well.
#[derive(Debug)]
pub struct Mux {
    runtime_id: RuntimeId,
    socket_name: String,
    next_id: u64,
    session_ordinal: u64,
    workspace_ordinal: u64,
    sessions: Vec<Session>,
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    terminals: BTreeMap<TerminalId, TerminalRuntime>,
}
impl Default for Mux {
    fn default() -> Self {
        Self::new(DEFAULT_SOCKET_NAME)
            .expect("runtime identity space exhausted")
    }
}
impl Mux {
    /// Creates an empty runtime with a logical socket name, without opening IPC.
    ///
    /// # Errors
    /// Rejects blank names or exhausted process-local runtime identities.
    pub fn new(socket_name: &str) -> Result<Self, MuxError> {
        validate_name(Some(socket_name))?;
        let runtime_id = NEXT_RUNTIME
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                n.checked_add(1)
            })
            .map_err(|_| MuxError::IdExhausted)?;
        Ok(Self {
            runtime_id: RuntimeId::new(runtime_id),
            socket_name: socket_name.into(),
            next_id: 0,
            session_ordinal: 0,
            workspace_ordinal: 0,
            sessions: Vec::new(),
            workspaces: BTreeMap::new(),
            terminals: BTreeMap::new(),
        })
    }
    /// Returns the runtime incarnation used to validate structural targets.
    #[must_use]
    pub fn runtime_id(&self) -> RuntimeId {
        self.runtime_id
    }
    /// Returns the logical socket name, which is not an identity or endpoint.
    #[must_use]
    pub fn socket_name(&self) -> &str {
        &self.socket_name
    }
    /// Advances allocation past a saved identity before restoring records.
    pub fn reserve_through(&mut self, id: u64) {
        self.next_id = self.next_id.max(id);
    }
    fn allocate(&mut self) -> Result<u64, MuxError> {
        self.next_id =
            self.next_id.checked_add(1).ok_or(MuxError::IdExhausted)?;
        Ok(self.next_id)
    }
    fn validate_scope(&self, runtime: RuntimeId) -> Result<(), MuxError> {
        if runtime == self.runtime_id {
            Ok(())
        } else {
            Err(MuxError::ForeignRuntime(runtime))
        }
    }
    /// Lists sessions in creation order.
    #[must_use]
    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }
    /// Looks up a session by scoped identity.
    #[must_use]
    pub fn session(&self, id: SessionId) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }
    /// Looks up a workspace by scoped identity.
    #[must_use]
    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.get(&id)
    }
    /// Looks up a tab by scoped identity, independent of current membership.
    #[must_use]
    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.workspaces
            .values()
            .flat_map(|w| &w.tabs)
            .find(|t| t.id == id)
    }
    /// Validates a session selection without changing client navigation state.
    /// # Errors
    /// Rejects foreign or missing identities.
    pub fn select_session(
        &self,
        id: SessionId,
    ) -> Result<SelectionTarget, MuxError> {
        self.validate_scope(id.runtime())?;
        self.session(id).ok_or(MuxError::UnknownSession(id))?;
        Ok(SelectionTarget {
            session: id,
            workspace: None,
            tab: None,
        })
    }
    /// Resolves a workspace selection to its current session.
    /// # Errors
    /// Rejects foreign or missing identities.
    pub fn select_workspace(
        &self,
        id: WorkspaceId,
    ) -> Result<SelectionTarget, MuxError> {
        self.validate_scope(id.runtime())?;
        let workspace =
            self.workspace(id).ok_or(MuxError::UnknownWorkspace(id))?;
        Ok(SelectionTarget {
            session: workspace.session_id,
            workspace: Some(id),
            tab: None,
        })
    }
    /// Resolves a tab selection to its current workspace and session.
    /// # Errors
    /// Rejects foreign or missing identities.
    pub fn select_tab(&self, id: TabId) -> Result<SelectionTarget, MuxError> {
        self.validate_scope(id.runtime())?;
        let workspace = self
            .workspaces
            .values()
            .find(|w| w.tabs.iter().any(|t| t.id == id))
            .ok_or(MuxError::UnknownTab(id))?;
        Ok(SelectionTarget {
            session: workspace.session_id,
            workspace: Some(workspace.id),
            tab: Some(id),
        })
    }
    /// Creates an empty session with an optional custom name.
    /// # Errors
    /// Rejects blank names or exhausted identities/creation ordinals.
    pub fn create_session(
        &mut self,
        name: Option<&str>,
    ) -> Result<SessionId, MuxError> {
        validate_name(name)?;
        let ordinal = self
            .session_ordinal
            .checked_add(1)
            .ok_or(MuxError::IdExhausted)?;
        let value = self.allocate()?;
        let id = SessionId::in_runtime(self.runtime_id, value);
        self.sessions.push(Session {
            id,
            workspaces: Vec::new(),
            custom_name: name.map(str::to_owned),
            automatic_name: format!("Session {ordinal}"),
        });
        self.session_ordinal = ordinal;
        Ok(id)
    }
    /// Appends an empty workspace to a session.
    /// # Errors
    /// Rejects invalid names, foreign/missing sessions, or exhausted identities.
    pub fn create_workspace(
        &mut self,
        session: SessionId,
        name: Option<&str>,
    ) -> Result<WorkspaceId, MuxError> {
        self.select_session(session)?;
        validate_name(name)?;
        let ordinal = self
            .workspace_ordinal
            .checked_add(1)
            .ok_or(MuxError::IdExhausted)?;
        let value = self.allocate()?;
        let id = WorkspaceId::in_runtime(self.runtime_id, value);
        self.workspaces.insert(
            id,
            Workspace {
                id,
                session_id: session,
                tabs: Vec::new(),
                custom_name: name.map(str::to_owned),
                automatic_name: format!("Workspace {ordinal}"),
            },
        );
        self.sessions
            .iter_mut()
            .find(|s| s.id == session)
            .ok_or(MuxError::UnknownSession(session))?
            .workspaces
            .push(id);
        self.workspace_ordinal = ordinal;
        Ok(id)
    }
    /// Sets a session's custom name; `None` resumes automatic naming.
    /// # Errors
    /// Rejects blank names and foreign/missing targets.
    pub fn rename_session(
        &mut self,
        id: SessionId,
        name: Option<&str>,
    ) -> Result<(), MuxError> {
        self.select_session(id)?;
        validate_name(name)?;
        self.sessions
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(MuxError::UnknownSession(id))?
            .custom_name = name.map(str::to_owned);
        Ok(())
    }
    /// Sets a workspace's custom name; `None` resumes automatic naming.
    /// # Errors
    /// Rejects blank names and foreign/missing targets.
    pub fn rename_workspace(
        &mut self,
        id: WorkspaceId,
        name: Option<&str>,
    ) -> Result<(), MuxError> {
        self.select_workspace(id)?;
        validate_name(name)?;
        self.workspaces
            .get_mut(&id)
            .ok_or(MuxError::UnknownWorkspace(id))?
            .custom_name = name.map(str::to_owned);
        Ok(())
    }
    /// Sets a tab's custom name; `None` resumes the current terminal title.
    /// # Errors
    /// Rejects blank names and foreign/missing targets.
    pub fn rename_tab(
        &mut self,
        id: TabId,
        name: Option<&str>,
    ) -> Result<(), MuxError> {
        self.select_tab(id)?;
        validate_name(name)?;
        self.workspaces
            .values_mut()
            .flat_map(|w| &mut w.tabs)
            .find(|t| t.id == id)
            .ok_or(MuxError::UnknownTab(id))?
            .custom_name = name.map(str::to_owned);
        Ok(())
    }
    /// Creates and appends a tab only after PTY creation succeeds.
    /// # Errors
    /// Rejects foreign/missing workspaces, exhausted IDs, or failed spawns.
    pub fn open_tab(
        &mut self,
        workspace: WorkspaceId,
        command: &TerminalCommand,
    ) -> Result<OpenedTab, MuxError> {
        self.select_workspace(workspace)?;
        let value = self.allocate()?;
        let tab = Tab {
            id: TabId::in_runtime(self.runtime_id, value),
            pane_id: PaneId::new(self.allocate()?),
            terminal_id: TerminalId::new(self.allocate()?),
            custom_name: None,
            fallback_name: command
                .program
                .file_name()
                .unwrap_or(command.program.as_os_str())
                .to_string_lossy()
                .into_owned(),
        };
        let runtime = TerminalRuntime::spawn(tab.terminal_id, command)?;
        let client = runtime.client();
        self.workspaces
            .get_mut(&workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?
            .tabs
            .push(tab.clone());
        self.terminals.insert(tab.terminal_id, runtime);
        Ok(OpenedTab { tab, client })
    }
    /// Reorders a tab within its current workspace, before an anchor or at end.
    /// # Errors
    /// Rejects foreign, missing, or nonmember targets without changing structure.
    pub fn reorder_tab(
        &mut self,
        workspace: WorkspaceId,
        tab: TabId,
        before: Option<TabId>,
    ) -> Result<(), MuxError> {
        self.move_tab(workspace, tab, workspace, before)
    }
    /// Atomically transfers a tab before a destination anchor, or to its end.
    /// Processes, IDs, and attachments survive. Empty workspaces remain owned.
    /// # Errors
    /// Rejects foreign, missing, or nonmember targets before changing structure.
    pub fn move_tab(
        &mut self,
        source: WorkspaceId,
        tab: TabId,
        destination: WorkspaceId,
        before: Option<TabId>,
    ) -> Result<(), MuxError> {
        self.select_workspace(source)?;
        self.select_workspace(destination)?;
        self.validate_scope(tab.runtime())?;
        if let Some(anchor) = before {
            self.validate_scope(anchor.runtime())?;
        }
        let source_index = self.workspaces[&source]
            .tabs
            .iter()
            .position(|t| t.id == tab)
            .ok_or(MuxError::UnknownTab(tab))?;
        let target_index = before.map_or(
            Ok(self.workspaces[&destination].tabs.len()),
            |anchor| {
                self.workspaces[&destination]
                    .tabs
                    .iter()
                    .position(|t| t.id == anchor)
                    .ok_or(MuxError::UnknownTab(anchor))
            },
        )?;
        if source == destination && source_index == target_index {
            return Ok(());
        }
        let record = self
            .workspaces
            .get_mut(&source)
            .ok_or(MuxError::UnknownWorkspace(source))?
            .tabs
            .remove(source_index);
        let target_index = target_index
            - usize::from(source == destination && source_index < target_index);
        self.workspaces
            .get_mut(&destination)
            .ok_or(MuxError::UnknownWorkspace(destination))?
            .tabs
            .insert(target_index, record);
        Ok(())
    }
    /// Atomically transfers a workspace before an anchor or to a session's end.
    /// Empty sessions remain owned. All tabs and terminal attachments survive.
    /// # Errors
    /// Rejects foreign, missing, or nonmember targets before changing structure.
    pub fn move_workspace(
        &mut self,
        source: SessionId,
        workspace: WorkspaceId,
        destination: SessionId,
        before: Option<WorkspaceId>,
    ) -> Result<(), MuxError> {
        self.select_session(source)?;
        self.select_session(destination)?;
        self.validate_scope(workspace.runtime())?;
        if let Some(anchor) = before {
            self.validate_scope(anchor.runtime())?;
        }
        let source_session = self
            .sessions
            .iter()
            .position(|s| s.id == source)
            .ok_or(MuxError::UnknownSession(source))?;
        let target_session = self
            .sessions
            .iter()
            .position(|s| s.id == destination)
            .ok_or(MuxError::UnknownSession(destination))?;
        let source_index = self.sessions[source_session]
            .workspaces
            .iter()
            .position(|w| *w == workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?;
        let target_index = before.map_or(
            Ok(self.sessions[target_session].workspaces.len()),
            |anchor| {
                self.sessions[target_session]
                    .workspaces
                    .iter()
                    .position(|w| *w == anchor)
                    .ok_or(MuxError::UnknownWorkspace(anchor))
            },
        )?;
        if source == destination && source_index == target_index {
            return Ok(());
        }
        self.sessions[source_session]
            .workspaces
            .remove(source_index);
        let target_index = target_index
            - usize::from(source == destination && source_index < target_index);
        self.sessions[target_session]
            .workspaces
            .insert(target_index, workspace);
        self.workspaces
            .get_mut(&workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?
            .session_id = destination;
        Ok(())
    }
    /// Attaches to a terminal without transferring runtime ownership.
    /// Events currently have one consumer; simultaneous clients need broadcast.
    #[must_use]
    pub fn attach(&self, terminal_id: TerminalId) -> Option<RuntimeClient> {
        self.terminals
            .get(&terminal_id)
            .map(TerminalRuntime::client)
    }
    /// Closes a tab and joins its terminal workers, leaving siblings intact.
    /// # Errors
    /// Rejects foreign/missing members or reports failed terminal cleanup.
    pub fn close_tab(
        &mut self,
        workspace: WorkspaceId,
        tab: TabId,
    ) -> Result<(), MuxError> {
        self.select_workspace(workspace)?;
        self.validate_scope(tab.runtime())?;
        let record = self
            .workspaces
            .get_mut(&workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?;
        let index = record
            .tabs
            .iter()
            .position(|t| t.id == tab)
            .ok_or(MuxError::UnknownTab(tab))?;
        let tab = record.tabs.remove(index);
        if let Some(runtime) = self.terminals.remove(&tab.terminal_id) {
            runtime.shutdown()?;
        }
        Ok(())
    }
    /// Deletes a workspace and stops all its terminals, retaining its session.
    /// # Errors
    /// Rejects foreign/missing targets. Attempts every cleanup even if one fails.
    pub fn close_workspace(
        &mut self,
        workspace: WorkspaceId,
    ) -> Result<(), MuxError> {
        self.select_workspace(workspace)?;
        let record = self
            .workspaces
            .remove(&workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?;
        for session in &mut self.sessions {
            session.workspaces.retain(|id| *id != workspace);
        }
        self.close_terminals(
            record.tabs.into_iter().map(|t| t.terminal_id).collect(),
        )
    }
    /// Deletes a session and stops every terminal in its workspaces.
    /// # Errors
    /// Rejects foreign/missing targets. Attempts every cleanup even if one fails.
    pub fn close_session(
        &mut self,
        session: SessionId,
    ) -> Result<(), MuxError> {
        self.select_session(session)?;
        let index = self
            .sessions
            .iter()
            .position(|s| s.id == session)
            .ok_or(MuxError::UnknownSession(session))?;
        let record = self.sessions.remove(index);
        let terminals = record
            .workspaces
            .into_iter()
            .filter_map(|id| self.workspaces.remove(&id))
            .flat_map(|w| w.tabs)
            .map(|t| t.terminal_id)
            .collect();
        self.close_terminals(terminals)
    }
    fn close_terminals(
        &mut self,
        terminals: Vec<TerminalId>,
    ) -> Result<(), MuxError> {
        for id in &terminals {
            if let Some(runtime) = self.terminals.get(id) {
                let _ = runtime.client().close();
            }
        }
        let mut failure = None;
        for id in terminals {
            if let Some(runtime) = self.terminals.remove(&id)
                && let Err(error) = runtime.shutdown()
            {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), |error| Err(error.into()))
    }
    /// Stops and joins every terminal, including unattached sessions.
    /// # Errors
    /// Returns the last failed cleanup after attempting every terminal.
    pub fn shutdown(&mut self) -> Result<(), MuxError> {
        self.sessions.clear();
        self.workspaces.clear();
        self.close_terminals(self.terminals.keys().copied().collect())
    }
    /// Returns the number of owned terminals, including exited shells.
    #[must_use]
    pub fn terminal_count(&self) -> usize {
        self.terminals.len()
    }
}
fn validate_name(name: Option<&str>) -> Result<(), MuxError> {
    if name.is_some_and(|name| name.trim().is_empty()) {
        Err(MuxError::InvalidName)
    } else {
        Ok(())
    }
}
/// Structural runtime operation failure.
#[derive(Debug, Error)]
pub enum MuxError {
    /// No more stable identities or creation ordinals can be allocated.
    #[error("runtime identity space exhausted")]
    IdExhausted,
    /// A custom or socket name contained no visible characters.
    #[error("name must not be blank; use None to clear an override")]
    InvalidName,
    /// A target belongs to a different runtime incarnation.
    #[error("target belongs to foreign runtime {0:?}")]
    ForeignRuntime(RuntimeId),
    /// Session no longer exists.
    #[error("session {0:?} does not exist")]
    UnknownSession(SessionId),
    /// Workspace no longer exists in the requested session.
    #[error("workspace {0:?} does not exist in this session")]
    UnknownWorkspace(WorkspaceId),
    /// Tab no longer belongs to the requested workspace.
    #[error("tab {0:?} does not exist in this workspace")]
    UnknownTab(TabId),
    /// Terminal startup or shutdown failed.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}
#[cfg(test)]
mod tests {
    use super::*;
    use huterm_protocol::{CellSize, GridSize, TerminalEvent, TerminalInput};
    use std::time::{Duration, Instant};

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "exercise generated names, overrides, and reset on one shared ownership tree"
    )]
    fn names_reset_to_stable_ordinals_and_current_title_with_duplicate_id_targeting()
     {
        let mut mux = Mux::default();
        assert_eq!(mux.socket_name(), DEFAULT_SOCKET_NAME);
        let first_session = mux.create_session(None).unwrap();
        let second_session = mux.create_session(Some("duplicate")).unwrap();
        let first_workspace =
            mux.create_workspace(first_session, None).unwrap();
        let second_workspace = mux
            .create_workspace(second_session, Some("duplicate"))
            .unwrap();
        let tab = mux
            .open_tab(first_workspace, &command("read value"))
            .unwrap()
            .tab;
        assert_eq!(
            mux.session(first_session).unwrap().display_name(),
            "Session 1"
        );
        assert_eq!(
            mux.workspace(first_workspace).unwrap().display_name(),
            "Workspace 1"
        );
        mux.rename_session(first_session, Some("duplicate"))
            .unwrap();
        mux.rename_workspace(first_workspace, Some("duplicate"))
            .unwrap();
        mux.rename_tab(tab.id, Some("custom")).unwrap();
        assert_eq!(
            mux.tab(tab.id).unwrap().display_name("first title"),
            "custom"
        );
        assert_eq!(
            mux.tab(tab.id).unwrap().display_name("changed title"),
            "custom"
        );
        mux.rename_tab(tab.id, None).unwrap();
        assert_eq!(
            mux.tab(tab.id).unwrap().display_name("changed title"),
            "changed title"
        );
        assert_eq!(mux.tab(tab.id).unwrap().display_name(" \t"), "sh");
        mux.rename_session(first_session, None).unwrap();
        mux.rename_workspace(first_workspace, None).unwrap();
        assert_eq!(
            mux.session(first_session).unwrap().display_name(),
            "Session 1"
        );
        assert_eq!(
            mux.session(second_session).unwrap().display_name(),
            "duplicate"
        );
        assert_eq!(
            mux.workspace(first_workspace).unwrap().display_name(),
            "Workspace 1"
        );
        assert_eq!(
            mux.workspace(second_workspace).unwrap().display_name(),
            "duplicate"
        );
        assert_eq!(
            mux.select_tab(tab.id).unwrap(),
            SelectionTarget {
                session: first_session,
                workspace: Some(first_workspace),
                tab: Some(tab.id)
            }
        );
        assert_eq!(
            mux.sessions().iter().map(|s| s.id).collect::<Vec<_>>(),
            [first_session, second_session]
        );
        let original = mux.sessions().to_vec();
        assert!(matches!(
            mux.create_session(Some(" \t")),
            Err(MuxError::InvalidName)
        ));
        assert!(matches!(
            mux.create_workspace(first_session, Some("")),
            Err(MuxError::InvalidName)
        ));
        assert!(matches!(
            mux.rename_session(first_session, Some("")),
            Err(MuxError::InvalidName)
        ));
        assert!(matches!(
            mux.rename_workspace(first_workspace, Some("")),
            Err(MuxError::InvalidName)
        ));
        assert!(matches!(
            mux.rename_tab(tab.id, Some("")),
            Err(MuxError::InvalidName)
        ));
        assert_eq!(mux.sessions(), original);
        mux.close_session(first_session).unwrap();
        mux.rename_session(second_session, None).unwrap();
        mux.rename_workspace(second_workspace, None).unwrap();
        assert_eq!(
            mux.session(second_session).unwrap().display_name(),
            "Session 2"
        );
        assert_eq!(
            mux.workspace(second_workspace).unwrap().display_name(),
            "Workspace 2"
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "retain live PTY attachments through transfers, reorders, and empty-parent cleanup"
    )]
    fn cross_parent_moves_preserve_live_pty_identity_and_update_selection_ancestry()
     {
        let mut mux = Mux::default();
        let first_session = mux.create_session(None).unwrap();
        let second_session = mux.create_session(None).unwrap();
        let first_workspace =
            mux.create_workspace(first_session, None).unwrap();
        let second_workspace =
            mux.create_workspace(first_session, None).unwrap();
        let third_workspace =
            mux.create_workspace(second_session, None).unwrap();
        let opened = mux.open_tab(first_workspace, &command("printf READY; read value; printf 'MOVED_%s' \"$value\"; read value")).unwrap();
        let anchor = mux
            .open_tab(second_workspace, &command("printf SIBLING; read value"))
            .unwrap();
        wait_for_text(&opened.client, "READY");
        mux.move_tab(
            first_workspace,
            opened.tab.id,
            second_workspace,
            Some(anchor.tab.id),
        )
        .unwrap();
        assert!(mux.workspace(first_workspace).unwrap().tabs.is_empty());
        assert_eq!(
            mux.workspace(second_workspace).unwrap().tabs,
            [opened.tab.clone(), anchor.tab.clone()]
        );
        mux.move_workspace(
            first_session,
            second_workspace,
            second_session,
            Some(third_workspace),
        )
        .unwrap();
        assert_eq!(
            mux.session(first_session).unwrap().workspaces,
            [first_workspace]
        );
        assert_eq!(
            mux.session(second_session).unwrap().workspaces,
            [second_workspace, third_workspace]
        );
        assert_eq!(
            mux.select_tab(opened.tab.id).unwrap(),
            SelectionTarget {
                session: second_session,
                workspace: Some(second_workspace),
                tab: Some(opened.tab.id)
            }
        );
        mux.move_workspace(
            second_session,
            second_workspace,
            second_session,
            None,
        )
        .unwrap();
        assert_eq!(
            mux.session(second_session).unwrap().workspaces,
            [third_workspace, second_workspace]
        );
        mux.move_workspace(
            second_session,
            second_workspace,
            second_session,
            Some(second_workspace),
        )
        .unwrap();
        assert_eq!(
            mux.session(second_session).unwrap().workspaces,
            [third_workspace, second_workspace]
        );
        mux.move_workspace(
            second_session,
            second_workspace,
            second_session,
            Some(third_workspace),
        )
        .unwrap();
        assert_eq!(
            mux.workspace(second_workspace).unwrap().display_name(),
            "Workspace 2"
        );
        assert_eq!(mux.tab(opened.tab.id).unwrap(), &opened.tab);
        opened
            .client
            .send_input(TerminalInput::Text("alive\n".into()))
            .unwrap();
        wait_for_text(
            &mux.attach(opened.tab.terminal_id).unwrap(),
            "MOVED_alive",
        );
        wait_for_text(&anchor.client, "SIBLING");
        mux.move_workspace(
            first_session,
            first_workspace,
            second_session,
            None,
        )
        .unwrap();
        assert!(mux.session(first_session).unwrap().workspaces.is_empty());
        mux.close_session(first_session).unwrap();
        assert_eq!(mux.terminal_count(), 2);
        mux.shutdown().unwrap();
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "compare one ownership snapshot after every invalid source, destination, and anchor"
    )]
    fn stale_and_foreign_structural_targets_reject_atomically() {
        let mut mux = Mux::default();
        let first_session = mux.create_session(None).unwrap();
        let second_session = mux.create_session(None).unwrap();
        let first_workspace =
            mux.create_workspace(first_session, None).unwrap();
        let second_workspace =
            mux.create_workspace(second_session, None).unwrap();
        let tab = mux
            .open_tab(first_workspace, &command("read value"))
            .unwrap()
            .tab;
        let other_tab = mux
            .open_tab(second_workspace, &command("read value"))
            .unwrap()
            .tab;
        let mut foreign = Mux::default();
        let foreign_session = foreign.create_session(None).unwrap();
        foreign.create_session(None).unwrap();
        let foreign_workspace =
            foreign.create_workspace(foreign_session, None).unwrap();
        foreign.create_workspace(foreign_session, None).unwrap();
        let foreign_tab = foreign
            .open_tab(foreign_workspace, &command("read value"))
            .unwrap()
            .tab;
        assert_eq!(mux.socket_name(), foreign.socket_name());
        assert_eq!(first_session.get(), foreign_session.get());
        assert_eq!(first_workspace.get(), foreign_workspace.get());
        assert_eq!(tab.id.get(), foreign_tab.id.get());
        let stale_session = SessionId::in_runtime(mux.runtime_id(), u64::MAX);
        let stale_workspace =
            WorkspaceId::in_runtime(mux.runtime_id(), u64::MAX);
        let stale_tab = TabId::in_runtime(mux.runtime_id(), u64::MAX);
        let original_sessions = mux.sessions().to_vec();
        let original_workspaces = mux.workspaces.clone();
        for (source, target, destination, anchor) in [
            (first_workspace, tab.id, second_workspace, Some(stale_tab)),
            (first_workspace, tab.id, second_workspace, Some(tab.id)),
            (second_workspace, tab.id, first_workspace, None),
            (first_workspace, stale_tab, second_workspace, None),
            (stale_workspace, tab.id, second_workspace, None),
            (first_workspace, tab.id, stale_workspace, None),
            (foreign_workspace, tab.id, second_workspace, None),
            (first_workspace, foreign_tab.id, second_workspace, None),
            (first_workspace, tab.id, foreign_workspace, None),
            (
                first_workspace,
                tab.id,
                second_workspace,
                Some(foreign_tab.id),
            ),
        ] {
            assert!(mux.move_tab(source, target, destination, anchor).is_err());
            assert_eq!(mux.workspaces, original_workspaces);
        }
        for (source, target, destination, anchor) in [
            (
                first_session,
                first_workspace,
                second_session,
                Some(stale_workspace),
            ),
            (
                first_session,
                first_workspace,
                second_session,
                Some(first_workspace),
            ),
            (second_session, first_workspace, first_session, None),
            (first_session, stale_workspace, second_session, None),
            (stale_session, first_workspace, second_session, None),
            (first_session, first_workspace, stale_session, None),
            (foreign_session, first_workspace, second_session, None),
            (first_session, foreign_workspace, second_session, None),
            (first_session, first_workspace, foreign_session, None),
            (
                first_session,
                first_workspace,
                second_session,
                Some(foreign_workspace),
            ),
        ] {
            assert!(
                mux.move_workspace(source, target, destination, anchor)
                    .is_err()
            );
            assert_eq!(mux.sessions(), original_sessions);
            assert_eq!(mux.workspaces, original_workspaces);
        }
        for invalid in [
            foreign_session,
            stale_session,
            SessionId::new(first_session.get()),
        ] {
            assert!(mux.create_workspace(invalid, None).is_err());
            assert!(mux.rename_session(invalid, Some("wrong")).is_err());
            assert!(mux.close_session(invalid).is_err());
            assert!(mux.select_session(invalid).is_err());
        }
        for invalid in [
            foreign_workspace,
            stale_workspace,
            WorkspaceId::new(first_workspace.get()),
        ] {
            assert!(mux.open_tab(invalid, &command("read value")).is_err());
            assert!(mux.rename_workspace(invalid, Some("wrong")).is_err());
            assert!(mux.close_workspace(invalid).is_err());
            assert!(mux.select_workspace(invalid).is_err());
        }
        for invalid in [foreign_tab.id, stale_tab, TabId::new(tab.id.get())] {
            assert!(mux.rename_tab(invalid, Some("wrong")).is_err());
            assert!(mux.close_tab(first_workspace, invalid).is_err());
            assert!(mux.select_tab(invalid).is_err());
        }
        assert!(mux.close_tab(first_workspace, other_tab.id).is_err());
        assert_eq!(mux.sessions(), original_sessions);
        assert_eq!(mux.workspaces, original_workspaces);
        assert_eq!(mux.terminal_count(), 2);
        mux.shutdown().unwrap();
        foreign.shutdown().unwrap();
    }

    #[test]
    fn session_close_stops_its_workers_and_preserves_sibling_input() {
        let mut mux = Mux::default();
        let first_session = mux.create_session(None).unwrap();
        let second_session = mux.create_session(None).unwrap();
        let first_workspace =
            mux.create_workspace(first_session, None).unwrap();
        let second_workspace =
            mux.create_workspace(first_session, None).unwrap();
        let third_workspace =
            mux.create_workspace(second_session, None).unwrap();
        let first = mux
            .open_tab(first_workspace, &command("printf READY; read value"))
            .unwrap();
        let second = mux
            .open_tab(second_workspace, &command("printf READY; read value"))
            .unwrap();
        let sibling = mux.open_tab(third_workspace, &command("printf READY; read value; printf 'SIBLING_%s' \"$value\"; read value")).unwrap();
        wait_for_text(&first.client, "READY");
        wait_for_text(&second.client, "READY");
        wait_for_text(&sibling.client, "READY");
        mux.close_session(first_session).unwrap();
        for client in [&first.client, &second.client] {
            assert!(matches!(
                client.read_snapshot(),
                Err(RuntimeError::Stopped)
            ));
        }
        assert!(mux.session(first_session).is_none());
        assert!(mux.workspace(first_workspace).is_none());
        assert!(mux.workspace(second_workspace).is_none());
        assert_eq!(mux.sessions().len(), 1);
        assert_eq!(mux.terminal_count(), 1);
        sibling
            .client
            .send_input(TerminalInput::Text("alive\n".into()))
            .unwrap();
        wait_for_text(&sibling.client, "SIBLING_alive");
        mux.shutdown().unwrap();
    }

    #[test]
    fn exhausted_creation_does_not_publish_partial_structure() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        mux.reserve_through(u64::MAX - 2);
        assert!(matches!(
            mux.open_tab(workspace, &command("read value")),
            Err(MuxError::IdExhausted)
        ));
        assert!(mux.workspace(workspace).unwrap().tabs.is_empty());
        assert_eq!(mux.terminal_count(), 0);
        assert!(matches!(
            mux.create_workspace(session, None),
            Err(MuxError::IdExhausted)
        ));
        assert!(matches!(
            mux.create_session(None),
            Err(MuxError::IdExhausted)
        ));
        assert_eq!(mux.sessions().len(), 1);
        assert_eq!(mux.session(session).unwrap().workspaces, [workspace]);
        mux.session_ordinal = u64::MAX;
        mux.workspace_ordinal = u64::MAX;
        mux.next_id = 5;
        assert!(matches!(
            mux.create_session(None),
            Err(MuxError::IdExhausted)
        ));
        assert!(matches!(
            mux.create_workspace(session, None),
            Err(MuxError::IdExhausted)
        ));
        assert_eq!(mux.next_id, 5);
    }
    #[test]
    fn reorder_preserves_live_terminals_and_rejects_stale_or_foreign_anchors() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let sibling = mux.create_workspace(session, None).unwrap();
        let first = mux.open_tab(workspace, &command("printf FIRST; read value; printf 'GOT_%s' \"$value\"; read value")).unwrap();
        let second = mux
            .open_tab(workspace, &command("printf SECOND; read value"))
            .unwrap();
        let third = mux
            .open_tab(workspace, &command("printf THIRD; read value"))
            .unwrap();
        let foreign = mux
            .open_tab(sibling, &command("printf FOREIGN; read value"))
            .unwrap();
        wait_for_text(&first.client, "FIRST");
        let original = mux.workspace(workspace).unwrap().tabs.clone();
        mux.reorder_tab(workspace, third.tab.id, Some(first.tab.id))
            .unwrap();
        assert_eq!(
            mux.workspace(workspace).unwrap().tabs,
            [third.tab.clone(), first.tab.clone(), second.tab.clone()]
        );
        mux.reorder_tab(workspace, third.tab.id, None).unwrap();
        assert_eq!(mux.workspace(workspace).unwrap().tabs, original);
        for anchor in [Some(first.tab.id), Some(second.tab.id)] {
            mux.reorder_tab(workspace, first.tab.id, anchor).unwrap();
            assert_eq!(mux.workspace(workspace).unwrap().tabs, original);
        }
        for (tab, anchor) in [
            (
                first.tab.id,
                Some(TabId::in_runtime(mux.runtime_id(), u64::MAX)),
            ),
            (first.tab.id, Some(foreign.tab.id)),
            (foreign.tab.id, None),
        ] {
            assert!(matches!(
                mux.reorder_tab(workspace, tab, anchor),
                Err(MuxError::UnknownTab(_))
            ));
            assert_eq!(mux.workspace(workspace).unwrap().tabs, original);
        }
        assert!(matches!(
            mux.reorder_tab(
                WorkspaceId::in_runtime(mux.runtime_id(), u64::MAX),
                first.tab.id,
                None
            ),
            Err(MuxError::UnknownWorkspace(_))
        ));
        first
            .client
            .send_input(TerminalInput::Text("alive\n".into()))
            .unwrap();
        wait_for_text(&first.client, "GOT_alive");
        wait_for_text(&second.client, "SECOND");
        assert_eq!(mux.terminal_count(), 4);
        assert_eq!(mux.workspace(sibling).unwrap().tabs, [foreign.tab]);
        mux.shutdown().unwrap();
    }

    fn command(script: &str) -> TerminalCommand {
        TerminalCommand {
            engine: huterm_protocol::TerminalEngineKind::Alacritty,
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), script.into()],
            working_directory: std::env::current_dir().unwrap(),
            environment: Vec::new(),
            grid_size: GridSize::clamped(40, 8),
            cell_size: CellSize {
                width: 8,
                height: 16,
            },
        }
    }
    fn wait_for_text(client: &RuntimeClient, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let snapshot = client.read_snapshot().unwrap();
            let text: String =
                snapshot.cells().map(|cell| cell.text.as_str()).collect();
            if text.contains(needle) {
                return;
            }
            assert!(Instant::now() < deadline, "missing {needle}: {text}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn allocation_preserves_reserved_identity_and_rejects_exhaustion() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        mux.reserve_through(90);
        assert_eq!(
            mux.create_workspace(session, None).unwrap(),
            WorkspaceId::in_runtime(mux.runtime_id(), 91)
        );
        mux.reserve_through(1);
        assert_eq!(
            mux.create_workspace(session, None).unwrap(),
            WorkspaceId::in_runtime(mux.runtime_id(), 92)
        );
        mux.reserve_through(u64::MAX);
        assert!(matches!(
            mux.create_workspace(session, None),
            Err(MuxError::IdExhausted)
        ));
    }

    #[test]
    fn failed_spawn_does_not_publish_a_tab_or_replace_siblings() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let first = mux
            .open_tab(workspace, &command("printf READY; read value"))
            .unwrap();
        let mut invalid = command("");
        invalid.program = "/huterm-nonexistent-shell".into();
        assert!(mux.open_tab(workspace, &invalid).is_err());
        assert_eq!(mux.workspace(workspace).unwrap().tabs, vec![first.tab]);
        assert_eq!(mux.terminal_count(), 1);
        wait_for_text(&first.client, "READY");
        mux.shutdown().unwrap();
    }

    #[test]
    fn closing_tabs_preserves_order_and_other_workspace_processes() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let first_workspace = mux.create_workspace(session, None).unwrap();
        let second_workspace = mux.create_workspace(session, None).unwrap();
        let script =
            "printf READY; read value; printf 'GOT:%s' \"$value\"; read value";
        let first = mux.open_tab(first_workspace, &command(script)).unwrap();
        let second = mux.open_tab(first_workspace, &command(script)).unwrap();
        let third = mux.open_tab(second_workspace, &command(script)).unwrap();
        assert_ne!(first.tab.terminal_id, third.tab.terminal_id);
        mux.close_tab(first_workspace, first.tab.id).unwrap();
        assert!(matches!(
            first.client.read_snapshot(),
            Err(RuntimeError::Stopped)
        ));
        assert_eq!(
            mux.workspace(first_workspace).unwrap().tabs,
            vec![second.tab]
        );
        mux.close_workspace(first_workspace).unwrap();
        assert!(matches!(
            second.client.read_snapshot(),
            Err(RuntimeError::Stopped)
        ));
        wait_for_text(&third.client, "READY");
        third
            .client
            .send_input(TerminalInput::Text("alive\n".into()))
            .unwrap();
        wait_for_text(&third.client, "GOT:alive");
        assert!(mux.workspace(first_workspace).is_none());
        assert_eq!(mux.terminal_count(), 1);
        assert!(matches!(
            mux.close_tab(second_workspace, first.tab.id),
            Err(MuxError::UnknownTab(_))
        ));
        mux.shutdown().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn deleting_a_session_reaps_its_child_process() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let opened = mux
            .open_tab(
                workspace,
                &command("printf 'PID:%s:READY' \"$$\"; read value"),
            )
            .unwrap();
        wait_for_text(&opened.client, "READY");
        let snapshot = opened.client.read_snapshot().unwrap();
        let text: String =
            snapshot.cells().map(|cell| cell.text.as_str()).collect();
        let pid = text
            .split("PID:")
            .nth(1)
            .unwrap()
            .split(':')
            .next()
            .unwrap()
            .parse::<i32>()
            .unwrap();
        mux.close_session(session).unwrap();
        assert_eq!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
            Err(nix::errno::Errno::ESRCH)
        );
    }

    #[test]
    fn detaching_preserves_terminal_and_unobserved_output_and_exit() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let opened = mux.open_tab(workspace, &command("read value; printf '\x1b]0;finished\x07BACKGROUND'; exit 7")).unwrap();
        let terminal_id = opened.tab.terminal_id;
        drop(opened.client);
        let client = mux.attach(terminal_id).unwrap();
        client
            .send_input(TerminalInput::Text("go\n".into()))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut title = false;
        let mut exited = false;
        // No snapshot requests while the terminal is hidden.
        while !title || !exited {
            while let Some(event) = client.try_recv_event().unwrap() {
                match event {
                    TerminalEvent::TitleChanged { title: value, .. } => {
                        title |= value == "finished";
                    }
                    TerminalEvent::Exited { status, .. } => {
                        assert_eq!(status.code, Some(7));
                        exited = true;
                    }
                    _ => {}
                }
            }
            assert!(Instant::now() < deadline, "missing title or exit");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(mux.workspace(workspace).unwrap().tabs.len(), 1);
        wait_for_text(&client, "BACKGROUND");
        mux.shutdown().unwrap();
    }
}
