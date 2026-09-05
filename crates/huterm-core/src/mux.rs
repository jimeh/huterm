use std::collections::BTreeMap;

use huterm_protocol::{
    PaneId, TabId, TerminalCommand, TerminalId, WorkspaceId,
};
use thiserror::Error;

use crate::{RuntimeClient, RuntimeError, TerminalRuntime};

/// One tab's canonical single-pane layout. Splits can replace this leaf later.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tab {
    /// Stable tab identity.
    pub id: TabId,
    /// Stable pane identity.
    pub pane_id: PaneId,
    /// Terminal displayed by the pane.
    pub terminal_id: TerminalId,
}

/// Canonical workspace structure, independent of client navigation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Workspace {
    /// Stable workspace identity.
    pub id: WorkspaceId,
    /// Tabs in presentation order.
    pub tabs: Vec<Tab>,
}

/// A successfully published tab and its in-process terminal attachment.
#[derive(Debug)]
pub struct OpenedTab {
    /// Canonical tab record.
    pub tab: Tab,
    /// Terminal attachment. Dropping it does not close the terminal.
    pub client: RuntimeClient,
}

/// Canonical workspace and terminal owner.
///
/// Structural operations require exclusive access. Spawn and close block while
/// creating or joining workers; clients must call them on a worker thread.
#[derive(Debug, Default)]
pub struct Mux {
    next_id: u64,
    workspaces: BTreeMap<WorkspaceId, Workspace>,
    terminals: BTreeMap<TerminalId, TerminalRuntime>,
}

impl Mux {
    /// Advances allocation past a saved identity before restoring records.
    pub fn reserve_through(&mut self, id: u64) {
        self.next_id = self.next_id.max(id);
    }

    fn allocate(&mut self) -> Result<u64, MuxError> {
        self.next_id =
            self.next_id.checked_add(1).ok_or(MuxError::IdExhausted)?;
        Ok(self.next_id)
    }

    /// Creates an empty workspace.
    ///
    /// # Errors
    /// Returns an error if the identity space is exhausted.
    pub fn create_workspace(&mut self) -> Result<WorkspaceId, MuxError> {
        let id = WorkspaceId::new(self.allocate()?);
        self.workspaces.insert(
            id,
            Workspace {
                id,
                tabs: Vec::new(),
            },
        );
        Ok(id)
    }

    /// Returns a workspace's canonical structure.
    #[must_use]
    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.get(&id)
    }

    /// Creates and appends a tab only after PTY creation succeeds.
    ///
    /// # Errors
    /// Returns an error for a missing workspace, exhausted IDs, or failed spawn.
    pub fn open_tab(
        &mut self,
        workspace_id: WorkspaceId,
        command: &TerminalCommand,
    ) -> Result<OpenedTab, MuxError> {
        if !self.workspaces.contains_key(&workspace_id) {
            return Err(MuxError::UnknownWorkspace(workspace_id));
        }
        let tab = Tab {
            id: TabId::new(self.allocate()?),
            pane_id: PaneId::new(self.allocate()?),
            terminal_id: TerminalId::new(self.allocate()?),
        };
        let runtime = TerminalRuntime::spawn(tab.terminal_id, command)?;
        let client = runtime.client();
        self.workspaces
            .get_mut(&workspace_id)
            .ok_or(MuxError::UnknownWorkspace(workspace_id))?
            .tabs
            .push(tab);
        self.terminals.insert(tab.terminal_id, runtime);
        Ok(OpenedTab { tab, client })
    }

    /// Moves a tab before an existing anchor, or to the end when `before` is None.
    /// Terminal and pane identities, attachments, and processes are unchanged.
    ///
    /// # Errors
    /// Returns an error for a stale workspace, tab, or anchor without changing order.
    pub fn reorder_tab(
        &mut self,
        workspace_id: WorkspaceId,
        tab_id: TabId,
        before: Option<TabId>,
    ) -> Result<(), MuxError> {
        let workspace = self
            .workspaces
            .get_mut(&workspace_id)
            .ok_or(MuxError::UnknownWorkspace(workspace_id))?;
        let source = workspace
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .ok_or(MuxError::UnknownTab(tab_id))?;
        let destination =
            before.map_or(Ok(workspace.tabs.len()), |anchor| {
                workspace
                    .tabs
                    .iter()
                    .position(|tab| tab.id == anchor)
                    .ok_or(MuxError::UnknownTab(anchor))
            })?;
        if source == destination {
            return Ok(());
        }
        let tab = workspace.tabs.remove(source);
        workspace.tabs.insert(
            if source < destination {
                destination - 1
            } else {
                destination
            },
            tab,
        );
        Ok(())
    }

    /// Attaches to a terminal without transferring runtime ownership.
    ///
    /// Events currently have one consumer; simultaneous clients need broadcast.
    #[must_use]
    pub fn attach(&self, terminal_id: TerminalId) -> Option<RuntimeClient> {
        self.terminals
            .get(&terminal_id)
            .map(TerminalRuntime::client)
    }

    /// Closes a tab and joins its terminal workers, leaving siblings intact.
    ///
    /// # Errors
    /// Returns an error for a stale workspace/tab or failed terminal cleanup.
    pub fn close_tab(
        &mut self,
        workspace_id: WorkspaceId,
        tab_id: TabId,
    ) -> Result<(), MuxError> {
        let workspace = self
            .workspaces
            .get_mut(&workspace_id)
            .ok_or(MuxError::UnknownWorkspace(workspace_id))?;
        let index = workspace
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .ok_or(MuxError::UnknownTab(tab_id))?;
        let tab = workspace.tabs.remove(index);
        if let Some(runtime) = self.terminals.remove(&tab.terminal_id) {
            runtime.shutdown()?;
        }
        Ok(())
    }

    /// Deletes a workspace and stops all of its terminals.
    ///
    /// # Errors
    /// Returns an error for a stale workspace or failed terminal cleanup. All terminal cleanups
    /// are attempted even if one fails.
    pub fn close_workspace(
        &mut self,
        workspace_id: WorkspaceId,
    ) -> Result<(), MuxError> {
        let workspace = self
            .workspaces
            .remove(&workspace_id)
            .ok_or(MuxError::UnknownWorkspace(workspace_id))?;
        let mut failure = None;
        for tab in &workspace.tabs {
            if let Some(runtime) = self.terminals.get(&tab.terminal_id) {
                let _ = runtime.client().close();
            }
        }
        for tab in workspace.tabs {
            if let Some(runtime) = self.terminals.remove(&tab.terminal_id)
                && let Err(error) = runtime.shutdown()
            {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), |error| Err(error.into()))
    }

    /// Stops and joins every terminal, including unattached workspaces.
    ///
    /// # Errors
    /// Returns the last failed terminal cleanup after attempting every cleanup.
    pub fn shutdown(&mut self) -> Result<(), MuxError> {
        for runtime in self.terminals.values() {
            let _ = runtime.client().close();
        }
        self.workspaces.clear();
        let mut failure = None;
        for runtime in std::mem::take(&mut self.terminals).into_values() {
            if let Err(error) = runtime.shutdown() {
                failure = Some(error);
            }
        }
        failure.map_or(Ok(()), |error| Err(error.into()))
    }

    /// Returns the number of owned terminals, including exited shells.
    #[must_use]
    pub fn terminal_count(&self) -> usize {
        self.terminals.len()
    }
}

