//! Session switching: a window shows another session, both sessions keep
//! running, and the window returns to its own navigation in each.
//!
//! A switch runs in four phases, and the window counts as busy from the
//! first to the last:
//!
//! 1. Guard: refused while the window is busy, closing, quitting, showing
//!    a confirmation, reserved, or restoring, and during teardown. Drags
//!    are cancelled, as a new tab cancels them.
//! 2. Prepare and commit on a worker under one Mux lock: validate the
//!    attachment and the target, resolve the target workspace, open a shell
//!    tab only when no workspace holds a tab, then retarget the attachment.
//!    A failure before the commit closes only what the switch created.
//! 3. Install: once the projection holds the commit, the window remembers
//!    the session it leaves, adopts the target, and reconciles with
//!    removals allowed and the emptied edge suppressed: the old views drop
//!    and the target's tabs install.
//! 4. Settle: once installs are idle, the remembered focus applies through
//!    `apply_reopen`, the phase clears, and queued closes run.
//!
//! The same workspace resolution opens a window on an existing session
//! (`open_session_in_new_window`, and a reopen after the last window
//! closed) and moves a window off a session Close Session closes, or off
//! the emptied session of a last window whose Quit was cancelled.

use super::model::NavigationMemory;
use super::*;

/// Where a window's session switch is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SwitchPhase {
    /// The worker prepares the target and retargets the attachment.
    Preparing,
    /// The window adopted the target and installs its views.
    Installing,
}

/// What a switch shows.
#[derive(Clone, Debug)]
pub(super) enum SwitchTarget {
    /// A new session, with an optional custom name.
    New(Option<String>),
    Existing(SessionId),
}

/// Why a switch, or an open of a session, changed nothing.
#[derive(Debug)]
pub(super) enum SwitchError {
    /// The target session closed, or never belonged to this runtime.
    TargetGone,
    /// The window's own attachment is invalid: its session closed.
    SourceGone,
    /// A shell the switch needed could not start.
    Spawn(MuxError),
    /// A shell was needed, but no shell command could be built.
    Shell(String),
    /// Any other runtime refusal, including teardown.
    Runtime(MuxError),
}

/// What an own-session Close Session's follow-up needs: the session the
/// close ends, the sessions it may move the window to, in the order it
/// tries them, each with its remembered workspace, and the shell the one
/// it reaches, or a fresh session, may need, or why none could be built.
pub(super) struct CloseFollow {
    pub(super) closing: SessionId,
    pub(super) candidates: Vec<(SessionId, Option<WorkspaceId>)>,
    pub(super) shell: Result<TerminalCommand, String>,
}

/// The shell a resolution may need to open: the command, or why none
/// could be built.
type ShellCommand<'a> = Result<&'a TerminalCommand, &'a str>;

/// When a follow-up falls back to a fresh session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Fresh {
    /// Only when a live candidate could not be shown: with no candidate
    /// left at all, nothing follows and the window closes, as settled
    /// decision 5 says.
    AfterFailure,
    /// Whenever no candidate could be shown, so the window always has
    /// something to show.
    Always,
}

/// Where a follow-up moved its window: the new attachment and what it
/// shows, or why nothing could be shown.
#[derive(Debug)]
pub(super) struct FollowUp {
    pub(super) result: Result<(AttachmentId, Switched), SwitchError>,
    /// The first candidate that could not be shown, by name, when the
    /// follow-up fell back to a fresh session because of it.
    pub(super) skipped: Option<(String, SwitchError)>,
}

/// What a close commit's worker did.
pub(super) struct CloseCommitted {
    pub(super) committed: Committed<Result<(), MuxError>>,
    /// Where an own-session close moved its window.
    pub(super) followed: Option<Committed<FollowUp>>,
    /// An emptied close that would become Quit found its ticket still
    /// fresh under the Mux lock, so it committed nothing and becomes Quit.
    pub(super) becomes_quit: bool,
}

/// Commits a close on its worker, then, for an own-session close whose
/// session is gone, moves the window to the first candidate it can show,
/// or to a fresh session. Core removes the session before it tears down
/// the session's terminals, so a commit that reports a teardown failure
/// still moved on. With `quit_check`, an emptied close that would become
/// Quit only checks its ticket under the lock: a fresh one commits nothing
/// and becomes Quit, and a stale one is refused for assessment again.
pub(super) async fn commit_close(
    runtime: Arc<DesktopRuntime>,
    assessment: CloseAssessment,
    confirmed: bool,
    windows: Option<huterm_state::DesktopLayout>,
    follow: Option<CloseFollow>,
    quit_check: bool,
) -> CloseCommitted {
    #[cfg(all(test, feature = "test-support"))]
    let fail = runtime
        .worker_hooks
        .enter(worker_hooks::Stage::Commit)
        .await;
    #[cfg(not(all(test, feature = "test-support")))]
    let fail = false;
    if quit_check {
        let mux = runtime.lock();
        let result = mux.validate_close_scope(&assessment);
        let becomes_quit = result.is_ok();
        return CloseCommitted {
            committed: Committed {
                result,
                seq: mux.hierarchy_seq(),
            },
            followed: None,
            becomes_quit,
        };
    }
    let mut committed = runtime.commit(&assessment, confirmed, windows);
    if fail {
        // A teardown failure after the runtime accepted the close.
        committed.result = Err(RuntimeError::Stopped.into());
    }
    let Some(follow) = follow.filter(|follow| {
        committed.result.is_ok()
            || runtime.lock().session(follow.closing).is_none()
    }) else {
        return CloseCommitted {
            committed,
            followed: None,
            becomes_quit: false,
        };
    };
    let followed = follow_up(
        &runtime,
        &follow.candidates,
        follow.shell.as_ref().map_err(String::as_str),
        None,
        Fresh::AfterFailure,
    )
    .await;
    CloseCommitted {
        committed,
        followed: Some(followed),
        becomes_quit: false,
    }
}

/// Runs [`DesktopRuntime::follow_up`] on its worker; tier 2 tests hold it
/// at `Stage::CloseFollow` and fail its attempts there.
#[cfg_attr(
    not(all(test, feature = "test-support")),
    expect(clippy::unused_async, reason = "only the test hook awaits")
)]
pub(super) async fn follow_up(
    runtime: &DesktopRuntime,
    candidates: &[(SessionId, Option<WorkspaceId>)],
    command: ShellCommand<'_>,
    old: Option<AttachmentId>,
    fresh: Fresh,
) -> Committed<FollowUp> {
    #[cfg(all(test, feature = "test-support"))]
    let fail = {
        let stage = worker_hooks::Stage::CloseFollow;
        let mut first = Some(runtime.worker_hooks.enter(stage).await);
        move || {
            first
                .take()
                .unwrap_or_else(|| runtime.worker_hooks.take_fail(stage))
        }
    };
    #[cfg(not(all(test, feature = "test-support")))]
    let fail = || false;
    runtime.follow_up(candidates, command, old, fresh, fail)
}

