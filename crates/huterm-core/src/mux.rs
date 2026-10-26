use std::collections::BTreeMap;
mod hierarchy;
mod lifecycle;
pub use hierarchy::{HierarchyRecvError, HierarchySubscription};
pub use lifecycle::{
    CloseAssessment, CloseEffect, CloseRequest, CloseTicket, HierarchySnapshot,
};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::terminal::RuntimeClient;
use crate::{RuntimeError, TerminalRuntime, TerminalViewer, ViewerOptions};
use hierarchy::HierarchyPublisher;
use huterm_protocol::{
    AttachmentId, HierarchyEvent, LayoutError, NodeId, PaneId, PaneInfo,
    PaneLayout, RuntimeId, SessionId, SplitDirection, SplitId, TabId,
    TerminalCommand, TerminalId, WorkspaceId,
};
use thiserror::Error;

/// Logical default socket name. No endpoint is opened by the runtime.
pub const DEFAULT_SOCKET_NAME: &str = "default";
static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(1);

/// One tab: its split tree of panes and its shared zoom.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tab {
    /// Stable tab identity.
    pub id: TabId,
    custom_name: Option<String>,
    layout: PaneLayout,
    zoomed: Option<PaneId>,
}
impl Tab {
    /// Returns the custom override, if set.
    #[must_use]
    pub fn custom_name(&self) -> Option<&str> {
        self.custom_name.as_deref()
    }
    /// The tab's split tree of panes.
    #[must_use]
    pub fn layout(&self) -> &PaneLayout {
        &self.layout
    }
    /// The first pane in traversal order.
    #[must_use]
    pub fn first_pane(&self) -> &PaneInfo {
        self.layout.first_pane()
    }
    /// The pane zoomed for clients that share zoom state.
    #[must_use]
    pub fn zoomed(&self) -> Option<PaneId> {
        self.zoomed
    }
    /// Every terminal the tab's panes show, in traversal order.
    #[must_use]
    pub fn terminals(&self) -> Vec<TerminalId> {
        self.layout
            .panes()
            .into_iter()
            .map(|pane| pane.terminal_id)
            .collect()
    }
    /// Whether a pane of the tab shows `terminal`.
    #[must_use]
    pub fn shows(&self, terminal: TerminalId) -> bool {
        self.layout
            .panes()
            .into_iter()
            .any(|pane| pane.terminal_id == terminal)
    }
    /// Resolves the override, current terminal title, then the first pane's
    /// launched program name.
    #[must_use]
    pub fn display_name<'a>(
        &'a self,
        current_terminal_title: &'a str,
    ) -> &'a str {
        huterm_protocol::resolve_tab_name(
            self.custom_name.as_deref(),
            current_terminal_title,
            &self.first_pane().fallback_name,
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
    /// Selected pane, if any.
    pub pane: Option<PaneId>,
}
/// A successfully published tab. Clients display its terminal through
/// [`Mux::subscribe_terminal`].
#[derive(Debug)]
pub struct OpenedTab {
    /// Canonical tab record.
    pub tab: Tab,
}
/// A pane a split published. Clients display its terminal through
/// [`Mux::subscribe_terminal`].
#[derive(Debug)]
pub struct OpenedPane {
    /// The tab that holds the pane.
    pub tab: TabId,
    /// The new pane.
    pub pane: PaneInfo,
}

