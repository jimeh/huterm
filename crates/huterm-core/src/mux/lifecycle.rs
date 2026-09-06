#[cfg(test)]
#[path = "holder_fixture.rs"]
mod holder_fixture;

use super::{
    AttachmentId, Mux, MuxError, RuntimeClient, RuntimeId, Session, SessionId,
    TabId, Workspace, WorkspaceId,
};
use crate::JobState;
use std::time::{Duration, Instant};

/// Explicit scope requested by a desktop lifecycle operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    /// Terminate every surviving session, including unattached sessions.
    Application,
}

/// Resolved effect captured before process inspection or confirmation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    /// Must run off the UI thread and outside the structural mutex.
    #[must_use]
    pub fn check_jobs(&self) -> CloseAssessment {
        let jobs = crate::jobs::inspect_all(
            self.clients
                .iter()
                .map(RuntimeClient::job_context)
                .collect(),
        );
        CloseAssessment {
            ticket: self.clone(),
            jobs,
            checked_at: Instant::now(),
        }
    }
    /// Returns the exact resolved effect.
    #[must_use]
    pub fn effect(&self) -> CloseEffect {
        self.effect
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
        self.attachment_session(attachment)?;
        self.select_session(session)?;
        self.changed();
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
        let effect = match request {
            CloseRequest::Window(attachment) => {
                let session = self.attachment_session(attachment)?;
                if self.attachments.values().filter(|s| **s == session).count()
                    > 1
                {
                    CloseEffect::Detach(attachment)
                } else {
                    CloseEffect::Session(session)
                }
            }
            CloseRequest::Session(session) => {
                self.select_session(session)?;
                CloseEffect::Session(session)
            }
            CloseRequest::Tab { workspace, tab } => {
                self.select_workspace(workspace)?;
                if self.select_tab(tab)?.workspace != Some(workspace) {
                    return Err(MuxError::UnknownTab(tab));
                }
                CloseEffect::Tab { workspace, tab }
            }
            CloseRequest::Application => CloseEffect::Application,
        };
        let ids: Vec<_> = match effect {
            CloseEffect::Detach(_) => Vec::new(),
            CloseEffect::Session(session) => self
                .workspaces
                .values()
                .filter(|w| w.session_id == session)
                .flat_map(|w| &w.tabs)
                .map(|t| t.terminal_id)
                .collect(),
            CloseEffect::Tab { tab, .. } => vec![
                self.tab(tab).ok_or(MuxError::UnknownTab(tab))?.terminal_id,
            ],
            CloseEffect::Application => {
                self.terminals.keys().copied().collect()
            }
        };
        Ok(CloseTicket {
            runtime: self.runtime_id,
            revision: self.revision,
            request,
            effect,
            clients: ids.into_iter().filter_map(|id| self.attach(id)).collect(),
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
        self.validate_close(consent, current, confirmed)?;
        before_teardown(self);
        current.record_cleanup_groups();
        match current.ticket.effect {
            CloseEffect::Detach(attachment) => self.detach_session(attachment),
            CloseEffect::Session(session) => self.close_session(session),
            CloseEffect::Tab { workspace, tab } => {
                self.close_tab(workspace, tab)
            }
            CloseEffect::Application => self.shutdown(),
        }
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
            || current.checked_at.elapsed() > Duration::from_secs(2)
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
mod tests {
    use super::*;
    use huterm_protocol::{CellSize, GridSize, TerminalCommand, TerminalInput};

    fn command(script: &str) -> TerminalCommand {
        TerminalCommand {
            engine: huterm_protocol::TerminalEngineKind::default(),
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), script.into()],
            working_directory: std::env::current_dir().unwrap(),
            environment: Vec::new(),
            grid_size: GridSize::clamped(80, 24),
            cell_size: CellSize {
                width: 8,
                height: 16,
            },
        }
    }
    fn ready(client: &RuntimeClient, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let snapshot = client.read_snapshot().unwrap();
            let output: String =
                snapshot.cells().map(|cell| cell.text.as_str()).collect();
            if output.contains(text) {
                return;
            }
            assert!(Instant::now() < deadline, "missing {text}: {output}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn attachment_detach_and_retarget_preserve_sessions_and_validate_atomically()
     {
        let mut mux = Mux::default();
        let first = mux.create_session(None).unwrap();
        let second = mux.create_session(None).unwrap();
        let attachment = mux.attach_session(first).unwrap();
        assert!(
            mux.retarget_attachment(attachment, SessionId::new(second.get()))
                .is_err()
        );
        assert_eq!(mux.attachment_session(attachment).unwrap(), first);
        mux.retarget_attachment(attachment, second).unwrap();
        assert!(mux.session(first).is_some());
        mux.detach_session(attachment).unwrap();
        assert!(mux.session(second).is_some());
        assert!(mux.attachment_session(attachment).is_err());
        let foreign = Mux::default();
        assert!(foreign.attachment_session(attachment).is_err());
    }
    #[test]
    fn concurrent_window_tickets_reassess_when_last_attachment_changes() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let first = mux.attach_session(session).unwrap();
        let second = mux.attach_session(session).unwrap();
        let a = mux
            .prepare_close(CloseRequest::Window(first))
            .unwrap()
            .check_jobs();
        let b = mux
            .prepare_close(CloseRequest::Window(second))
            .unwrap()
            .check_jobs();
        mux.commit_close(&a, &a.recheck(), false).unwrap();
        assert!(matches!(
            mux.commit_close(&b, &b.recheck(), false),
            Err(MuxError::StaleClose)
        ));
        assert_eq!(mux.attachment_session(second).unwrap(), session);
        let b = mux.prepare_close(CloseRequest::Window(second)).unwrap();
        assert_eq!(b.effect(), CloseEffect::Session(session));
        let b = b.check_jobs();
        mux.commit_close(&b, &b.recheck(), false).unwrap();
        assert!(mux.session(session).is_none());
        assert!(mux.attachment_session(second).is_err());
    }
    #[test]
    fn rejected_wrong_workspace_close_preserves_an_unrelated_assessment() {
        let mut mux = Mux::default();
        let source = mux.create_session(None).unwrap();
        let source_workspace = mux.create_workspace(source, None).unwrap();
        let opened = mux
            .open_tab(source_workspace, &command("printf READY; read value"))
            .unwrap();
        ready(&opened.client, "READY");
        let target = mux.create_session(None).unwrap();
        let wrong_workspace = mux.create_workspace(target, None).unwrap();
        let attachment = mux.attach_session(target).unwrap();
        let assessment = mux
            .prepare_close(CloseRequest::Window(attachment))
            .unwrap()
            .check_jobs();
        assert!(
            matches!(mux.close_tab(wrong_workspace, opened.tab.id), Err(MuxError::UnknownTab(id)) if id == opened.tab.id)
        );
        mux.commit_close(&assessment, &assessment.recheck(), false)
            .expect("rejected close invalidated an unrelated assessment");
        assert!(mux.session(target).is_none());
        assert_eq!(
            mux.select_tab(opened.tab.id).unwrap().workspace,
            Some(source_workspace)
        );
        assert!(opened.client.read_snapshot().is_ok());
        mux.shutdown().unwrap();
    }

    #[test]
    fn structural_changes_invalidate_consent_without_consuming_attachment() {
        let mut mux = Mux::default();
        let first = mux.create_session(None).unwrap();
        let second = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(first, None).unwrap();
        let attachment = mux.attach_session(first).unwrap();
        let ticket = mux
            .prepare_close(CloseRequest::Window(attachment))
            .unwrap()
            .check_jobs();
        mux.move_workspace(first, workspace, second, None).unwrap();
        assert!(matches!(
            mux.commit_close(&ticket, &ticket.recheck(), true),
            Err(MuxError::StaleClose)
        ));
        assert_eq!(mux.attachment_session(attachment).unwrap(), first);
        let ticket = mux
            .prepare_close(CloseRequest::Window(attachment))
            .unwrap()
            .check_jobs();
        mux.retarget_attachment(attachment, second).unwrap();
        assert!(matches!(
            mux.commit_close(&ticket, &ticket.recheck(), true),
            Err(MuxError::StaleClose)
        ));
        assert_eq!(mux.attachment_session(attachment).unwrap(), second);
    }
    #[test]
    fn detach_keeps_real_child_alive_and_last_window_close_reaps_it() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let opened = mux
            .open_tab(
                workspace,
                &command("printf READY; read value; printf ALIVE; read value"),
            )
            .unwrap();
        ready(&opened.client, "READY");
        let pid = opened.client.job_context().unwrap().shell.unwrap();
        let attachment = mux.attach_session(session).unwrap();
        mux.detach_session(attachment).unwrap();
        opened
            .client
            .send_input(TerminalInput::Text("continue\n".into()))
            .unwrap();
        ready(&opened.client, "ALIVE");
        assert_eq!(mux.capture_hierarchy().sessions.len(), 1);
        assert!(mux.capture_hierarchy().attachments.is_empty());
        let attachment = mux.attach_session(session).unwrap();
        let assessment = mux
            .prepare_close(CloseRequest::Window(attachment))
            .unwrap()
            .check_jobs();
        assert!(!assessment.needs_confirmation(), "{:?}", assessment.jobs());
        mux.commit_close(&assessment, &assessment.recheck(), false)
            .unwrap();
        assert_eq!(mux.terminal_count(), 0);
        assert_eq!(
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
                None
            ),
            Err(nix::errno::Errno::ESRCH)
        );
        assert!(opened.client.read_snapshot().is_err());
    }
    #[test]
    fn new_background_job_requires_renewed_consent_and_quit_includes_zero_views()
     {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let opened = mux.open_tab(workspace, &command("printf IDLE; read value; set -m; sleep 30 & printf BUSY; read value")).unwrap();
        ready(&opened.client, "IDLE");
        let idle = mux
            .prepare_close(CloseRequest::Application)
            .unwrap()
            .check_jobs();
        assert!(!idle.needs_confirmation(), "{:?}", idle.jobs());
        opened
            .client
            .send_input(TerminalInput::Text("go\n".into()))
            .unwrap();
        ready(&opened.client, "BUSY");
        let busy = idle.recheck();
        assert!(busy.needs_confirmation(), "{:?}", busy.jobs());
        assert!(matches!(
            mux.commit_close(&idle, &busy, true),
            Err(MuxError::StaleClose)
        ));
        assert!(matches!(
            mux.commit_close(&busy, &busy.recheck(), false),
            Err(MuxError::ConfirmationRequired)
        ));
        mux.commit_close(&busy, &busy.recheck(), true).unwrap();
        assert_eq!(mux.terminal_count(), 0);
        for state in busy.jobs() {
            if let JobState::Running(jobs) = state {
                for job in jobs {
                    assert_eq!(
                        nix::sys::signal::kill(
                            nix::unistd::Pid::from_raw(
                                i32::try_from(job.pid).unwrap()
                            ),
                            None
                        ),
                        Err(nix::errno::Errno::ESRCH),
                        "background job {} survived",
                        job.pid
                    );
                }
            }
        }
    }
    #[test]
    fn foreground_exit_and_unavailable_runtime_are_assessed_conservatively() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let opened = mux
            .open_tab(
                workspace,
                &command("printf READY; read value; set -m; sleep 30 & fg"),
            )
            .unwrap();
        ready(&opened.client, "READY");
        opened
            .client
            .send_input(TerminalInput::Text("go\n".into()))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let foreground = loop {
            let assessment = mux
                .prepare_close(CloseRequest::Session(session))
                .unwrap()
                .check_jobs();
            if assessment.jobs().iter().any(|state| matches!(state, JobState::Running(jobs) if jobs.iter().any(|job| job.foreground))) { break assessment; }
            assert!(Instant::now() < deadline, "foreground job not observed");
        };
        mux.commit_close(&foreground, &foreground.recheck(), true)
            .unwrap();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let exited = mux.open_tab(workspace, &command("printf DONE")).unwrap();
        ready(&exited.client, "DONE");
        while !exited
            .client
            .job_context()
            .is_some_and(|context| context.exited && context.pty_eof)
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let assessment = mux
            .prepare_close(CloseRequest::Application)
            .unwrap()
            .check_jobs();
        assert!(!assessment.needs_confirmation(), "{:?}", assessment.jobs());
        exited.client.close().unwrap();
        while exited.client.job_context().is_some() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let assessment = mux
            .prepare_close(CloseRequest::Application)
            .unwrap()
            .check_jobs();
        assert_eq!(assessment.jobs(), &[JobState::Unknown]);
        assert!(matches!(
            mux.commit_close(&assessment, &assessment.recheck(), false),
            Err(MuxError::ConfirmationRequired)
        ));
        mux.commit_close(&assessment, &assessment.recheck(), true)
            .unwrap();
    }

    #[test]
    fn root_exit_completes_the_terminal_even_with_a_surviving_slave_holder() {
        exited_holder(huterm_protocol::TerminalEngineKind::Alacritty);
    }

    #[test]
    fn ghostty_root_exit_completes_the_terminal_even_with_a_surviving_slave_holder()
     {
        exited_holder(huterm_protocol::TerminalEngineKind::Ghostty);
    }

    fn exited_holder(engine: huterm_protocol::TerminalEngineKind) {
        let fixture = super::holder_fixture::HolderFixture::new();
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let mut command = fixture.command();
        command.engine = engine;
        let opened = mux.open_tab(workspace, &command).unwrap();
        let helper = fixture.wait_ready();
        let root = opened.client.job_context().unwrap().shell.unwrap();
        let root = nix::unistd::Pid::from_raw(i32::try_from(root).unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !opened.client.job_context().unwrap().exited {
            assert!(
                Instant::now() < deadline,
                "root did not exit; helper={helper}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut exit_events = 0;
        while let Some(event) = opened.client.try_recv_event().unwrap() {
            if let huterm_protocol::TerminalEvent::Exited { status, .. } = event
            {
                exit_events += 1;
                assert_eq!(
                    status,
                    huterm_protocol::ExitStatus {
                        code: Some(1),
                        success: false
                    }
                );
            }
        }
        assert_eq!(exit_events, 1, "signaled root exit must be reported once");
        assert!(
            nix::sys::signal::kill(root, None).is_err(),
            "root must be reaped when its exit is published"
        );
        assert!(!fixture.directory.join("done").exists());
        assert!(nix::sys::signal::kill(helper, None).is_ok());
        let assessment = mux
            .prepare_close(CloseRequest::Application)
            .unwrap()
            .check_jobs();
        assert!(nix::sys::signal::kill(helper, None).is_ok());
        assert!(
            !assessment.needs_confirmation(),
            "live helper={helper}, context={:?}, jobs={:?}",
            opened.client.job_context(),
            assessment.jobs()
        );
        let current = assessment.recheck();
        assert_eq!(current.jobs(), &[JobState::Idle]);
        while let Some(event) = opened.client.try_recv_event().unwrap() {
            assert!(
                !matches!(event, huterm_protocol::TerminalEvent::Exited { .. }),
                "reaping emitted a second exit event"
            );
        }
        assert!(opened.client.read_snapshot().is_ok());
        mux.commit_close(&current, &current.recheck(), false)
            .unwrap();
        assert!(
            nix::sys::signal::kill(helper, None).is_ok(),
            "closing completed history must not signal old process groups"
        );
        fixture.release().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !fixture.directory.join("done").exists() {
            assert!(
                Instant::now() < deadline,
                "helper did not finish after release"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn consent_survives_child_replacement_inside_the_same_live_job_group() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let opened = mux.open_tab(workspace, &command("sleep 30 & first=$!; printf FIRST; read step; kill $first; wait $first 2>/dev/null; sleep 30 & printf SECOND; read step")).unwrap();
        ready(&opened.client, "FIRST");
        let consent = mux
            .prepare_close(CloseRequest::Application)
            .unwrap()
            .check_jobs();
        assert!(consent.needs_confirmation());
        opened
            .client
            .send_input(TerminalInput::Text("next\n".into()))
            .unwrap();
        ready(&opened.client, "SECOND");
        let current = consent.recheck();
        assert_ne!(
            consent.jobs(),
            current.jobs(),
            "fixture did not replace the child"
        );
        mux.commit_close(&consent, &current, true).unwrap();
        assert_eq!(mux.terminal_count(), 0);
        for state in current.jobs() {
            if let JobState::Running(jobs) = state {
                for job in jobs {
                    assert_eq!(
                        nix::sys::signal::kill(
                            nix::unistd::Pid::from_raw(
                                i32::try_from(job.pid).unwrap()
                            ),
                            None
                        ),
                        Err(nix::errno::Errno::ESRCH)
                    );
                }
            }
        }
    }

    #[test]
    fn accepted_capture_precedes_teardown_without_a_second_freshness_check() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let assessment = mux
            .prepare_close(CloseRequest::Application)
            .unwrap()
            .check_jobs();
        let mut captured = None;
        mux.commit_close_with(&assessment, &assessment, false, |mux| {
            captured = Some(mux.capture_hierarchy());
            // Cross the freshness deadline after validation. Capture must not
            // leave a half-accepted quit when teardown follows this callback.
            std::thread::sleep(Duration::from_millis(2100));
        })
        .unwrap();
        assert_eq!(captured.unwrap().sessions[0].id, session);
        assert!(mux.sessions().is_empty());
    }

    #[test]
    fn explicit_session_termination_invalidates_every_view() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let first = mux.attach_session(session).unwrap();
        let second = mux.attach_session(session).unwrap();
        mux.close_session(session).unwrap();
        assert!(mux.attachment_session(first).is_err());
        assert!(mux.attachment_session(second).is_err());
    }
}