/// What a committed switch shows.
#[derive(Clone, Copy, Debug)]
pub(super) struct Switched {
    pub(super) session: SessionId,
    pub(super) workspace: WorkspaceId,
    /// The terminal of a shell the switch opened, whose first viewer
    /// reports an early exit.
    pub(super) spawned: Option<TerminalId>,
}

/// What the target needs to show: an existing workspace with a tab, or a
/// shell to open.
struct Resolved {
    switched: Switched,
    /// A workspace this resolution created.
    created_workspace: Option<WorkspaceId>,
    /// A tab this resolution opened.
    opened_tab: Option<TabId>,
}

impl Resolved {
    /// Closes only what this resolution created.
    fn undo(&self, mux: &mut Mux) {
        let workspace = self.switched.workspace;
        if let Some(tab) = self.opened_tab {
            let _ = mux.close_tab(workspace, tab);
        }
        if let Some(workspace) = self.created_workspace {
            let _ = mux.close_workspace(workspace);
        }
    }
}

fn source_error(error: MuxError) -> SwitchError {
    match error {
        MuxError::UnknownAttachment(_) | MuxError::ForeignRuntime(_) => {
            SwitchError::SourceGone
        }
        other => SwitchError::Runtime(other),
    }
}

/// Opens a shell tab running `command` in `workspace`, unless no command
/// could be built or the test hook asked the switch's spawn to fail.
fn open_shell(
    mux: &mut Mux,
    workspace: WorkspaceId,
    command: ShellCommand<'_>,
    fail: bool,
) -> Result<OpenedTab, SwitchError> {
    if fail {
        return Err(SwitchError::Spawn(
            RuntimeError::Invariant("injected spawn failure").into(),
        ));
    }
    let command =
        command.map_err(|error| SwitchError::Shell(error.to_owned()))?;
    mux.open_tab(workspace, command).map_err(SwitchError::Spawn)
}

/// The workspace `session` shows: `remembered` when it still belongs to
/// the session and holds a tab, else the first that holds one. Only when
/// none does, a shell tab opens in the first workspace, creating one when
/// the session has none.
fn resolve_workspace(
    mux: &mut Mux,
    session: SessionId,
    remembered: Option<WorkspaceId>,
    command: ShellCommand<'_>,
    fail_spawn: bool,
) -> Result<Resolved, SwitchError> {
    let record = mux.session(session).ok_or(SwitchError::TargetGone)?;
    let holds_tab = |mux: &Mux, workspace: WorkspaceId| {
        mux.workspace(workspace).is_some_and(|workspace| {
            workspace.session_id == session && !workspace.tabs.is_empty()
        })
    };
    let workspaces = record.workspaces.clone();
    let shown = remembered
        .filter(|workspace| holds_tab(mux, *workspace))
        .or_else(|| {
            workspaces
                .iter()
                .copied()
                .find(|workspace| holds_tab(mux, *workspace))
        });
    if let Some(workspace) = shown {
        return Ok(Resolved {
            switched: Switched {
                session,
                workspace,
                spawned: None,
            },
            created_workspace: None,
            opened_tab: None,
        });
    }
    let (workspace, created_workspace) =
        if let Some(workspace) = workspaces.first() {
            (*workspace, None)
        } else {
            let workspace = mux
                .create_workspace(session, None)
                .map_err(SwitchError::Spawn)?;
            (workspace, Some(workspace))
        };
    match open_shell(mux, workspace, command, fail_spawn) {
        Ok(opened) => Ok(Resolved {
            switched: Switched {
                session,
                workspace,
                spawned: Some(opened.tab.first_pane().terminal_id),
            },
            created_workspace,
            opened_tab: Some(opened.tab.id),
        }),
        Err(error) => {
            if let Some(workspace) = created_workspace {
                let _ = mux.close_workspace(workspace);
            }
            Err(error)
        }
    }
}

/// A new session with one workspace and a shell; closed again on failure.
fn create_session(
    mux: &mut Mux,
    name: Option<&str>,
    command: ShellCommand<'_>,
    fail_spawn: bool,
) -> Result<Switched, SwitchError> {
    let session = mux.create_session(name).map_err(SwitchError::Spawn)?;
    let opened = mux
        .create_workspace(session, None)
        .map_err(SwitchError::Spawn)
        .and_then(|workspace| {
            open_shell(mux, workspace, command, fail_spawn)
                .map(|opened| (workspace, opened))
        });
    match opened {
        Ok((workspace, opened)) => Ok(Switched {
            session,
            workspace,
            spawned: Some(opened.tab.first_pane().terminal_id),
        }),
        Err(error) => {
            let _ = mux.close_session(session);
            Err(error)
        }
    }
}

impl DesktopRuntime {
    /// Prepares `target` and retargets `attachment` to it under one lock.
    /// A failure before the retarget closes only what this switch created
    /// and leaves the attachment as it was.
    pub(super) fn switch_session(
        &self,
        attachment: AttachmentId,
        target: &SwitchTarget,
        remembered: Option<WorkspaceId>,
        command: &TerminalCommand,
        fail_spawn: bool,
    ) -> Committed<Result<Switched, SwitchError>> {
        let mut mux = self.lock();
        let result = self.switch_locked(
            &mut mux, attachment, target, remembered, command, fail_spawn,
        );
        Committed {
            result,
            seq: mux.hierarchy_seq(),
        }
    }

    fn switch_locked(
        &self,
        mux: &mut Mux,
        attachment: AttachmentId,
        target: &SwitchTarget,
        remembered: Option<WorkspaceId>,
        command: &TerminalCommand,
        fail_spawn: bool,
    ) -> Result<Switched, SwitchError> {
        if self.terminating.load(Ordering::Acquire) {
            return Err(SwitchError::Runtime(RuntimeError::Stopped.into()));
        }
        mux.attachment_session(attachment).map_err(source_error)?;
        let (switched, undo): (Switched, Option<Resolved>) = match target {
            SwitchTarget::New(name) => (
                create_session(mux, name.as_deref(), Ok(command), fail_spawn)?,
                None,
            ),
            SwitchTarget::Existing(session) => {
                mux.select_session(*session)
                    .map_err(|_| SwitchError::TargetGone)?;
                let resolved = resolve_workspace(
                    mux,
                    *session,
                    remembered,
                    Ok(command),
                    fail_spawn,
                )?;
                (resolved.switched, Some(resolved))
            }
        };
        if let Err(error) =
            mux.retarget_attachment(attachment, switched.session)
        {
            match &undo {
                Some(resolved) => resolved.undo(mux),
                None => {
                    let _ = mux.close_session(switched.session);
                }
            }
            return Err(match error {
                MuxError::UnknownSession(_) => SwitchError::TargetGone,
                other => source_error(other),
            });
        }
        Ok(switched)
    }

