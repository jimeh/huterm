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
//! closed) and moves a window off a session Close Session closes.

use super::model::SessionMemory;
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
    /// Any other runtime refusal, including teardown.
    Runtime(MuxError),
}

/// The session a Close Session of the window's own session moves the
/// window to, its remembered workspace, and the shell it may need.
pub(super) type CloseFollow =
    ((SessionId, Option<WorkspaceId>), TerminalCommand);

/// A Close Session follow-up's attach, with the sequence it read.
pub(super) type Followed =
    Committed<Result<(AttachmentId, Switched), SwitchError>>;

/// Commits a close on its worker, then, when it succeeded and `follow`
/// names one, attaches the window to the session it moves to.
#[cfg_attr(
    not(all(test, feature = "test-support")),
    expect(clippy::unused_async, reason = "only the test hook awaits")
)]
pub(super) async fn commit_close(
    runtime: Arc<DesktopRuntime>,
    assessment: CloseAssessment,
    confirmed: bool,
    windows: Option<huterm_state::DesktopLayout>,
    follow: Option<CloseFollow>,
) -> (Committed<Result<(), MuxError>>, Option<Followed>) {
    #[cfg(all(test, feature = "test-support"))]
    if runtime
        .worker_hooks
        .enter(worker_hooks::Stage::Commit)
        .await
    {
        // A teardown failure after the runtime accepted the close.
        let mut committed = runtime.commit(&assessment, confirmed, windows);
        committed.result = Err(RuntimeError::Stopped.into());
        return (committed, None);
    }
    let committed = runtime.commit(&assessment, confirmed, windows);
    let Some(((session, workspace), command)) =
        follow.filter(|_| committed.result.is_ok())
    else {
        return (committed, None);
    };
    #[cfg(all(test, feature = "test-support"))]
    let fail = runtime
        .worker_hooks
        .enter(worker_hooks::Stage::CloseFollow)
        .await;
    #[cfg(not(all(test, feature = "test-support")))]
    let fail = false;
    let followed = runtime.attach_existing(session, workspace, &command, fail);
    (committed, Some(followed))
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

/// Opens a shell tab running `command` in `workspace`, unless the test
/// hook asked the switch's spawn to fail.
fn open_shell(
    mux: &mut Mux,
    workspace: WorkspaceId,
    command: &TerminalCommand,
    fail: bool,
) -> Result<OpenedTab, MuxError> {
    if fail {
        return Err(RuntimeError::Invariant("injected spawn failure").into());
    }
    mux.open_tab(workspace, command)
}

/// The workspace `session` shows: `remembered` when it still belongs to
/// the session and holds a tab, else the first that holds one. Only when
/// none does, a shell tab opens in the first workspace, creating one when
/// the session has none.
fn resolve_workspace(
    mux: &mut Mux,
    session: SessionId,
    remembered: Option<WorkspaceId>,
    command: &TerminalCommand,
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
            Err(SwitchError::Spawn(error))
        }
    }
}