/// Structural runtime operation failure.
#[derive(Debug, Error)]
pub enum MuxError {
    /// No more stable identities can be allocated.
    #[error("workspace identity space exhausted")]
    IdExhausted,
    /// Workspace no longer exists.
    #[error("workspace {0:?} does not exist")]
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
    use huterm_protocol::{
        CellSize, GridSize, TerminalEvent, TerminalInput, Viewport,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn reorder_preserves_live_terminals_and_rejects_stale_or_foreign_anchors() {
        let mut mux = Mux::default();
        let workspace = mux.create_workspace().unwrap();
        let sibling = mux.create_workspace().unwrap();
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
            [third.tab, first.tab, second.tab]
        );
        mux.reorder_tab(workspace, third.tab.id, None).unwrap();
        assert_eq!(mux.workspace(workspace).unwrap().tabs, original);
        for anchor in [Some(first.tab.id), Some(second.tab.id)] {
            mux.reorder_tab(workspace, first.tab.id, anchor).unwrap();
            assert_eq!(mux.workspace(workspace).unwrap().tabs, original);
        }
        for (tab, anchor) in [
            (first.tab.id, Some(TabId::new(u64::MAX))),
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
            mux.reorder_tab(WorkspaceId::new(u64::MAX), first.tab.id, None),
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
            let snapshot = client.read_snapshot(Viewport::default()).unwrap();
            let text: String = snapshot
                .cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect();
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
        mux.reserve_through(90);
        assert_eq!(mux.create_workspace().unwrap(), WorkspaceId::new(91));
        mux.reserve_through(1);
        assert_eq!(mux.create_workspace().unwrap(), WorkspaceId::new(92));
        mux.reserve_through(u64::MAX);
        assert!(matches!(mux.create_workspace(), Err(MuxError::IdExhausted)));
    }

    #[test]
    fn failed_spawn_does_not_publish_a_tab_or_replace_siblings() {
        let mut mux = Mux::default();
        let workspace = mux.create_workspace().unwrap();
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
        let first_workspace = mux.create_workspace().unwrap();
        let second_workspace = mux.create_workspace().unwrap();
        let script =
            "printf READY; read value; printf 'GOT:%s' \"$value\"; read value";
        let first = mux.open_tab(first_workspace, &command(script)).unwrap();
        let second = mux.open_tab(first_workspace, &command(script)).unwrap();
        let third = mux.open_tab(second_workspace, &command(script)).unwrap();
        assert_ne!(first.tab.terminal_id, third.tab.terminal_id);
        mux.close_tab(first_workspace, first.tab.id).unwrap();
        assert!(matches!(
            first.client.read_snapshot(Viewport::default()),
            Err(RuntimeError::Stopped)
        ));
        assert_eq!(
            mux.workspace(first_workspace).unwrap().tabs,
            vec![second.tab]
        );
        mux.close_workspace(first_workspace).unwrap();
        assert!(matches!(
            second.client.read_snapshot(Viewport::default()),
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
    fn deleting_a_workspace_reaps_its_child_process() {
        let mut mux = Mux::default();
        let workspace = mux.create_workspace().unwrap();
        let opened = mux
            .open_tab(
                workspace,
                &command("printf 'PID:%s:READY' \"$$\"; read value"),
            )
            .unwrap();
        wait_for_text(&opened.client, "READY");
        let snapshot =
            opened.client.read_snapshot(Viewport::default()).unwrap();
        let text: String = snapshot
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        let pid = text
            .split("PID:")
            .nth(1)
            .unwrap()
            .split(':')
            .next()
            .unwrap()
            .parse::<i32>()
            .unwrap();
        mux.close_workspace(workspace).unwrap();
        assert_eq!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
            Err(nix::errno::Errno::ESRCH)
        );
    }

    #[test]
    fn detaching_preserves_terminal_and_unobserved_output_and_exit() {
        let mut mux = Mux::default();
        let workspace = mux.create_workspace().unwrap();
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