    /// Attaches a new view to `session`, opening a shell first only when
    /// no workspace holds a tab. A failure closes only what this created.
    pub(super) fn attach_existing(
        &self,
        session: SessionId,
        remembered: Option<WorkspaceId>,
        command: &TerminalCommand,
        fail: bool,
    ) -> Committed<Result<(AttachmentId, Switched), SwitchError>> {
        let mut mux = self.lock();
        let result = self.attach_locked(
            &mut mux,
            session,
            remembered,
            Ok(command),
            fail,
        );
        Committed {
            result,
            seq: mux.hierarchy_seq(),
        }
    }

    /// Attaches a new view, under one lock, to the first of `candidates`
    /// it can show, as [`Self::attach_existing`] does for one. It passes
    /// over a candidate that closed since the window chose it, and one
    /// that cannot be attached or needs a shell that fails to start, or
    /// that no command could be built for. With none left it attaches to a
    /// fresh session with a shell as `fresh` allows; otherwise the result
    /// is `TargetGone` with nothing skipped. Once it attached, it detaches
    /// `old`, the window's previous attachment. `fail` says whether each
    /// attempt fails, for the test hook.
    fn follow_up(
        &self,
        candidates: &[(SessionId, Option<WorkspaceId>)],
        command: ShellCommand<'_>,
        old: Option<AttachmentId>,
        fresh: Fresh,
        mut fail: impl FnMut() -> bool,
    ) -> Committed<FollowUp> {
        let mut mux = self.lock();
        let mut failed = None;
        let mut attached = None;
        for (session, remembered) in candidates {
            let Some(name) = mux
                .session(*session)
                .map(|record| record.display_name().to_owned())
            else {
                continue;
            };
            match self.attach_locked(
                &mut mux,
                *session,
                *remembered,
                command,
                fail(),
            ) {
                Ok(done) => {
                    attached = Some(done);
                    break;
                }
                Err(SwitchError::TargetGone) => {}
                Err(error) => {
                    failed.get_or_insert((name, error));
                }
            }
        }
        let (result, skipped) = match attached {
            Some(done) => (Ok(done), None),
            None if failed.is_none() && fresh == Fresh::AfterFailure => {
                (Err(SwitchError::TargetGone), None)
            }
            None => (self.attach_fresh(&mut mux, command, fail()), failed),
        };
        if result.is_ok()
            && let Some(old) = old
        {
            let _ = mux.detach_session(old);
        }
        Committed {
            result: FollowUp { result, skipped },
            seq: mux.hierarchy_seq(),
        }
    }

    /// Attaches a new view to a new session with one workspace and a
    /// shell; closes that session again on failure.
    fn attach_fresh(
        &self,
        mux: &mut Mux,
        command: ShellCommand<'_>,
        fail_spawn: bool,
    ) -> Result<(AttachmentId, Switched), SwitchError> {
        if self.terminating.load(Ordering::Acquire) {
            return Err(SwitchError::Runtime(RuntimeError::Stopped.into()));
        }
        let switched = create_session(mux, None, command, fail_spawn)?;
        match mux.attach_session(switched.session) {
            Ok(attachment) => Ok((attachment, switched)),
            Err(error) => {
                let _ = mux.close_session(switched.session);
                Err(SwitchError::Runtime(error))
            }
        }
    }

    pub(super) fn attach_locked(
        &self,
        mux: &mut Mux,
        session: SessionId,
        remembered: Option<WorkspaceId>,
        command: ShellCommand<'_>,
        fail: bool,
    ) -> Result<(AttachmentId, Switched), SwitchError> {
        if self.terminating.load(Ordering::Acquire) {
            return Err(SwitchError::Runtime(RuntimeError::Stopped.into()));
        }
        mux.select_session(session)
            .map_err(|_| SwitchError::TargetGone)?;
        let resolved =
            resolve_workspace(mux, session, remembered, command, false)?;
        let attached = if fail {
            Err(RuntimeError::Invariant("injected attach failure").into())
        } else {
            mux.attach_session(session)
        };
        match attached {
            Ok(attachment) => Ok((attachment, resolved.switched)),
            Err(error) => {
                resolved.undo(mux);
                Err(SwitchError::Runtime(error))
            }
        }
    }
}

/// The saved focus a remembered session's install applies.
fn remembered_focus(memory: Option<NavigationMemory>) -> restore::Focus {
    memory.map_or_else(restore::Focus::default, |memory| restore::Focus {
        active: memory.active,
        history: memory.history,
        panes: memory.panes,
    })
}

/// What stopped a follow-up from showing a session, for its notices.
fn follow_up_error(error: &SwitchError) -> String {
    match error {
        SwitchError::TargetGone => "the session no longer exists".to_owned(),
        SwitchError::SourceGone => "the window has no session".to_owned(),
        SwitchError::Spawn(error) | SwitchError::Runtime(error) => {
            error.to_string()
        }
        SwitchError::Shell(error) => error.clone(),
    }
}

/// The notice for a follow-up that opened a fresh session because it
/// could not show `name`.
fn fresh_session_notice(name: &str, error: &SwitchError) -> NoticeContent {
    NoticeContent::command_failure(
        "Cannot show session",
        format!(
            "Cannot show session \"{name}\": {}. Opened a new session.",
            follow_up_error(error)
        ),
    )
}

/// What a switch that runs on a worker makes the window show.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SwitchKind {
    /// A new session.
    NewSession,
    /// An existing session.
    Session,
    /// A new workspace of the window's session.
    NewWorkspace,
}

/// The notice for a switch that changed nothing.
pub(super) fn switch_failure(
    kind: SwitchKind,
    error: &SwitchError,
) -> Option<NoticeContent> {
    let title = match kind {
        SwitchKind::NewSession => "Cannot create session",
        SwitchKind::Session => "Cannot switch session",
        SwitchKind::NewWorkspace => "Cannot create workspace",
    };
    let message = match error {
        SwitchError::SourceGone => return None,
        SwitchError::TargetGone => "Session no longer exists".to_owned(),
        SwitchError::Spawn(error) | SwitchError::Runtime(error) => {
            format!("{title}: {error}")
        }
        SwitchError::Shell(error) => format!("{title}: {error}"),
    };
    Some(NoticeContent::command_failure(title, message))
}

impl WorkspaceView {
    /// The shell command a switch or session window opens with, at the
    /// active view's requested grid.
    pub(super) fn session_shell(
        &self,
        window: &Window,
        cx: &App,
    ) -> Result<TerminalCommand, CommandError> {
        #[cfg(test)]
        if self.session_shell_fails {
            return Err(CommandError::Runtime(
                "injected shell failure".to_owned(),
            ));
        }
        let mut command = shell_command(
            self.metrics.at_scale(window.scale_factor()),
            &self.config.theme,
            self.config.terminal,
            &self.config.shell_integration,
            self.config.window,
        )
        .map_err(|error| CommandError::Runtime(error.to_string()))?;
        if let Some(view) = self.active_view(cx) {
            command.grid_size = view.read(cx).requested_grid;
        }
        Ok(command)
    }

