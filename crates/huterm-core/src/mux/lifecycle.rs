#[cfg(test)]
#[path = "holder_fixture.rs"]
mod holder_fixture;

use super::{
    AttachmentId, Mux, MuxError, PaneId, RuntimeClient, RuntimeId, Session,
    SessionId, Tab, TabId, TerminalId, Workspace, WorkspaceId,
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
    /// attachment's session; otherwise detaches, or deletes the session
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

/// Versionless in-memory hierarchy captured before teardown, without history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HierarchySnapshot {
    /// Logical socket name.
    pub socket_name: String,
    /// All surviving sessions in canonical order.
    pub sessions: Vec<Session>,
    /// All workspace records, including their ordered tab layouts.
    pub workspaces: Vec<Workspace>,
    /// Open view attachments and their current sessions.
    pub attachments: Vec<(AttachmentId, SessionId)>,
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
    /// Attaches one view to a session without claiming terminal ownership.
    /// # Errors
    /// Rejects foreign/missing sessions or exhausted identities.
    pub fn attach_session(
        &mut self,
        session: SessionId,
    ) -> Result<AttachmentId, MuxError> {
        self.select_session(session)?;
        let id = AttachmentId::in_runtime(self.runtime_id, self.allocate()?);
        self.attachments.insert(id, session);
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
    /// Detaches without closing the session, including when no views remain.
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
        Ok(())
    }
    /// Atomically switches a view's session, preserving the previous session.
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
        Ok(())
    }
    /// Captures every session and layout before any destructive teardown.
    #[must_use]
    pub fn capture_hierarchy(&self) -> HierarchySnapshot {
        HierarchySnapshot {
            socket_name: self.socket_name.clone(),
            sessions: self.sessions.clone(),
            workspaces: self.workspaces.values().cloned().collect(),
            attachments: self
                .attachments
                .iter()
                .map(|(a, s)| (*a, *s))
                .collect(),
        }
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
            CloseEffect::Detach(_) => Vec::new(),
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
    /// Resolves an emptied window's close so that it stops no terminal:
    /// refused while its workspace still holds tabs in the attachment's
    /// session; a detach while another attachment holds the session or the
    /// session keeps terminals in other workspaces; otherwise deletes the
    /// empty session.
    fn resolve_emptied_window(
        &self,
        attachment: AttachmentId,
        workspace: WorkspaceId,
    ) -> Result<CloseEffect, MuxError> {
        let session = self.attachment_session(attachment)?;
        self.validate_scope(workspace.runtime())?;
        let workspaces: Vec<&Workspace> = self
            .workspaces
            .values()
            .filter(|w| w.session_id == session)
            .collect();
        if workspaces
            .iter()
            .any(|w| w.id == workspace && !w.tabs.is_empty())
        {
            return Err(MuxError::ViewNotEmpty(workspace));
        }
        let shared =
            self.attachments.values().filter(|s| **s == session).count() > 1;
        if shared || workspaces.iter().any(|w| !w.tabs.is_empty()) {
            Ok(CloseEffect::Detach(attachment))
        } else {
            Ok(CloseEffect::Session(session))
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