/// A new session with one workspace and a shell; closed again on failure.
fn create_session(
    mux: &mut Mux,
    name: Option<&str>,
    command: &TerminalCommand,
    fail_spawn: bool,
) -> Result<Switched, SwitchError> {
    let session = mux.create_session(name).map_err(SwitchError::Spawn)?;
    let opened = mux.create_workspace(session, None).and_then(|workspace| {
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
            Err(SwitchError::Spawn(error))
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
                create_session(mux, name.as_deref(), command, fail_spawn)?,
                None,
            ),
            SwitchTarget::Existing(session) => {
                mux.select_session(*session)
                    .map_err(|_| SwitchError::TargetGone)?;
                let resolved = resolve_workspace(
                    mux, *session, remembered, command, fail_spawn,
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
        let result =
            self.attach_locked(&mut mux, session, remembered, command, fail);
        Committed {
            result,
            seq: mux.hierarchy_seq(),
        }
    }

    pub(super) fn attach_locked(
        &self,
        mux: &mut Mux,
        session: SessionId,
        remembered: Option<WorkspaceId>,
        command: &TerminalCommand,
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
fn remembered_focus(memory: Option<SessionMemory>) -> restore::Focus {
    memory.map_or_else(restore::Focus::default, |memory| restore::Focus {
        active: memory.active,
        history: memory.history,
        panes: memory.panes,
    })
}

/// The notice for a switch that changed nothing.
fn switch_failure(new: bool, error: &SwitchError) -> Option<NoticeContent> {
    let title = if new {
        "Cannot create session"
    } else {
        "Cannot switch session"
    };
    let message = match error {
        SwitchError::SourceGone => return None,
        SwitchError::TargetGone => "Session no longer exists".to_owned(),
        SwitchError::Spawn(error) | SwitchError::Runtime(error) => {
            format!("{title}: {error}")
        }
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
            SwitchTarget::Existing(session) => {
                self.record(cx).and_then(|record| {
                    record
                        .memory
                        .iter()
                        .find(|memory| memory.session == *session)
                        .map(|memory| memory.workspace)
                })
            }
            SwitchTarget::New(_) => None,
        };
        let command = self.session_shell(window, cx)?;
        let new = matches!(target, SwitchTarget::New(_));
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
                    view.finish_switch(new, result, resolution, window, cx);
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
    fn end_switch(&mut self, cx: &mut Context<'_, Self>) {
        self.switch = None;
        self.busy = false;
        checkpoints::set_switching(cx, self.window, false);
        release_switch_spawn(cx, self.window);
    }

    /// Phase 3, or the end of a switch that failed or met teardown.
    fn finish_switch(
        &mut self,
        new: bool,
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
                self.install_session(switched, None, window, cx);
            }
            Err(error) => {
                self.end_switch(cx);
                if let Some(notice) = switch_failure(new, &error) {
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
    /// window was in the session it leaves, takes the target's memory, and
    /// reconciles with removals allowed and the emptied edge suppressed.
    /// `attachment` replaces the window's own, as when Close Session took
    /// it away.
    pub(super) fn install_session(
        &mut self,
        switched: Switched,
        attachment: Option<AttachmentId>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let desktop = cx.global_mut::<Desktop>();
        if let Some(terminal) = switched.spawned {
            desktop.fresh_terminals.insert(terminal);
        }
        let memory = desktop.windows.switch_session(
            self.window,
            switched.session,
            switched.workspace,
        );
        if let Some(attachment) = attachment {
            desktop.windows.set_attachment(self.window, attachment);
        }
        // A session the window leaves because it closed has nothing left
        // to return to.
        let state = desktop.hierarchy.state();
        desktop.windows.prune_sessions(state);
        self.switch = Some(SwitchPhase::Installing);
        self.close.emptied = EmptiedClose::None;
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
    /// closes that waited run. A window left with nothing to show takes its
    /// emptied close now.
    pub(super) fn settle_switch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.switch != Some(SwitchPhase::Installing) {
            return;
        }
        self.end_switch(cx);
        self.reconcile_own(window, cx);
        if self.tabs.is_empty()
            && (self.attached_session_gone(cx)
                || self.has_nothing_to_install(cx))
        {
            self.owe_emptied_close(cx);
        }
        self.resume_close(window, cx);
        cx.notify();
    }

    /// Whether this window's reconcile may remove and install while it
    /// stays busy for commands and closes.
    pub(super) fn switch_installing(&self) -> bool {
        self.switch == Some(SwitchPhase::Installing)
    }

    /// The session Close Session moves this window to when it closes the
    /// window's own `closing` session: the most recently left one that
    /// still exists, else the session viewed most recently that no other
    /// window or Quake stash shows, as a reopen picks.
    pub(super) fn next_session(
        &self,
        closing: SessionId,
        cx: &App,
    ) -> Option<(SessionId, Option<WorkspaceId>)> {
        let desktop = cx.global::<Desktop>();
        let state = desktop.hierarchy.state();
        let remembered = self.record(cx)?.memory.iter().find(|memory| {
            memory.session != closing && state.session(memory.session).is_some()
        });
        if let Some(memory) = remembered {
            return Some((memory.session, Some(memory.workspace)));
        }
        let stashed = stashed_sessions(desktop);
        desktop
            .windows
            .unviewed_sessions(state, Some(self.window), |session| {
                session == closing || stashed.contains(&session)
            })
            .first()
            .map(|session| (*session, None))
    }

    /// Whether `target` closes this window's own session, and then the
    /// session the commit's worker attaches the window to right after it,
    /// with the shell it may need: the most recently left one that still
    /// exists. A follow-up holds a spawn in `pending_spawns` until its
    /// install settles, so Quit waits for the navigation it restores.
    pub(super) fn close_follow(
        &self,
        target: &CloseTarget,
        window: &Window,
        cx: &mut App,
    ) -> (bool, Option<CloseFollow>) {
        let CloseTarget::Session(session) = target else {
            return (false, None);
        };
        if Some(*session) != self.attached_session(cx) {
            return (false, None);
        }
        let follow = self.next_session(*session, cx).and_then(|next| {
            Some((next, self.session_shell(window, cx).ok()?))
        });
        if follow.is_some() {
            hold_switch_spawn(cx, self.window);
        }
        (true, follow)
    }

    /// Ends a committed session close: a window whose own session closed
    /// installs the session the worker attached it to, keeping its switch
    /// hold until the install settles, or closes when it had none left or
    /// the attach failed; any other window carries on.
    pub(super) fn finish_session_close(
        &mut self,
        own: bool,
        followed: Option<&Result<(AttachmentId, Switched), SwitchError>>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(Ok((attachment, switched))) = followed {
            self.busy = true;
            checkpoints::set_switching(cx, self.window, true);
            self.install_session(*switched, Some(*attachment), window, cx);
            return;
        }
        release_switch_spawn(cx, self.window);
        if !own {
            self.reconcile_own(window, cx);
            self.resume_close(window, cx);
            cx.notify();
        } else if self.window_close_target(Platform::current(), cx)
            == CloseTarget::Application
        {
            // On Linux the last window quits rather than leave sessions no
            // window shows out of reach; a cancelled Quit keeps it open.
            self.request_close(CloseTarget::Application, window, cx);
        } else {
            self.remove_detached_window(window, cx);
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
    /// out of reach, Quit hosted by this window, as Close Window does
    /// there; otherwise the emptied close itself.
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

    /// Whether closing this window would leave sessions running that no
    /// window shows, with no other window or Quake shortcut to reach them.
    fn last_window_strands(&self, cx: &App) -> bool {
        let desktop = cx.global::<Desktop>();
        if quake_windows::keep_alive(cx)
            || desktop
                .windows
                .records()
                .any(|record| record.id != self.window && !record.closing)
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

/// Holds one of `pending_spawns` for `window`'s session switch or Close
/// Session follow-up, at most one per window, until it settles.
pub(super) fn hold_switch_spawn(cx: &mut App, window: gpui::WindowId) {
    let desktop = cx.global_mut::<Desktop>();
    if desktop.switch_spawns.insert(window) {
        desktop.pending_spawns += 1;
    }
}

/// Ends the switch hold of a Close Session follow-up once its completion
/// ran: an install still in progress keeps it until `end_switch`; one that
/// never started, because the close failed, moved on, or the window went
/// away, releases it now.
pub(super) fn settle_follow(cx: &mut App, window: gpui::WindowId) {
    if !cx
        .global::<Desktop>()
        .checkpoints
        .switching
        .contains(&window)
    {
        release_switch_spawn(cx, window);
    }
}

/// Releases `window`'s switch hold, if it has one: when its switch settled
/// or failed, and when the window went away.
pub(super) fn release_switch_spawn(cx: &mut App, window: gpui::WindowId) {
    if cx.global_mut::<Desktop>().switch_spawns.remove(&window) {
        settle_session_spawn(cx);
    }
}

/// Detaches the attachment a Close Session follow-up made for a window
/// that moved on before its completion, on a worker; the session stays for
/// the windows and menus that show it.
pub(super) fn drop_followed(
    followed: Option<&Result<(AttachmentId, Switched), SwitchError>>,
    cx: &App,
) {
    let Some(Ok((attachment, _))) = followed else {
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
                    if let Some(notice) = switch_failure(false, &error) {
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