    /// Refuses a switch while it would race other work: the full
    /// structural guard, a switch or restore in progress, a closing
    /// window, Quit, and teardown.
    pub(super) fn check_switch_available(
        &self,
        cx: &App,
    ) -> Result<(), CommandError> {
        self.check_new_tab_available(cx)?;
        self.check_available(true)?;
        let refusal = if self.switch.is_some() {
            "session switch in progress"
        } else if self.reopen.is_some() {
            "window is still restoring"
        } else if terminating(cx) {
            "runtime is terminating"
        } else if self.record(cx).is_none_or(|record| record.closing) {
            "window is closing"
        } else if self.attachment_id(cx).is_none() {
            "window has no session"
        } else {
            return Ok(());
        };
        Err(CommandError::Unavailable(refusal.to_owned()))
    }

    /// Switches this window to `target`. Showing the current session
    /// changes nothing.
    pub(super) fn switch_session(
        &mut self,
        target: SwitchTarget,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.cancel_reorder(window, cx);
        self.cancel_pane_drag(window, cx);
        self.clear_incoming(true, window, cx);
        self.cancel_divider_drag(cx);
        self.resizing_sidebar = false;
        self.check_switch_available(cx)?;
        if let SwitchTarget::Existing(session) = &target
            && Some(*session) == self.attached_session(cx)
        {
            return Ok(CommandOutcome::Completed);
        }
        let attachment = self.attachment_id(cx).ok_or_else(|| {
            CommandError::Unavailable("window has no session".to_owned())
        })?;
        let remembered = match &target {
            SwitchTarget::Existing(session) => cx
                .global::<Desktop>()
                .windows
                .remembered_workspace(self.window, *session),
            SwitchTarget::New(_) => None,
        };
        let command = self.session_shell(window, cx)?;
        let kind = if matches!(target, SwitchTarget::New(_)) {
            SwitchKind::NewSession
        } else {
            SwitchKind::Session
        };
        self.busy = true;
        self.switch = Some(SwitchPhase::Preparing);
        hold_switch_spawn(cx, self.window);
        checkpoints::set_switching(cx, self.window, true);
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let task = cx.background_executor().spawn(async move {
            #[cfg(all(test, feature = "test-support"))]
            let fail_spawn = runtime
                .worker_hooks
                .enter(worker_hooks::Stage::Switch)
                .await;
            #[cfg(not(all(test, feature = "test-support")))]
            let fail_spawn = false;
            let committed = runtime.switch_session(
                attachment, &target, remembered, &command, fail_spawn,
            );
            #[cfg(all(test, feature = "test-support"))]
            runtime
                .worker_hooks
                .enter(worker_hooks::Stage::Switched)
                .await;
            committed
        });
        let id = self.window;
        let app = cx.to_async();
        cx.spawn_in(window, async move |view, cx| {
            let Committed { result, seq } = task.await;
            let resolution = projected(&app, seq).await;
            let mut result = Some(result);
            let _ = view.update_in(cx, |view, window, cx| {
                if let Some(result) = result.take() {
                    view.finish_switch(kind, result, resolution, window, cx);
                }
            });
            // A window removed before its completion never ended the switch
            // itself. Otherwise the switch holds its spawn until it settles.
            if result.is_some() {
                app.update(|cx| {
                    checkpoints::set_switching(cx, id, false);
                    release_switch_spawn(cx, id);
                });
            }
        })
        .detach();
        cx.notify();
        Ok(CommandOutcome::Accepted)
    }

    /// Ends a switch, settled or not, and releases the Quit that waited
    /// for it.
    pub(super) fn end_switch(&mut self, cx: &mut Context<'_, Self>) {
        self.switch = None;
        self.busy = false;
        checkpoints::set_switching(cx, self.window, false);
        release_switch_spawn(cx, self.window);
    }

    /// Phase 3, or the end of a switch that failed or met teardown.
    pub(super) fn finish_switch(
        &mut self,
        kind: SwitchKind,
        result: Result<Switched, SwitchError>,
        resolution: Resolution,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if resolution == Resolution::Cancelled || terminating(cx) {
            // Teardown owns everything; apply nothing.
            self.end_switch(cx);
            return;
        }
        match result {
            Ok(switched) => {
                self.install_session(switched, None, true, window, cx);
            }
            Err(error) => {
                self.end_switch(cx);
                if let Some(notice) = switch_failure(kind, &error) {
                    self.notify(notice, cx);
                }
                self.reconcile_own(window, cx);
                // The window's own session closed: it takes the existing
                // session-gone path now that the phase cleared.
                if self.attached_session_gone(cx) {
                    self.owe_emptied_close(cx);
                }
                self.resume_close(window, cx);
                cx.notify();
            }
        }
    }

    /// Adopts `switched` and installs its views: remembers where the
    /// window was in the workspace it leaves, takes the target's memory,
    /// and reconciles with removals allowed and the emptied edge
    /// suppressed. `switched` names a session, or another workspace of the
    /// session the window shows. `attachment` replaces the window's own,
    /// as when Close Session took it away. `recovers` says the target was
    /// chosen because it held tabs, so a target left empty or gone at the
    /// settle owes the emptied close again; a workspace the user selected
    /// stays shown empty.
    pub(super) fn install_session(
        &mut self,
        switched: Switched,
        attachment: Option<AttachmentId>,
        recovers: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let desktop = cx.global_mut::<Desktop>();
        if let Some(terminal) = switched.spawned {
            desktop.fresh_terminals.insert(terminal);
        }
        let memory = desktop.windows.switch_to(
            self.window,
            switched.session,
            switched.workspace,
        );
        if let Some(attachment) = attachment {
            desktop.windows.set_attachment(self.window, attachment);
        }
        // A workspace or session the window leaves because it closed has
        // nothing left to return to.
        let state = desktop.hierarchy.state();
        desktop.windows.prune_sessions(state);
        self.switch = Some(SwitchPhase::Installing);
        self.switch_recovers = recovers;
        self.close.emptied = EmptiedClose::None;
        desktop.windows.set_emptied_close(self.window, false);
        self.restore_focus = None;
        self.reopen = Some(remembered_focus(memory));
        if !self.overlay_holds_focus() {
            // The focused view is about to drop.
            self.focus.focus(window, cx);
        }
        self.reconcile(&Touched::everything(), terminating(cx), cx);
        self.measure_tab_widths(window, cx);
        self.apply_reopen(window, cx);
        cx.notify();
    }

