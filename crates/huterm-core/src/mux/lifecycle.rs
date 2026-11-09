#[cfg(test)]
#[path = "holder_fixture.rs"]
mod holder_fixture;

use super::{
    AttachmentId, HierarchyEvent, Mux, MuxError, PaneId, RuntimeClient,
    RuntimeId, SessionId, Tab, TabId, TerminalId, WorkspaceId,
};
use crate::JobState;
use std::time::{Duration, Instant};

const CLOSE_ASSESSMENT_MAX_AGE: Duration = Duration::from_secs(2);

/// Explicit scope requested by a desktop lifecycle operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CloseRequest {
    /// Close one view, terminating its session only if it is the final view.
    Window(AttachmentId),
    /// Explicitly terminate a session and invalidate every attachment.
    Session(SessionId),
    /// Explicitly terminate a workspace and every tab in it. Refused for a
    /// session's only workspace, which only a session close ends.
    Workspace(WorkspaceId),
    /// Close a tab in its current workspace.
    Tab {
        /// Expected owning workspace.
        workspace: WorkspaceId,
        /// Selected tab.
        tab: TabId,
    },
    /// Close one pane of a tab; the tab's only pane closes the tab.
    Pane {
        /// Expected owning workspace.
        workspace: WorkspaceId,
        /// Expected owning tab.
        tab: TabId,
        /// Selected pane.
        pane: PaneId,
    },
    /// Close several tabs of one workspace together.
    Tabs {
        /// Expected owning workspace of every tab.
        workspace: WorkspaceId,
        /// Selected tabs. Repeated entries close once; an empty list is
        /// rejected.
        tabs: Vec<TabId>,
    },
    /// Terminate every surviving session, including unattached sessions.
    Application,
    /// Close a window whose last tab view went away without stopping any
    /// terminal. Refused while `workspace` still holds tabs in the
    /// attachment's session; leaves the workspace while that session holds
    /// any other workspace, empty or not, including when `workspace` no
    /// longer belongs to it; otherwise detaches, or deletes the session
    /// once it holds no terminal and no other attachment.
    EmptiedWindow {
        /// The window's attachment.
        attachment: AttachmentId,
        /// The workspace the window showed.
        workspace: WorkspaceId,
    },
}

/// Resolved effect captured before process inspection or confirmation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CloseEffect {
    /// Only remove this attachment; the session and processes survive.
    Detach(AttachmentId),
    /// Terminate this session and invalidate its attachments.
    Session(SessionId),
    /// Terminate this workspace; its session keeps its other workspaces.
    Workspace(WorkspaceId),
    /// Keep the attachment and move its window off `workspace`, closing
    /// the workspace if it still belongs to the attachment's session and
    /// holds no tab. Stops no terminal.
    LeaveWorkspace {
        /// The window's attachment, which stays.
        attachment: AttachmentId,
        /// The workspace the window showed.
        workspace: WorkspaceId,
    },
    /// Terminate one tab.
    Tab {
        /// Expected owning workspace.
        workspace: WorkspaceId,
        /// Selected tab.
        tab: TabId,
    },
    /// Terminate one pane of a tab that keeps other panes.
    Pane {
        /// Expected owning workspace.
        workspace: WorkspaceId,
        /// Expected owning tab.
        tab: TabId,
        /// Selected pane.
        pane: PaneId,
    },
    /// Terminate several tabs of one workspace.
    Tabs {
        /// Expected owning workspace.
        workspace: WorkspaceId,
        /// Distinct tabs in request order.
        tabs: Vec<TabId>,
    },
    /// Terminate the entire runtime.
    Application,
}

/// Immutable structural close scope. Inspect jobs after releasing the Mux lock.
#[derive(Clone, Debug)]
pub struct CloseTicket {
    runtime: RuntimeId,
    revision: u64,
    request: CloseRequest,
    effect: CloseEffect,
    /// Affected terminals, in the order their job evidence is reported.
    clients: Vec<RuntimeClient>,
}