/// Canonical session, workspace, and terminal owner.
///
/// Structural operations require exclusive access. Spawn and close block while
/// creating or joining workers; clients must call them on a worker thread.
/// Runtime scopes are unique within this process; a future wire protocol must
/// assign incarnation identity across processes and restarts as well.
#[derive(Debug)]
pub struct Mux {
    /// Advances on every change that can affect a close ticket's scope.
    /// Pane resizes and shared zoom leave it alone.
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
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
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
            pane: None,
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
            pane: None,
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
            pane: None,
        })
    }
    /// Resolves a pane selection to its current tab, workspace, and session.
    /// # Errors
    /// Rejects foreign or missing identities.
    pub fn select_pane(&self, id: PaneId) -> Result<SelectionTarget, MuxError> {
        self.validate_scope(id.runtime())?;
        let (workspace, tab) = self
            .workspaces
            .values()
            .find_map(|w| {
                w.tabs
                    .iter()
                    .find(|t| t.layout.pane(id).is_some())
                    .map(|t| (w, t.id))
            })
            .ok_or(MuxError::UnknownPane(id))?;
        Ok(SelectionTarget {
            session: workspace.session_id,
            workspace: Some(workspace.id),
            tab: Some(tab),
            pane: Some(id),
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
        let id = TabId::in_runtime(self.runtime_id, value);
        let pane = self.new_pane(command)?;
        let terminal_id = pane.terminal_id;
        let tab = Tab {
            id,
            custom_name: None,
            layout: PaneLayout::single(pane),
            zoomed: None,
        };
        let runtime =
            TerminalRuntime::spawn_in(self.runtime_id, terminal_id, command)?;
        let tabs = &mut self
            .workspaces
            .get_mut(&workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?
            .tabs;
        tabs.push(tab.clone());
        let index = position(tabs.len() - 1);
        self.terminals.insert(terminal_id, runtime);
        self.emit_hierarchy(|_| HierarchyEvent::TabOpened {
            workspace,
            tab: tab.info(),
            index,
        });
        Ok(OpenedTab { tab })
    }
    /// Allocates a pane and its terminal identity for `command`.
    fn new_pane(
        &mut self,
        command: &TerminalCommand,
    ) -> Result<PaneInfo, MuxError> {
        Ok(PaneInfo {
            id: PaneId::in_runtime(self.runtime_id, self.allocate()?),
            terminal_id: TerminalId::new(self.allocate()?),
            fallback_name: command
                .program
                .file_name()
                .unwrap_or(command.program.as_os_str())
                .to_string_lossy()
                .into_owned(),
        })
    }
    /// Splits `pane` toward `direction` with a new terminal running
    /// `command`. The terminal spawns before the layout changes, so a failed
    /// spawn changes no structure and emits nothing. A split clears the
    /// tab's shared zoom.
    /// # Errors
    /// Rejects foreign, missing, or nonmember targets, a pane too small or a
    /// parent too full to split, exhausted identities, and failed spawns.
    pub fn split_pane(
        &mut self,
        workspace: WorkspaceId,
        tab: TabId,
        pane: PaneId,
        direction: SplitDirection,
        command: &TerminalCommand,
    ) -> Result<OpenedPane, MuxError> {
        self.select_workspace_pane(workspace, tab, pane)?;
        let added = self.new_pane(command)?;
        let split = SplitId::in_runtime(self.runtime_id, self.allocate()?);
        let layout = self.tab_record(workspace, tab)?.layout.with_split(
            pane,
            direction,
            added.clone(),
            split,
        );
        let layout = layout.map_err(|error| layout_error(error, pane))?;
        let runtime = TerminalRuntime::spawn_in(
            self.runtime_id,
            added.terminal_id,
            command,
        )?;
        self.changed();
        let record = self.tab_record_mut(workspace, tab)?;
        record.layout = layout;
        record.zoomed = None;
        self.terminals.insert(added.terminal_id, runtime);
        self.emit_layout(workspace, tab);
        Ok(OpenedPane { tab, pane: added })
    }
    /// Closes a pane and joins its terminal workers, leaving its siblings
    /// running; they take its space. Closing a tab's only pane closes the
    /// tab.
    /// # Errors
    /// Rejects foreign, missing, or nonmember targets, or reports failed
    /// terminal cleanup after the pane was removed.
    pub fn close_pane(
        &mut self,
        workspace: WorkspaceId,
        tab: TabId,
        pane: PaneId,
    ) -> Result<(), MuxError> {
        self.select_workspace_pane(workspace, tab, pane)?;
        let record = self.tab_record(workspace, tab)?;
        let layout = match record.layout.without_pane(pane) {
            Err(LayoutError::LastPane) => {
                return self.close_tab(workspace, tab);
            }
            result => result.map_err(|error| layout_error(error, pane))?,
        };
        let terminal = record
            .layout
            .pane(pane)
            .ok_or(MuxError::UnknownPane(pane))?
            .terminal_id;
        self.changed();
        let record = self.tab_record_mut(workspace, tab)?;
        record.layout = layout;
        if record.zoomed == Some(pane) {
            record.zoomed = None;
        }
        self.emit_layout(workspace, tab);
        self.close_terminals(vec![terminal])
    }
    /// Sets the weights of a split's children, which must still be exactly
    /// `children`, in order. Weights never change which terminals a close
    /// covers, so pending close assessments stay valid.
    /// # Errors
    /// Rejects foreign, missing, or nonmember targets, a split whose
    /// children changed, and invalid weights.
    pub fn resize_split(
        &mut self,
        workspace: WorkspaceId,
        tab: TabId,
        split: SplitId,
        children: &[NodeId],
        weights: &[u32],
    ) -> Result<(), MuxError> {
        self.select_workspace_tab(workspace, tab)?;
        self.validate_scope(split.runtime())?;
        let record = self.tab_record(workspace, tab)?;
        let layout = record
            .layout
            .with_weights(split, children, weights)
            .map_err(|error| match error {
                LayoutError::UnknownSplit => MuxError::UnknownSplit(split),
                error => MuxError::InvalidLayout(error),
            })?;
        if layout == record.layout {
            return Ok(());
        }
        self.tab_record_mut(workspace, tab)?.layout = layout;
        self.emit_layout(workspace, tab);
        Ok(())
    }
    /// Sets or clears the tab's shared zoom, which only clients that share
    /// zoom state follow. Zoom never changes which terminals a close
    /// covers, so pending close assessments stay valid.
    /// # Errors
    /// Rejects foreign, missing, or nonmember targets, and zooming a tab
    /// with one pane.
    pub fn set_pane_zoom(
        &mut self,
        workspace: WorkspaceId,
        tab: TabId,
        zoomed: Option<PaneId>,
    ) -> Result<(), MuxError> {
        match zoomed {
            Some(pane) => self.select_workspace_pane(workspace, tab, pane)?,
            None => self.select_workspace_tab(workspace, tab)?,
        }
        let record = self.tab_record_mut(workspace, tab)?;
        if zoomed.is_some() && record.layout.pane_count() < 2 {
            return Err(MuxError::InvalidLayout(LayoutError::LastPane));
        }
        if record.zoomed == zoomed {
            return Ok(());
        }
        record.zoomed = zoomed;
        self.emit_hierarchy(|_| HierarchyEvent::PaneZoomChanged {
            tab,
            zoomed,
        });
        Ok(())
    }
    /// Validates that a pane belongs to the tab in the workspace.
    fn select_workspace_pane(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
        pane: PaneId,
    ) -> Result<(), MuxError> {
        self.select_workspace_tab(workspace, tab)?;
        self.validate_scope(pane.runtime())?;
        if self.tab_record(workspace, tab)?.layout.pane(pane).is_some() {
            Ok(())
        } else {
            Err(MuxError::UnknownPane(pane))
        }
    }
    fn tab_record(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
    ) -> Result<&Tab, MuxError> {
        self.workspaces
            .get(&workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?
            .tabs
            .iter()
            .find(|t| t.id == tab)
            .ok_or(MuxError::UnknownTab(tab))
    }
    fn tab_record_mut(
        &mut self,
        workspace: WorkspaceId,
        tab: TabId,
    ) -> Result<&mut Tab, MuxError> {
        self.workspaces
            .get_mut(&workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?
            .tabs
            .iter_mut()
            .find(|t| t.id == tab)
            .ok_or(MuxError::UnknownTab(tab))
    }
    fn emit_layout(&mut self, workspace: WorkspaceId, tab: TabId) {
        self.emit_hierarchy(|mux| {
            let record = &mux.workspaces[&workspace]
                .tabs
                .iter()
                .find(|t| t.id == tab)
                .expect("an emitted layout's tab exists");
            HierarchyEvent::PaneLayoutChanged {
                tab,
                layout: record.layout.clone(),
                zoomed: record.zoomed,
            }
        });
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
        if source_session != destination_session {
            for terminal_id in
                self.workspaces[&source].tabs[source_index].terminals()
            {
                self.invalidate_terminal_authority(terminal_id);
            }
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
                .flat_map(Tab::terminals)
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
    /// The crate-private client close assessment uses for job queries.
    pub(crate) fn attach(
        &self,
        terminal_id: TerminalId,
    ) -> Option<RuntimeClient> {
        self.terminals
            .get(&terminal_id)
            .map(TerminalRuntime::client)
    }

    /// Registers a viewer of a terminal in the attachment's session.
    ///
    /// The viewer belongs to the attachment: detaching, retargeting, or
    /// closing its session, or moving the terminal to another session,
    /// revokes it.
    ///
    /// # Errors
    /// Rejects missing attachments and terminals outside the attachment's
    /// session, and reports a stopped terminal or a full viewer limit.
    pub fn subscribe_terminal(
        &self,
        attachment: AttachmentId,
        terminal_id: TerminalId,
        options: ViewerOptions,
    ) -> Result<TerminalViewer, MuxError> {
        let session = self.attachment_session(attachment)?;
        let is_member = self.workspaces.values().any(|workspace| {
            workspace.session_id == session
                && workspace.tabs.iter().any(|tab| tab.shows(terminal_id))
        });
        let runtime = self
            .terminals
            .get(&terminal_id)
            .filter(|_| is_member)
            .ok_or(MuxError::TerminalNotInAttachment {
                terminal: terminal_id,
                attachment,
            })?;
        Ok(runtime.subscribe_for(Some(attachment), options)?)
    }

    fn invalidate_terminal_authority(&mut self, terminal_id: TerminalId) {
        if let Some(runtime) = self.terminals.get(&terminal_id) {
            runtime.host_effect_sink().invalidate_all();
            runtime.registry().revoke_all();
        }
    }

    fn invalidate_attachment_authority(&mut self, attachment: AttachmentId) {
        for runtime in self.terminals.values() {
            runtime.host_effect_sink().invalidate_attachment(attachment);
            runtime.registry().revoke_attachment(attachment);
        }
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
        self.close_terminals(tab.terminals())
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
            record.tabs.iter().flat_map(Tab::terminals).collect(),
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
            .flat_map(|t| t.terminals())
            .collect();
        self.emit_hierarchy(|_| HierarchyEvent::SessionClosed { session });
        self.close_terminals(terminals)
    }
    fn close_terminals(
        &mut self,
        terminals: Vec<TerminalId>,
    ) -> Result<(), MuxError> {
        // Read before the close signals below, which end every runtime.
        let stopped: Vec<bool> = terminals
            .iter()
            .map(|id| {
                self.terminals
                    .get(id)
                    .is_some_and(|runtime| runtime.registry().is_closed())
            })
            .collect();
        for id in &terminals {
            if let Some(runtime) = self.terminals.get(id) {
                let _ = runtime.client().close();
            }
        }
        let mut failure = None;
        for (id, stopped) in terminals.into_iter().zip(stopped) {
            if let Some(runtime) = self.terminals.remove(&id)
                && let Err(error) = shut_down(runtime, stopped)
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
/// Stops and joins a runtime Mux no longer owns. A runtime that had
/// `stopped` on its own before this close already surfaced its failure to
/// its viewers, and its teardown stopped its child, so only a timeout, which
/// may leave a child alive, still reports. A runtime that fails during this
/// close reports as usual.
fn shut_down(
    runtime: TerminalRuntime,
    stopped: bool,
) -> Result<(), RuntimeError> {
    match runtime.shutdown() {
        Err(error)
            if stopped && !matches!(error, RuntimeError::ShutdownTimedOut) =>
        {
            Ok(())
        }
        result => result,
    }
}
/// A tab opened by a test with a full-capability viewer subscribed at
/// spawn, so tests can read the terminal after it closes.
#[cfg(test)]
pub(crate) struct TestTab {
    pub(crate) tab: Tab,
    pub(crate) client: crate::test_support::TestClient,
}

#[cfg(test)]
impl Mux {
    pub(crate) fn open_test_tab(
        &mut self,
        workspace: WorkspaceId,
        command: &TerminalCommand,
    ) -> Result<TestTab, MuxError> {
        let opened = self.open_tab(workspace, command)?;
        let client = crate::test_support::TestClient::new(
            &self.terminals[&opened.tab.first_pane().terminal_id],
        );
        Ok(TestTab {
            tab: opened.tab,
            client,
        })
    }

    /// Splits a pane like [`Mux::split_pane`] and subscribes a
    /// full-capability viewer to the new terminal.
    pub(crate) fn split_test_pane(
        &mut self,
        workspace: WorkspaceId,
        tab: TabId,
        pane: PaneId,
        direction: SplitDirection,
        command: &TerminalCommand,
    ) -> Result<TestPane, MuxError> {
        let opened =
            self.split_pane(workspace, tab, pane, direction, command)?;
        let client = crate::test_support::TestClient::new(
            &self.terminals[&opened.pane.terminal_id],
        );
        Ok(TestPane {
            pane: opened.pane,
            client,
        })
    }
}

/// A pane split by a test with a full-capability viewer subscribed at
/// spawn.
#[cfg(test)]
pub(crate) struct TestPane {
    pub(crate) pane: PaneInfo,
    pub(crate) client: crate::test_support::TestClient,
}

/// Maps a rejected layout transform on `pane` to a Mux error.
fn layout_error(error: LayoutError, pane: PaneId) -> MuxError {
    match error {
        LayoutError::UnknownPane => MuxError::UnknownPane(pane),
        LayoutError::PaneTooSmall | LayoutError::TooManyChildren => {
            MuxError::PaneTooSmall(pane)
        }
        error => MuxError::InvalidLayout(error),
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
    /// Structure or job evidence changed since consent was requested.
    #[error("close assessment changed; assess again before closing")]
    StaleClose,
    /// Current job evidence requires explicit user consent.
    #[error("close requires confirmation")]
    ConfirmationRequired,
    /// An emptied window's workspace still holds tabs it has not shown yet.
    #[error("workspace {0:?} still holds tabs")]
    ViewNotEmpty(WorkspaceId),
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
    /// Pane no longer belongs to the requested tab.
    #[error("pane {0:?} does not exist in this tab")]
    UnknownPane(PaneId),
    /// Split no longer belongs to the requested tab.
    #[error("split {0:?} does not exist in this tab")]
    UnknownSplit(SplitId),
    /// A pane's share of its split is too small, or its split too full, to
    /// split it.
    #[error("pane {0:?} is too small to split")]
    PaneTooSmall(PaneId),
    /// A layout change named different split children, invalid weights, or
    /// zoomed a tab's only pane.
    #[error("invalid pane layout: {0}")]
    InvalidLayout(LayoutError),
    /// Terminal startup or shutdown failed.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}
#[cfg(test)]
mod tests;