    /// Phase 4: the remembered focus applied, so the switch ends and the
    /// closes that waited run. A window whose session or workspace went
    /// takes its emptied close now, and so does one whose target was
    /// chosen for its tabs and holds none now; a workspace the user
    /// selected stays shown empty.
    pub(super) fn settle_switch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.switch != Some(SwitchPhase::Installing) {
            return;
        }
        let recovers = std::mem::take(&mut self.switch_recovers);
        self.end_switch(cx);
        self.reconcile_own(window, cx);
        if self.tabs.is_empty()
            && (self.attached_session_gone(cx)
                || self.shown_workspace_gone(cx)
                || (recovers && self.has_nothing_to_install(cx)))
        {
            self.owe_emptied_close(cx);
        }
        if self.tabs.is_empty() {
            self.focus_empty_state(window, cx);
        }
        self.resume_close(window, cx);
        cx.notify();
    }

    /// Whether this window's reconcile may remove and install while it
    /// stays busy for commands and closes.
    pub(super) fn switch_installing(&self) -> bool {
        self.switch == Some(SwitchPhase::Installing)
    }

    /// The sessions Close Session may move this window to when it closes
    /// the window's own `closing` session, in the order its worker tries
    /// them: the ones it left that still exist, most recent first, then
    /// those no other window or Quake stash shows, most recently viewed
    /// first, as a reopen picks.
    pub(super) fn next_sessions(
        &self,
        closing: SessionId,
        cx: &App,
    ) -> Vec<(SessionId, Option<WorkspaceId>)> {
        let desktop = cx.global::<Desktop>();
        let state = desktop.hierarchy.state();
        let Some(record) = self.record(cx) else {
            return Vec::new();
        };
        let mut candidates: Vec<(SessionId, Option<WorkspaceId>)> = Vec::new();
        // Memory lists workspaces, most recent first: a session's first
        // entry is the workspace the window left last there.
        for memory in &record.memory {
            if memory.session != closing
                && state.session(memory.session).is_some()
                && !candidates
                    .iter()
                    .any(|(listed, _)| *listed == memory.session)
            {
                candidates.push((memory.session, Some(memory.workspace)));
            }
        }
        let stashed = stashed_sessions(desktop);
        let unviewed = desktop.windows.unviewed_sessions(
            state,
            Some(self.window),
            |session| session == closing || stashed.contains(&session),
        );
        for session in unviewed {
            if !candidates.iter().any(|(listed, _)| *listed == session) {
                candidates.push((session, None));
            }
        }
        candidates
    }

    /// As the commit of `target` with `effect` starts: whether it closes
    /// this window's own session or workspace, and then what an own-session
    /// close's worker needs to attach the window elsewhere right after it.
    /// A shell command that cannot be built fails only the attempts that
    /// need a shell. Such a close, an own-workspace close, and an emptied
    /// close that leaves its workspace take the window's switch hold and
    /// defer checkpoints here, until the follow-up settles.
    pub(super) fn commit_follow(
        &mut self,
        target: &CloseTarget,
        effect: &CloseEffect,
        window: &Window,
        cx: &mut App,
    ) -> (bool, Option<CloseFollow>) {
        match target {
            CloseTarget::Session(session) if self.closes_own(target, cx) => {
                self.hold_for_close_commit(cx);
                let follow = CloseFollow {
                    closing: *session,
                    candidates: self.next_sessions(*session, cx),
                    shell: self
                        .session_shell(window, cx)
                        .map_err(|error| error.to_string()),
                };
                (true, Some(follow))
            }
            CloseTarget::Workspace(_) if self.closes_own(target, cx) => {
                self.hold_for_close_commit(cx);
                (true, None)
            }
            CloseTarget::Emptied
                if matches!(effect, CloseEffect::LeaveWorkspace { .. }) =>
            {
                self.hold_for_close_commit(cx);
                (false, None)
            }
            _ => (false, None),
        }
    }

    /// Whether the close pipeline takes `target` at all: an own-session
    /// Close Session or own-workspace Close Workspace is dropped while a
    /// Quit is under way, with what it held, as when a stale commit retries
    /// or a check merges into it. A Quit marks the application quitting
    /// from its request on.
    pub(super) fn admit_close(
        &mut self,
        target: &CloseTarget,
        cx: &mut App,
    ) -> bool {
        if self.quit_refuses(target, cx) {
            self.release_close_hold(cx);
            self.release_exit_close_if_idle(cx);
            return false;
        }
        if matches!(target, CloseTarget::Application) {
            cx.global_mut::<Desktop>().quitting = true;
        }
        true
    }

    /// Refuses Close Session of this window's own session while a Quit is
    /// requested or runs: its follow-up moves the window to another
    /// session, which would race the Quit as a switch would.
    pub(super) fn check_close_session_available(
        &self,
        session: SessionId,
        cx: &App,
    ) -> Result<(), CommandError> {
        if quit_underway(cx) && Some(session) == self.attached_session(cx) {
            Err(CommandError::Unavailable(
                "application is quitting".to_owned(),
            ))
        } else {
            Ok(())
        }
    }

    /// Refuses Close Workspace of this window's own workspace while a Quit
    /// is requested or runs, as for its own session.
    pub(super) fn check_close_workspace_available(
        &self,
        workspace: WorkspaceId,
        cx: &App,
    ) -> Result<(), CommandError> {
        if quit_underway(cx) && Some(workspace) == self.workspace_id(cx) {
            Err(CommandError::Unavailable(
                "application is quitting".to_owned(),
            ))
        } else {
            Ok(())
        }
    }

    /// Whether `target` is a Close Session of this window's own session or
    /// a Close Workspace of the workspace it shows.
    pub(super) fn closes_own(&self, target: &CloseTarget, cx: &App) -> bool {
        match target {
            CloseTarget::Session(session) => {
                Some(*session) == self.attached_session(cx)
            }
            CloseTarget::Workspace(workspace) => {
                Some(*workspace) == self.workspace_id(cx)
            }
            _ => false,
        }
    }

    /// Whether `target` is an own-session or own-workspace close that a
    /// Quit under way refuses, as it refuses the command: such a close
    /// could move the window elsewhere while the Quit captures it.
    pub(super) fn quit_refuses(&self, target: &CloseTarget, cx: &App) -> bool {
        quit_underway(cx) && self.closes_own(target, cx)
    }

    /// Takes this window's switch hold as an own-session Close Session
    /// starts committing: its follow-up restores remembered navigation,
    /// which a Quit must wait for, and until it installs, the window still
    /// names the closed session, which a checkpoint must not save. The
    /// hold and the checkpoint deferral last until the follow-up settles;
    /// a close that has not started committing holds nothing, since a
    /// Quit cancels it.
    pub(super) fn hold_for_close_commit(&mut self, cx: &mut App) {
        if !self.close_hold {
            self.close_hold = true;
            hold_switch_spawn(cx, self.window);
            checkpoints::set_switching(cx, self.window, true);
        }
    }

    /// Ends the close pipeline's switch hold and checkpoint deferral, if it
    /// has them, for a close that ends without a follow-up install.
    pub(super) fn release_close_hold(&mut self, cx: &mut App) {
        if std::mem::take(&mut self.close_hold) {
            checkpoints::set_switching(cx, self.window, false);
            release_switch_spawn(cx, self.window);
        }
    }

    /// Cancels this window's own-session Close Session or own-workspace
    /// Close Workspace that has not started committing, for a Quit: a
    /// check still assessing, whose answer then finds a moved generation, a
    /// confirmation showing, or one queued behind other work. Its session
    /// or workspace was never closed, so the Quit saves it. A committing
    /// close keeps its hold, which the Quit waits for. Queued closes of
    /// other targets stay.
    pub(super) fn cancel_own_session_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let own = |target: &Option<CloseTarget>| {
            target.as_ref().is_some_and(|target| {
                matches!(
                    target,
                    CloseTarget::Session(_) | CloseTarget::Workspace(_)
                ) && self.closes_own(target, cx)
            })
        };
        let pending = own(&self.close.pending);
        let assessing = !self.close_hold && own(&self.close.current);
        let confirming = own(&self.close.confirmation);
        if pending {
            self.close.pending = None;
        }
        if assessing || confirming {
            self.close.dismiss_confirmation();
            if assessing {
                self.busy = false;
            }
        }
        if !(pending || assessing || confirming) {
            return;
        }
        self.sync_dialog(window, cx);
        self.release_exit_close_if_idle(cx);
        self.restore_tab_focus(window, cx);
        // The notification resumes other queued closes.
        cx.notify();
    }

    /// Ends an own-session Close Session that reached its commit while a
    /// Quit is under way, as if it were cancelled; queued closes stay.
    pub(super) fn refuse_own_session_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.close.dismiss_confirmation();
        self.busy = false;
        self.release_close_hold(cx);
        self.sync_dialog(window, cx);
        self.release_exit_close_if_idle(cx);
        self.restore_tab_focus(window, cx);
        cx.notify();
    }

    /// Ends a committed session close: a window whose own session closed
    /// installs the session the worker attached it to, whose switch takes
    /// over the close's hold and checkpoint deferral until the install
    /// settles; one the worker could attach nowhere closes by the
    /// last-window rule; any other window, and one whose session survived
    /// a failed commit, carries on.
    pub(super) fn finish_session_close(
        &mut self,
        own: bool,
        followed: Option<FollowUp>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(followed) = followed.filter(|_| own) else {
            self.release_close_hold(cx);
            self.reconcile_own(window, cx);
            self.resume_close(window, cx);
            cx.notify();
            return;
        };
        match followed {
            FollowUp {
                result: Ok((attachment, switched)),
                skipped,
            } => {
                if let Some((name, error)) = skipped {
                    self.notify(fresh_session_notice(&name, &error), cx);
                }
                // The install's `end_switch` releases the hold and the
                // checkpoint deferral.
                self.close_hold = false;
                self.busy = true;
                self.install_session(
                    switched,
                    Some(attachment),
                    true,
                    window,
                    cx,
                );
            }
            // No session was left to follow: the window closes.
            FollowUp {
                result: Err(SwitchError::TargetGone),
                skipped: None,
            } => {
                self.release_close_hold(cx);
                self.close_by_last_window_rule(None, window, cx);
            }
            FollowUp {
                result: Err(error), ..
            } => {
                self.release_close_hold(cx);
                let message = format!(
                    "Cannot open a new session: {}",
                    follow_up_error(&error)
                );
                self.close_by_last_window_rule(Some(message), window, cx);
            }
        }
    }

    /// Closes a window that has nothing left to show: on Linux the last
    /// window quits rather than leave sessions no window shows out of
    /// reach, and a cancelled Quit then shows the session left last, as
    /// for an emptied close; otherwise the window goes, removed when its
    /// session closed and through its emptied close while it is still
    /// attached. `failure` explains why, in this window or, once it is
    /// gone, the next one.
    fn close_by_last_window_rule(
        &mut self,
        failure: Option<String>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.window_close_target(self.close_platform(), cx)
            == CloseTarget::Application
        {
            if let Some(message) = failure {
                self.report_failure("Cannot show a session", message, cx);
            }
            // The Quit takes the place of the emptied close the gone
            // session owed; run first, that close would remove the window.
            self.set_emptied(EmptiedClose::None, cx);
            cx.global_mut::<Desktop>().emptied_quit = Some(self.window);
            cx.defer(request_quit);
        } else {
            if let Some(message) = failure {
                report_deferred_failure_with_global_latch(cx, None, message);
            }
            if self.attached_session_gone(cx) {
                self.remove_detached_window(window, cx);
            } else {
                self.owe_emptied_close(cx);
                self.resume_close(window, cx);
            }
        }
    }

    /// The sessions a last window whose emptied close became a cancelled
    /// Quit moves to: the ones no window shows, most recently viewed
    /// first, as a reopen picks, each with the workspace this window
    /// remembers. Kept apart so the choice can change in one place.
    fn left_sessions(&self, cx: &App) -> Vec<(SessionId, Option<WorkspaceId>)> {
        let desktop = cx.global::<Desktop>();
        let own = self.attached_session(cx);
        let stashed = stashed_sessions(desktop);
        let memory = self.record(cx).map(|record| record.memory.as_slice());
        desktop
            .windows
            .unviewed_sessions(
                desktop.hierarchy.state(),
                Some(self.window),
                |session| Some(session) == own || stashed.contains(&session),
            )
            .into_iter()
            .map(|session| {
                let remembered = memory.and_then(|memory| {
                    memory
                        .iter()
                        .find(|memory| memory.session == session)
                        .map(|memory| memory.workspace)
                });
                (session, remembered)
            })
            .collect()
    }

    /// After a cancelled Quit that this last window's emptied close
    /// became, shows the session the user left last that no window shows,
    /// attached afresh and installed as a switch is, so the window is not
    /// left with nothing to show. A window that shows something again
    /// changes nothing; one with no such session takes its emptied close.
    /// When nothing can be shown, not even a fresh session, the window
    /// says why and closes by the last-window rule.
    pub(super) fn show_left_session(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.tabs.is_empty()
            || self.busy
            || terminating(cx)
            || self.record(cx).is_none_or(|record| record.closing)
            || !(self.attached_session_gone(cx)
                || self.has_nothing_to_install(cx))
        {
            return;
        }
        let candidates = self.left_sessions(cx);
        if candidates.is_empty() {
            self.owe_emptied_close(cx);
            return;
        }
        let command = self
            .session_shell(window, cx)
            .map_err(|error| error.to_string());
        let old = self.attachment_id(cx);
        self.busy = true;
        self.switch = Some(SwitchPhase::Preparing);
        hold_switch_spawn(cx, self.window);
        checkpoints::set_switching(cx, self.window, true);
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let task = cx.background_executor().spawn(async move {
            let command = command.as_ref().map_err(String::as_str);
            follow_up(&runtime, &candidates, command, old, Fresh::Always).await
        });
        let id = self.window;
        let app = cx.to_async();
        cx.spawn_in(window, async move |view, cx| {
            let Committed { result, seq } = task.await;
            let resolution = projected(&app, seq).await;
            let mut result = Some(result);
            let _ = view.update_in(cx, |view, window, cx| {
                let Some(FollowUp { result, skipped }) = result.take() else {
                    return;
                };
                if resolution == Resolution::Cancelled || terminating(cx) {
                    view.end_switch(cx);
                    return;
                }
                match result {
                    Ok((attachment, switched)) => {
                        if let Some((name, error)) = skipped {
                            view.notify(
                                fresh_session_notice(&name, &error),
                                cx,
                            );
                        }
                        view.install_session(
                            switched,
                            Some(attachment),
                            true,
                            window,
                            cx,
                        );
                    }
                    Err(error) => {
                        view.end_switch(cx);
                        let message = format!(
                            "Cannot open a new session: {}",
                            follow_up_error(&error)
                        );
                        view.close_by_last_window_rule(
                            Some(message),
                            window,
                            cx,
                        );
                        cx.notify();
                    }
                }
            });
            if let Some(followed) = result {
                // The window went away first: its removal ended the hold,
                // and the attachment it never took goes.
                app.update(|cx| {
                    drop_followed(Some(&followed), cx);
                    checkpoints::set_switching(cx, id, false);
                    release_switch_spawn(cx, id);
                });
            }
        })
        .detach();
        cx.notify();
    }

    /// Closes this window as Close Window does on `platform`. When the
    /// last window on Linux would strand sessions no window shows, the
    /// close becomes Quit, which runs through [`request_quit`].
    pub(super) fn close_window(
        &mut self,
        platform: Platform,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match self.window_close_target(platform, cx) {
            CloseTarget::Application => cx.defer(request_quit),
            target => self.request_close(target, window, cx),
        }
    }

    /// The close target Close Window takes: Quit on Linux when this is the
    /// last window and sessions no window shows would otherwise stay
    /// running out of reach.
    pub(super) fn window_close_target(
        &self,
        platform: Platform,
        cx: &App,
    ) -> CloseTarget {
        if platform == Platform::Linux && self.last_window_strands(cx) {
            CloseTarget::Application
        } else {
            CloseTarget::Window
        }
    }

    /// The close an owed emptied close takes: on Linux, when this is the
    /// last window and other sessions no window shows would stay running
    /// out of reach, Quit, as Close Window does there; otherwise the
    /// emptied close itself.
    pub(super) fn emptied_close_target(
        &self,
        platform: Platform,
        cx: &App,
    ) -> CloseTarget {
        if platform == Platform::Linux && self.last_window_strands(cx) {
            CloseTarget::Application
        } else {
            CloseTarget::Emptied
        }
    }

    /// At the commit of a Window close: whether it became Quit because this
    /// window would now be the last one and strand sessions no window
    /// shows. Another window's close may have started committing since
    /// this one was requested, so the rule applies again here. The close
    /// ends without committing, and Quit runs through [`request_quit`]. An
    /// emptied close converts only after its assessment
    /// ([`Self::emptied_quit_check`]).
    pub(super) fn close_becomes_quit(
        &mut self,
        target: &CloseTarget,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if *target != CloseTarget::Window
            || self.window_close_target(self.close_platform(), cx)
                != CloseTarget::Application
        {
            return false;
        }
        self.close.dismiss_confirmation();
        self.end_close_as_quit(window, cx);
        true
    }

    /// Whether an emptied close assessed as `effect` must check its ticket
    /// under the Mux lock before turning into Quit: only a close that
    /// removes the window does, and only when the last-window rule asks
    /// for Quit. Leaving a workspace never becomes Quit.
    pub(super) fn emptied_quit_check(
        &self,
        target: &CloseTarget,
        effect: &CloseEffect,
        cx: &App,
    ) -> bool {
        *target == CloseTarget::Emptied
            && closes_window(effect)
            && self.emptied_close_target(self.close_platform(), cx)
                == CloseTarget::Application
    }

    /// Ends an emptied close whose fresh ticket made it Quit: the close is
    /// no longer owed, and a cancelled Quit shows the session left last.
    pub(super) fn emptied_becomes_quit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.set_emptied(EmptiedClose::None, cx);
        cx.global_mut::<Desktop>().emptied_quit = Some(self.window);
        self.end_close_as_quit(window, cx);
    }

    /// Ends an emptied close whose session closed elsewhere, so core can
    /// no longer assess it, the session-gone path: on Linux the last window
    /// quits rather than strand sessions no window shows; otherwise the
    /// window goes without a notice.
    pub(super) fn close_gone_emptied(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.emptied_close_target(self.close_platform(), cx)
            == CloseTarget::Application
        {
            self.close.current = None;
            self.emptied_becomes_quit(window, cx);
        } else {
            self.remove_detached_window(window, cx);
        }
    }

    fn end_close_as_quit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.busy = false;
        self.sync_dialog(window, cx);
        self.release_exit_close_if_idle(cx);
        self.restore_tab_focus(window, cx);
        cx.defer(request_quit);
        cx.notify();
    }

    /// The platform the last-window rule applies; tests may name another.
    #[cfg_attr(
        not(test),
        expect(clippy::unused_self, reason = "only tests name a platform")
    )]
    pub(super) fn close_platform(&self) -> Platform {
        #[cfg(test)]
        if let Some(platform) = self.test_platform {
            return platform;
        }
        Platform::current()
    }

    /// Notes that this window's `target` close, with `effect`, started or
    /// ended committing. Another window's last-window rule counts a window
    /// whose Window close, or emptied close that removes it, is committing
    /// as going; one that only leaves a workspace stays.
    pub(super) fn note_close_commit(
        &self,
        target: &CloseTarget,
        effect: &CloseEffect,
        committing: bool,
        cx: &mut App,
    ) {
        let removes = match target {
            CloseTarget::Window => true,
            CloseTarget::Emptied => closes_window(effect),
            _ => false,
        };
        if removes {
            cx.global_mut::<Desktop>()
                .windows
                .set_close_committing(self.window, committing);
        }
    }

    /// Whether closing this window would leave sessions running that no
    /// window shows, with no other window or Quake shortcut to reach them.
    /// A window whose Window or emptied close is committing counts as gone.
    /// A Quit already requested, queued, or in progress covers every
    /// session, so the close stays a close and the Quit follows it.
    fn last_window_strands(&self, cx: &App) -> bool {
        let desktop = cx.global::<Desktop>();
        let quit_requested = quit_underway(cx)
            || [&self.close.current, &self.close.pending]
                .into_iter()
                .any(|target| *target == Some(CloseTarget::Application));
        if quit_requested
            || quake_windows::keep_alive(cx)
            || desktop.windows.records().any(|record| {
                record.id != self.window
                    && !record.closing
                    && !record.close_committing
            })
        {
            return false;
        }
        let own = self.attached_session(cx);
        desktop
            .windows
            .unviewed_sessions(
                desktop.hierarchy.state(),
                Some(self.window),
                |_| false,
            )
            .into_iter()
            .any(|session| Some(session) != own)
    }
}

