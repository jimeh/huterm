use std::collections::BTreeMap;
mod hierarchy;
mod lifecycle;
pub use hierarchy::{HierarchyRecvError, HierarchySubscription};
pub use lifecycle::{
    CloseAssessment, CloseEffect, CloseRequest, CloseTicket, HierarchySnapshot,
};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::presentation::PresentationAuthority;
use crate::{
    DesktopHostEffectClient, HostEffectRecipient, HostEffectRecipientOptions,
    PresentationController, RuntimeClient, RuntimeError, TerminalRuntime,
};
use hierarchy::HierarchyPublisher;
use huterm_protocol::{
    AttachmentId, HierarchyEvent, PaneId, RuntimeId, SessionId, TabId,
    TerminalCommand, TerminalId, WorkspaceId,
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
        huterm_protocol::resolve_tab_name(
            self.custom_name.as_deref(),
            current_terminal_title,
            &self.fallback_name,
        )
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
    revision: u64,
    attachments: BTreeMap<AttachmentId, SessionId>,
    runtime_id: RuntimeId,
    socket_name: String,
    next_id: u64,
    session_ordinal: u64,
    workspace_ordinal: u64,
    sessions: Vec<Session>,
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    terminals: BTreeMap<TerminalId, TerminalRuntime>,
    presentation_authority: PresentationAuthority,
    hierarchy: HierarchyPublisher,
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
            revision: 0,
            attachments: BTreeMap::new(),
            runtime_id: RuntimeId::new(runtime_id),
            socket_name: socket_name.into(),
            next_id: 0,
            session_ordinal: 0,
            workspace_ordinal: 0,
            sessions: Vec::new(),
            workspaces: BTreeMap::new(),
            terminals: BTreeMap::new(),
            presentation_authority: PresentationAuthority::default(),
            hierarchy: HierarchyPublisher::default(),
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
    fn changed(&mut self) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("structural revision exhausted");
    }
    fn allocate(&mut self) -> Result<u64, MuxError> {
        self.changed();
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
        let index = position(self.sessions.len() - 1);
        self.emit_hierarchy(|mux| HierarchyEvent::SessionCreated {
            session: mux.sessions[mux.sessions.len() - 1].info(),
            index,
        });
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
        let siblings = &mut self
            .sessions
            .iter_mut()
            .find(|s| s.id == session)
            .ok_or(MuxError::UnknownSession(session))?
            .workspaces;
        siblings.push(id);
        let index = position(siblings.len() - 1);
        self.workspace_ordinal = ordinal;
        self.emit_hierarchy(|mux| HierarchyEvent::WorkspaceCreated {
            session,
            workspace: mux.workspaces[&id].info(),
            index,
        });
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
        let record = self
            .sessions
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(MuxError::UnknownSession(id))?;
        if record.custom_name.as_deref() == name {
            return Ok(());
        }
        record.custom_name = name.map(str::to_owned);
        self.changed();
        self.emit_hierarchy(|_| HierarchyEvent::SessionRenamed {
            session: id,
            custom_name: name.map(str::to_owned),
        });
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
        let record = self
            .workspaces
            .get_mut(&id)
            .ok_or(MuxError::UnknownWorkspace(id))?;
        if record.custom_name.as_deref() == name {
            return Ok(());
        }
        record.custom_name = name.map(str::to_owned);
        self.changed();
        self.emit_hierarchy(|_| HierarchyEvent::WorkspaceRenamed {
            workspace: id,
            custom_name: name.map(str::to_owned),
        });
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
        let record = self
            .workspaces
            .values_mut()
            .flat_map(|w| &mut w.tabs)
            .find(|t| t.id == id)
            .ok_or(MuxError::UnknownTab(id))?;
        if record.custom_name.as_deref() == name {
            return Ok(());
        }
        record.custom_name = name.map(str::to_owned);
        self.changed();
        self.emit_hierarchy(|_| HierarchyEvent::TabRenamed {
            tab: id,
            custom_name: name.map(str::to_owned),
        });
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
        let tabs = &mut self
            .workspaces
            .get_mut(&workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?
            .tabs;
        tabs.push(tab.clone());
        let index = position(tabs.len() - 1);
        self.terminals.insert(tab.terminal_id, runtime);
        self.emit_hierarchy(|_| HierarchyEvent::TabOpened {
            workspace,
            tab: tab.info(),
            index,
        });
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
        // The final position, after removing the item from its own parent.
        let target_index = target_index
            - usize::from(source == destination && source_index < target_index);
        if source == destination && source_index == target_index {
            return Ok(());
        }
        let source_session = self.workspaces[&source].session_id;
        let destination_session = self.workspaces[&destination].session_id;
        let terminal_id =
            self.workspaces[&source].tabs[source_index].terminal_id;
        if source_session != destination_session {
            self.invalidate_terminal_authority(terminal_id);
        }
        self.changed();
        let record = self
            .workspaces
            .get_mut(&source)
            .ok_or(MuxError::UnknownWorkspace(source))?
            .tabs
            .remove(source_index);
        self.workspaces
            .get_mut(&destination)
            .ok_or(MuxError::UnknownWorkspace(destination))?
            .tabs
            .insert(target_index, record);
        self.emit_hierarchy(|_| HierarchyEvent::TabMoved {
            tab,
            workspace: destination,
            index: position(target_index),
        });
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
        // The final position, after removing the item from its own parent.
        let target_index = target_index
            - usize::from(source == destination && source_index < target_index);
        if source == destination && source_index == target_index {
            return Ok(());
        }
        if source != destination {
            let terminal_ids: Vec<_> = self.workspaces[&workspace]
                .tabs
                .iter()
                .map(|tab| tab.terminal_id)
                .collect();
            for terminal_id in terminal_ids {
                self.invalidate_terminal_authority(terminal_id);
            }
        }
        self.changed();
        self.sessions[source_session]
            .workspaces
            .remove(source_index);
        self.sessions[target_session]
            .workspaces
            .insert(target_index, workspace);
        self.workspaces
            .get_mut(&workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?
            .session_id = destination;
        self.emit_hierarchy(|_| HierarchyEvent::WorkspaceMoved {
            workspace,
            session: destination,
            index: position(target_index),
        });
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

    /// Clones the current terminal clients for one bounded host operation.
    /// Callers must release the structural lock before using the handles and
    /// must not retain them beyond that operation.
    #[must_use]
    pub fn runtime_clients(&self) -> Vec<RuntimeClient> {
        self.terminals
            .values()
            .map(TerminalRuntime::client)
            .collect()
    }

    /// Registers one validated attachment view to receive host effects from a terminal.
    ///
    /// # Errors
    /// Rejects missing attachments, terminals outside the attachment's session,
    /// or exhausted bounded registration capacity.
    pub fn register_host_effect_recipient(
        &self,
        attachment: AttachmentId,
        terminal_id: TerminalId,
        process: &DesktopHostEffectClient,
        options: HostEffectRecipientOptions,
    ) -> Result<HostEffectRecipient, MuxError> {
        let session = self.attachment_session(attachment)?;
        let is_member = self.workspaces.values().any(|workspace| {
            workspace.session_id == session
                && workspace
                    .tabs
                    .iter()
                    .any(|tab| tab.terminal_id == terminal_id)
        });
        if !is_member {
            return Err(MuxError::TerminalNotInAttachment {
                terminal: terminal_id,
                attachment,
            });
        }
        self.terminals
            .get(&terminal_id)
            .and_then(|runtime| {
                runtime
                    .host_effect_sink()
                    .register(attachment, process, options)
            })
            .ok_or(MuxError::HostEffectRegistrationUnavailable)
    }

    /// Authorizes one validated attachment to publish retained presentation.
    ///
    /// A new controller supersedes the terminal's previous controller. Revoking
    /// a controller retains the last presentation already accepted by runtime.
    ///
    /// # Errors
    ///
    /// Rejects missing attachments, terminals outside the attachment's session,
    /// or exhausted controller generations.
    pub fn register_presentation_controller(
        &mut self,
        attachment: AttachmentId,
        terminal_id: TerminalId,
    ) -> Result<PresentationController, MuxError> {
        let session = self.attachment_session(attachment)?;
        let is_member = self.workspaces.values().any(|workspace| {
            workspace.session_id == session
                && workspace
                    .tabs
                    .iter()
                    .any(|tab| tab.terminal_id == terminal_id)
        });
        if !is_member {
            return Err(MuxError::TerminalNotInAttachment {
                terminal: terminal_id,
                attachment,
            });
        }
        let client = self
            .terminals
            .get(&terminal_id)
            .map(TerminalRuntime::client)
            .ok_or(MuxError::PresentationControllerUnavailable)?;
        self.presentation_authority
            .authorize(attachment, terminal_id, client)
            .ok_or(MuxError::PresentationControllerUnavailable)
    }

    fn invalidate_terminal_authority(&mut self, terminal_id: TerminalId) {
        if let Some(runtime) = self.terminals.get(&terminal_id) {
            runtime.host_effect_sink().invalidate_all();
        }
        self.presentation_authority.invalidate_terminal(terminal_id);
    }

    fn invalidate_attachment_authority(&mut self, attachment: AttachmentId) {
        for runtime in self.terminals.values() {
            runtime.host_effect_sink().invalidate_attachment(attachment);
        }
        self.presentation_authority
            .invalidate_attachment(attachment);
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
        self.changed();
        self.emit_hierarchy(|_| HierarchyEvent::TabClosed { tab: tab.id });
        self.presentation_authority
            .invalidate_terminal(tab.terminal_id);
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
        self.changed();
        let record = self
            .workspaces
            .remove(&workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?;
        for session in &mut self.sessions {
            session.workspaces.retain(|id| *id != workspace);
        }
        self.emit_hierarchy(|_| HierarchyEvent::WorkspaceClosed { workspace });
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
        self.changed();
        let invalidated_attachments: Vec<_> = self
            .attachments
            .iter()
            .filter_map(|(attachment, attached)| {
                (*attached == session).then_some(*attachment)
            })
            .collect();
        for attachment in invalidated_attachments {
            self.invalidate_attachment_authority(attachment);
        }
        self.attachments.retain(|_, attached| *attached != session);
        let record = self.sessions.remove(index);
        let terminals = record
            .workspaces
            .into_iter()
            .filter_map(|id| self.workspaces.remove(&id))
            .flat_map(|w| w.tabs)
            .map(|t| t.terminal_id)
            .collect();
        self.emit_hierarchy(|_| HierarchyEvent::SessionClosed { session });
        self.close_terminals(terminals)
    }
    fn close_terminals(
        &mut self,
        terminals: Vec<TerminalId>,
    ) -> Result<(), MuxError> {
        for id in &terminals {
            self.presentation_authority.invalidate_terminal(*id);
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
        self.changed();
        self.attachments.clear();
        self.presentation_authority.invalidate_all();
        let had_structure = !self.sessions.is_empty();
        self.sessions.clear();
        self.workspaces.clear();
        if had_structure {
            self.emit_hierarchy(|_| HierarchyEvent::Reset);
        }
        self.close_terminals(self.terminals.keys().copied().collect())
    }
    /// Returns the number of owned terminals, including exited shells.
    #[must_use]
    pub fn terminal_count(&self) -> usize {
        self.terminals.len()
    }
}
/// Converts a structural position to the protocol's fixed width.
fn position(index: usize) -> u32 {
    u32::try_from(index).expect("structural position exceeds u32")
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
    /// Attachment no longer exists.
    #[error("attachment {0:?} does not exist")]
    UnknownAttachment(AttachmentId),
    /// Terminal does not belong to the attachment's current session.
    #[error(
        "terminal {terminal:?} does not belong to attachment {attachment:?}"
    )]
    TerminalNotInAttachment {
        /// Requested terminal.
        terminal: TerminalId,
        /// Validated attachment.
        attachment: AttachmentId,
    },
    /// Bounded host-effect recipient registration could not be acquired.
    #[error("host-effect recipient registration is unavailable")]
    HostEffectRegistrationUnavailable,
    /// Presentation controller registration could not be acquired.
    #[error("presentation controller registration is unavailable")]
    PresentationControllerUnavailable,
    /// Structure or job evidence changed since consent was requested.
    #[error("close assessment changed; assess again before closing")]
    StaleClose,
    /// Current job evidence requires explicit user consent.
    #[error("close requires confirmation")]
    ConfirmationRequired,
    /// A multi-tab close request named no tabs.
    #[error("close request names no tabs")]
    EmptyClose,
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
mod tests;