/// Job evidence bound to a structural close ticket.
#[derive(Clone, Debug)]
pub struct CloseAssessment {
    ticket: CloseTicket,
    jobs: Vec<JobState>,
    checked_at: Instant,
}
impl CloseTicket {
    /// Checks foreground and background processes on the calling worker.
    /// Must run off the UI thread and outside the structural mutex. A
    /// terminal whose runtime has stopped by now assesses as idle without
    /// asking it: its own teardown already stopped its child.
    #[must_use]
    pub fn check_jobs(&self) -> CloseAssessment {
        let stopped: Vec<bool> = self
            .clients
            .iter()
            .map(|client| client.registry().is_closed())
            .collect();
        let mut inspected = crate::jobs::inspect_all(
            self.clients
                .iter()
                .zip(&stopped)
                .filter(|(_, stopped)| !**stopped)
                .map(|(client, _)| client.job_context())
                .collect(),
        )
        .into_iter();
        let jobs = stopped
            .iter()
            .map(|stopped| {
                if *stopped {
                    JobState::Idle
                } else {
                    inspected.next().unwrap_or(JobState::Unknown)
                }
            })
            .collect();
        CloseAssessment {
            ticket: self.clone(),
            jobs,
            checked_at: Instant::now(),
        }
    }
    /// Returns the exact resolved effect.
    #[must_use]
    pub fn effect(&self) -> CloseEffect {
        self.effect.clone()
    }
}
impl CloseAssessment {
    /// Returns the exact resolved effect this evidence was gathered for.
    #[must_use]
    pub fn effect(&self) -> &CloseEffect {
        &self.ticket.effect
    }
    /// Whether any running job or unknown process state needs consent.
    #[must_use]
    pub fn needs_confirmation(&self) -> bool {
        self.jobs
            .iter()
            .any(|state| !matches!(state, JobState::Idle))
    }
    /// Returns the job evidence, in the ticket's terminal order.
    #[must_use]
    pub fn jobs(&self) -> &[JobState] {
        &self.jobs
    }
    /// Returns the affected terminals in the same order as [`Self::jobs`].
    #[must_use]
    pub fn terminals(&self) -> Vec<TerminalId> {
        self.ticket
            .clients
            .iter()
            .map(RuntimeClient::terminal_id)
            .collect()
    }
    /// Pairs each affected terminal with its job evidence.
    pub fn terminal_jobs(
        &self,
    ) -> impl Iterator<Item = (TerminalId, &JobState)> + '_ {
        self.ticket
            .clients
            .iter()
            .map(RuntimeClient::terminal_id)
            .zip(&self.jobs)
    }
    /// Carries observed job groups to terminal cleanup without process scanning.
    /// Call only for a fresh assessment immediately before explicit teardown.
    pub fn record_cleanup_groups(&self) {
        for (client, jobs) in self.ticket.clients.iter().zip(&self.jobs) {
            let groups = match jobs {
                JobState::Running(processes) => {
                    processes.iter().map(|process| process.group).collect()
                }
                _ => Vec::new(),
            };
            client.record_shutdown_groups(groups);
        }
    }
    /// Rechecks the same terminal scope off the UI thread and Mux mutex.
    #[must_use]
    pub fn recheck(&self) -> Self {
        self.ticket.check_jobs()
    }
}