/// Whether a committed emptied close with `effect` removes its window: a
/// detach or session close does, leaving a workspace does not.
pub(super) fn closes_window(effect: &CloseEffect) -> bool {
    matches!(effect, CloseEffect::Detach(_) | CloseEffect::Session(_))
}

/// Whether a Quit is requested or runs: it waits for pending spawns, or
/// it assesses, asks, or commits.
pub(super) fn quit_underway(cx: &App) -> bool {
    let desktop = cx.global::<Desktop>();
    desktop.quitting || desktop.quit_pending
}

/// Holds one of `pending_spawns` for `window`'s session switch, or Close
/// Session of its own session, at most one per window, until it settles.
pub(super) fn hold_switch_spawn(cx: &mut App, window: gpui::WindowId) {
    let desktop = cx.global_mut::<Desktop>();
    if desktop.switch_spawns.insert(window) {
        desktop.pending_spawns += 1;
    }
}

/// Releases `window`'s switch hold, if it has one: when its switch settled
/// or failed, and when the window went away.
pub(super) fn release_switch_spawn(cx: &mut App, window: gpui::WindowId) {
    if cx.global_mut::<Desktop>().switch_spawns.remove(&window) {
        settle_session_spawn(cx);
    }
}

/// Detaches the attachment a follow-up made for a window that moved on
/// before its completion, on a worker; the session stays for the windows
/// and menus that show it.
pub(super) fn drop_followed(followed: Option<&FollowUp>, cx: &App) {
    let Some(FollowUp {
        result: Ok((attachment, _)),
        ..
    }) = followed
    else {
        return;
    };
    let attachment = *attachment;
    let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
    cx.background_executor()
        .spawn(async move {
            let _ = runtime.lock().detach_session(attachment);
        })
        .detach();
}

/// Settles one switch or session window in `pending_spawns`, resuming a
/// Quit that waited for it.
pub(super) fn settle_session_spawn(cx: &mut App) {
    let desktop = cx.global_mut::<Desktop>();
    if settle_spawn(&mut desktop.pending_spawns, &mut desktop.quit_pending) {
        cx.defer(|cx| invoke(ids::QUIT).dispatch(cx));
    } else if desktop.pending_spawns == 0 {
        maybe_exit(cx);
    }
}

/// Opens a window attached to the existing `session` without creating
/// one. A window that fails to open detaches only its own attachment, and
/// with `fresh_on_failure` a window on a new session opens instead.
pub(super) fn open_session_window(
    cx: &mut App,
    session: SessionId,
    reporter: Option<WeakEntity<WorkspaceView>>,
    fresh_on_failure: bool,
) {
    let config = cx.global::<Desktop>().config.clone();
    let command = resolve_metrics(&config, cx)
        .map_err(|error| error.to_string())
        .and_then(|(_, metrics, _)| {
            shell_command(
                metrics,
                &config.theme,
                config.terminal,
                &config.shell_integration,
                config.window,
            )
            .map_err(|error| error.to_string())
        });
    let command = match command {
        Ok(command) => command,
        Err(error) => {
            report_deferred_failure(
                cx,
                reporter,
                format!("Cannot open window: {error}"),
            );
            if fresh_on_failure {
                open_window(cx);
            }
            return;
        }
    };
    cx.global_mut::<Desktop>().pending_spawns += 1;
    let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
    let worker = Arc::clone(&runtime);
    let task = cx.background_executor().spawn(async move {
        #[cfg(all(test, feature = "test-support"))]
        let fail = worker
            .worker_hooks
            .enter(worker_hooks::Stage::OpenSession)
            .await;
        #[cfg(not(all(test, feature = "test-support")))]
        let fail = false;
        worker.attach_existing(session, None, &command, fail)
    });
    cx.spawn(async move |cx| {
        let Committed { result, seq } = task.await;
        let resolution = projected(cx, seq).await;
        let attachment =
            result.as_ref().ok().map(|(attachment, _)| *attachment);
        let opened = cx.update(|cx| {
            if resolution == Resolution::Cancelled || terminating(cx) {
                return true;
            }
            match result {
                Ok((attachment, switched)) => {
                    if let Some(terminal) = switched.spawned {
                        cx.global_mut::<Desktop>()
                            .fresh_terminals
                            .insert(terminal);
                    }
                    open_attached(cx, attachment, switched, reporter)
                }
                Err(error) => {
                    if let Some(notice) =
                        switch_failure(SwitchKind::Session, &error)
                    {
                        report_deferred_failure(cx, reporter, notice.message);
                    }
                    false
                }
            }
        });
        if !opened && let Some(attachment) = attachment {
            // Only this window's attachment goes; the session stays for
            // the windows and menus that show it.
            cx.background_executor()
                .spawn(async move {
                    let _ = runtime.lock().detach_session(attachment);
                })
                .await;
        }
        cx.update(|cx| {
            if !opened && fresh_on_failure && !terminating(cx) {
                open_window(cx);
            }
            settle_session_spawn(cx);
        });
    })
    .detach();
}