impl Mux {
    /// Attaches one view to a session without claiming terminal ownership,
    /// and emits [`HierarchyEvent::AttachmentCreated`].
    /// # Errors
    /// Rejects foreign/missing sessions, exhausted identities, or a runtime
    /// at its hierarchy item limit.
    pub fn attach_session(
        &mut self,
        session: SessionId,
    ) -> Result<AttachmentId, MuxError> {
        self.select_session(session)?;
        self.admit_items(0, 1)?;
        let id = AttachmentId::in_runtime(self.runtime_id, self.allocate()?);
        self.attachments.insert(id, session);
        self.emit_hierarchy(|_| HierarchyEvent::AttachmentCreated {
            attachment: id,
            session,
        });
        Ok(id)
    }
    /// Resolves a live attachment to its session.
    /// # Errors
    /// Rejects foreign or invalidated attachments.
    pub fn attachment_session(
        &self,
        attachment: AttachmentId,
    ) -> Result<SessionId, MuxError> {
        self.validate_scope(attachment.runtime())?;
        self.attachments
            .get(&attachment)
            .copied()
            .ok_or(MuxError::UnknownAttachment(attachment))
    }
    /// Whether any attachment views the session.
    #[must_use]
    pub fn session_attached(&self, session: SessionId) -> bool {
        self.attachments
            .values()
            .any(|attached| *attached == session)
    }
    /// Detaches without closing the session, including when no views remain,
    /// and emits [`HierarchyEvent::AttachmentDetached`].
    /// # Errors
    /// Rejects foreign or invalidated attachments.
    pub fn detach_session(
        &mut self,
        attachment: AttachmentId,
    ) -> Result<(), MuxError> {
        self.attachment_session(attachment)?;
        self.changed();
        self.invalidate_attachment_authority(attachment);
        self.attachments.remove(&attachment);
        self.emit_hierarchy(|_| HierarchyEvent::AttachmentDetached {
            attachment,
        });
        Ok(())
    }
    /// Atomically switches a view's session, preserving the previous session.
    /// A switch to another session emits
    /// [`HierarchyEvent::AttachmentRetargeted`]; one to the current session
    /// changes and emits nothing.
    /// # Errors
    /// Validates both targets before changing either attachment or membership.
    pub fn retarget_attachment(
        &mut self,
        attachment: AttachmentId,
        session: SessionId,
    ) -> Result<(), MuxError> {
        let current = self.attachment_session(attachment)?;
        self.select_session(session)?;
        if current == session {
            return Ok(());
        }
        self.changed();
        self.invalidate_attachment_authority(attachment);
        self.attachments.insert(attachment, session);
        self.emit_hierarchy(|_| HierarchyEvent::AttachmentRetargeted {
            attachment,
            session,
        });
        Ok(())
    }
    /// Resolves an exact close scope while holding exclusive structural access.
    /// # Errors
    /// Rejects foreign, missing, or nonmember targets.
    pub fn prepare_close(
        &self,
        request: CloseRequest,
    ) -> Result<CloseTicket, MuxError> {
        let effect = self.resolve_close(&request)?;
        let ids = self.close_terminal_ids(&effect)?;
        Ok(CloseTicket {
            runtime: self.runtime_id,
            revision: self.revision,
            request,
            effect,
            clients: ids.into_iter().filter_map(|id| self.attach(id)).collect(),
        })
    }
    /// Resolves the effect a close request would have now.
    fn resolve_close(
        &self,
        request: &CloseRequest,
    ) -> Result<CloseEffect, MuxError> {
        Ok(match request {
            CloseRequest::Window(attachment) => {
                let session = self.attachment_session(*attachment)?;
                if self.attachments.values().filter(|s| **s == session).count()
                    > 1
                {
                    CloseEffect::Detach(*attachment)
                } else {
                    CloseEffect::Session(session)
                }
            }
            CloseRequest::Session(session) => {
                self.select_session(*session)?;
                CloseEffect::Session(*session)
            }
            CloseRequest::Workspace(workspace) => {
                let session = self.select_workspace(*workspace)?.session;
                let only = self
                    .session(session)
                    .is_none_or(|record| record.workspaces.len() < 2);
                if only {
                    return Err(MuxError::LastWorkspace(*workspace));
                }
                CloseEffect::Workspace(*workspace)
            }
            CloseRequest::Tab { workspace, tab } => {
                self.select_workspace_tab(*workspace, *tab)?;
                CloseEffect::Tab {
                    workspace: *workspace,
                    tab: *tab,
                }
            }
            CloseRequest::Pane {
                workspace,
                tab,
                pane,
            } => {
                self.select_workspace_pane(*workspace, *tab, *pane)?;
                if self.tab_record(*workspace, *tab)?.layout.pane_count() == 1 {
                    CloseEffect::Tab {
                        workspace: *workspace,
                        tab: *tab,
                    }
                } else {
                    CloseEffect::Pane {
                        workspace: *workspace,
                        tab: *tab,
                        pane: *pane,
                    }
                }
            }
            CloseRequest::Tabs { workspace, tabs } => {
                self.select_workspace(*workspace)?;
                if tabs.is_empty() {
                    return Err(MuxError::EmptyClose);
                }
                let mut distinct = Vec::with_capacity(tabs.len());
                for &tab in tabs {
                    self.select_workspace_tab(*workspace, tab)?;
                    if !distinct.contains(&tab) {
                        distinct.push(tab);
                    }
                }
                CloseEffect::Tabs {
                    workspace: *workspace,
                    tabs: distinct,
                }
            }
            CloseRequest::Application => CloseEffect::Application,
            CloseRequest::EmptiedWindow {
                attachment,
                workspace,
            } => self.resolve_emptied_window(*attachment, *workspace)?,
        })
    }
    /// The terminals a close effect stops, in the order its job evidence is
    /// reported.
    fn close_terminal_ids(
        &self,
        effect: &CloseEffect,
    ) -> Result<Vec<TerminalId>, MuxError> {
        let terminals = |tab: &TabId| {
            self.tab(*tab)
                .map(Tab::terminals)
                .ok_or(MuxError::UnknownTab(*tab))
        };
        Ok(match effect {
            CloseEffect::Detach(_) | CloseEffect::LeaveWorkspace { .. } => {
                Vec::new()
            }
            CloseEffect::Workspace(workspace) => self
                .workspaces
                .get(workspace)
                .ok_or(MuxError::UnknownWorkspace(*workspace))?
                .tabs
                .iter()
                .flat_map(Tab::terminals)
                .collect(),
            CloseEffect::Session(session) => self
                .workspaces
                .values()
                .filter(|w| w.session_id == *session)
                .flat_map(|w| &w.tabs)
                .flat_map(Tab::terminals)
                .collect(),
            CloseEffect::Tab { tab, .. } => terminals(tab)?,
            CloseEffect::Pane {
                workspace,
                tab,
                pane,
            } => vec![
                self.tab_record(*workspace, *tab)?
                    .layout
                    .pane(*pane)
                    .ok_or(MuxError::UnknownPane(*pane))?
                    .terminal_id,
            ],
            CloseEffect::Tabs { tabs, .. } => tabs
                .iter()
                .map(terminals)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect(),
            CloseEffect::Application => {
                self.terminals.keys().copied().collect()
            }
        })
    }
    /// Commits only fresh, structurally valid evidence matching the user's consent.
    /// The caller checks jobs on a worker immediately before acquiring Mux.
    /// Process creation after the OS snapshot remains an unavoidable race.
    /// # Errors
    /// Returns `StaleClose` without mutation when structure/evidence changed;
    /// requires consent for jobs/unknown state. Cleanup failures are reported.
    pub fn commit_close(
        &mut self,
        consent: &CloseAssessment,
        current: &CloseAssessment,
        confirmed: bool,
    ) -> Result<(), MuxError> {
        self.commit_close_with(consent, current, confirmed, |_| {})
    }
    /// Validates once, captures accepted state, then performs teardown.
    /// The callback receives immutable Mux access under the caller's structural
    /// lock, after all rejection paths and before any cleanup. Elapsed time in
    /// capture does not invalidate an already accepted close.
    /// # Errors
    /// Rejects stale scope or widened job evidence before invoking the callback;
    /// requires consent for jobs/unknown state and reports cleanup failures.
    pub fn commit_close_with(
        &mut self,
        consent: &CloseAssessment,
        current: &CloseAssessment,
        confirmed: bool,
        before_teardown: impl FnOnce(&Self),
    ) -> Result<(), MuxError> {
        self.commit_close_with_clock(
            consent,
            current,
            confirmed,
            before_teardown,
            Instant::now,
        )
    }
    fn commit_close_with_clock(
        &mut self,
        consent: &CloseAssessment,
        current: &CloseAssessment,
        confirmed: bool,
        before_teardown: impl FnOnce(&Self),
        mut now: impl FnMut() -> Instant,
    ) -> Result<(), MuxError> {
        self.validate_close_at(consent, current, confirmed, now())?;
        before_teardown(self);
        current.record_cleanup_groups();
        match &current.ticket.effect {
            CloseEffect::Detach(attachment) => self.detach_session(*attachment),
            CloseEffect::Session(session) => self.close_session(*session),
            CloseEffect::Workspace(workspace) => {
                self.close_workspace(*workspace)
            }
            CloseEffect::LeaveWorkspace {
                attachment,
                workspace,
            } => self.leave_workspace(*attachment, *workspace),
            CloseEffect::Tab { workspace, tab } => {
                self.close_tab(*workspace, *tab)
            }
            CloseEffect::Pane {
                workspace,
                tab,
                pane,
            } => self.close_pane(*workspace, *tab, *pane),
            CloseEffect::Tabs { workspace, tabs } => {
                self.close_tabs(*workspace, tabs)
            }
            CloseEffect::Application => self.shutdown(),
        }
    }
    /// Resolves an emptied window's close so that it stops no terminal,
    /// deciding under the lock and scoped to the attachment's session:
    /// refused while its workspace belongs to that session and holds
    /// tabs ([`MuxError::ViewNotEmpty`]) or is pinned
    /// ([`MuxError::PinnedWorkspace`]); a [`CloseEffect::LeaveWorkspace`] while the session holds
    /// another workspace, with or without tabs, including when the
    /// workspace no longer belongs to the session (closed, or moved to
    /// another) and the session keeps one; otherwise, when it was the
    /// session's only workspace, a detach while another attachment holds
    /// the session, else the deletion of the empty session. So an emptied
    /// window never ends a session that still holds another workspace.
    fn resolve_emptied_window(
        &self,
        attachment: AttachmentId,
        workspace: WorkspaceId,
    ) -> Result<CloseEffect, MuxError> {
        let session = self.attachment_session(attachment)?;
        self.validate_scope(workspace.runtime())?;
        let members = || {
            self.workspaces
                .values()
                .filter(move |w| w.session_id == session)
        };
        if members().any(|w| w.id == workspace && !w.tabs.is_empty()) {
            return Err(MuxError::ViewNotEmpty(workspace));
        }
        // Pinned, the emptied workspace stays and its window shows it
        // empty, even as its session's only workspace.
        if members().any(|w| w.id == workspace && w.pinned) {
            return Err(MuxError::PinnedWorkspace(workspace));
        }
        // Also true when `workspace` left the session that keeps one.
        if members().any(|w| w.id != workspace) {
            return Ok(CloseEffect::LeaveWorkspace {
                attachment,
                workspace,
            });
        }
        let shared =
            self.attachments.values().filter(|s| **s == session).count() > 1;
        if shared {
            Ok(CloseEffect::Detach(attachment))
        } else {
            Ok(CloseEffect::Session(session))
        }
    }
    /// Commits [`CloseEffect::LeaveWorkspace`]: closes `workspace` only if
    /// it still belongs to the attachment's session, holds no tab, and is
    /// not pinned, and leaves a missing, moved-away, or pinned workspace
    /// alone. The attachment stays.
    fn leave_workspace(
        &mut self,
        attachment: AttachmentId,
        workspace: WorkspaceId,
    ) -> Result<(), MuxError> {
        let session = self.attachment_session(attachment)?;
        let empty_member = self.workspaces.get(&workspace).is_some_and(|w| {
            w.session_id == session && w.tabs.is_empty() && !w.pinned
        });
        if empty_member {
            self.close_workspace(workspace)
        } else {
            Ok(())
        }
    }
    /// Validates that a tab currently belongs to the workspace.
    pub(super) fn select_workspace_tab(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
    ) -> Result<(), MuxError> {
        self.select_workspace(workspace)?;
        if self.select_tab(tab)?.workspace == Some(workspace) {
            Ok(())
        } else {
            Err(MuxError::UnknownTab(tab))
        }
    }
    /// Closes every tab like [`Mux::close_tab`], attempting each cleanup even
    /// if an earlier one fails, and reports the first failure.
    fn close_tabs(
        &mut self,
        workspace: WorkspaceId,
        tabs: &[TabId],
    ) -> Result<(), MuxError> {
        let mut outcome = Ok(());
        for &tab in tabs {
            let result = self.close_tab(workspace, tab);
            if outcome.is_ok() {
                outcome = result;
            }
        }
        outcome
    }
    /// Validates consent without mutation, allowing capture before an approved quit.
    /// # Errors
    /// Rejects stale scope, newly independent groups, lost group continuity,
    /// known-to-unknown transitions, or missing consent. Child churn within an
    /// authorized group and completed jobs do not require renewed consent.
    pub fn validate_close(
        &self,
        consent: &CloseAssessment,
        current: &CloseAssessment,
        confirmed: bool,
    ) -> Result<(), MuxError> {
        self.validate_close_at(consent, current, confirmed, Instant::now())
    }
    /// Checks only that `assessment`'s ticket still matches the current
    /// structure, without job evidence or consent, so a client can decide
    /// on the ticket's effect, such as turning an emptied window's close
    /// into Quit, knowing a commit would not go stale.
    /// # Errors
    /// Returns `StaleClose` when the structure changed since assessment.
    pub fn validate_close_scope(
        &self,
        assessment: &CloseAssessment,
    ) -> Result<(), MuxError> {
        let ticket = &assessment.ticket;
        if ticket.runtime == self.runtime_id && ticket.revision == self.revision
        {
            Ok(())
        } else {
            Err(MuxError::StaleClose)
        }
    }
    fn validate_close_at(
        &self,
        consent: &CloseAssessment,
        current: &CloseAssessment,
        confirmed: bool,
        now: Instant,
    ) -> Result<(), MuxError> {
        let ticket = &current.ticket;
        if ticket.runtime != self.runtime_id
            || ticket.revision != self.revision
            || consent.ticket.runtime != ticket.runtime
            || consent.ticket.revision != ticket.revision
            || consent.ticket.request != ticket.request
            || consent.ticket.effect != ticket.effect
            || consent.jobs.len() != current.jobs.len()
            || !current.jobs.iter().zip(&consent.jobs).all(
                |(current, consent)| crate::jobs::covered_by(current, consent),
            )
            || now.saturating_duration_since(current.checked_at)
                > CLOSE_ASSESSMENT_MAX_AGE
        {
            return Err(MuxError::StaleClose);
        }
        if current.needs_confirmation() && !confirmed {
            return Err(MuxError::ConfirmationRequired);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