/// Opens a regular window on `attachment`; returns whether it opened.
fn open_attached(
    cx: &mut App,
    attachment: AttachmentId,
    switched: Switched,
    reporter: Option<WeakEntity<WorkspaceView>>,
) -> bool {
    let before = cx.windows();
    open_window_with_profile(
        cx,
        false,
        None,
        reporter.clone(),
        Some(restore::Reopen {
            attachment,
            session: switched.session,
            workspace: switched.workspace,
            placement: None,
            focus: restore::Focus::default(),
            notices: Vec::new(),
            stashed: None,
            remembered: Vec::new(),
            saved_index: None,
        }),
    );
    let opened = cx.windows().into_iter().any(|handle| {
        !before.contains(&handle)
            && cx
                .global::<Desktop>()
                .windows
                .record(handle.window_id())
                .is_some_and(|record| record.attachment == Some(attachment))
    });
    if !opened {
        report_deferred_failure(
            cx,
            reporter,
            "Cannot open window: the window could not be created".to_owned(),
        );
    }
    opened
}

/// The sessions stashed Quake windows hold.
fn stashed_sessions(desktop: &Desktop) -> Vec<SessionId> {
    desktop
        .quake_stash
        .values()
        .map(|reopen| reopen.session)
        .collect()
}

/// What a reopen with no window open does: a window on the session the
/// user left most recently that no window shows, or a fresh one.
pub(super) fn reopen_window(cx: &mut App) {
    let desktop = cx.global::<Desktop>();
    let stashed = stashed_sessions(desktop);
    let candidate = desktop
        .windows
        .unviewed_sessions(desktop.hierarchy.state(), None, |session| {
            stashed.contains(&session)
        })
        .first()
        .copied();
    match candidate {
        Some(session) => open_session_window(cx, session, None, true),
        None => open_window(cx),
    }
}
