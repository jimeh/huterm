//! Workspace switching: a window shows another workspace of its session,
//! creates one, and moves on when the workspace it shows empties or goes.
//!
//! A workspace switch runs the session switch's phases without the worker
//! step, since nothing changes in core: the guard, the install, which files
//! the left workspace's navigation memory and reconciles with removals
//! allowed, and the settle once installs are idle. It holds
//! `pending_spawns` and defers checkpoints from install to settle. New
//! Workspace adds a worker that creates the workspace and its shell under
//! one Mux lock, closing the workspace again when the shell fails.
//!
//! When the shown workspace empties or goes, core answers the window's
//! emptied close with `LeaveWorkspace`, and an own-workspace Close
//! Workspace answers itself: the window then follows to the workspace it
//! remembers, else the first in session order that holds tabs, else the
//! first (the empty state), on its existing attachment.

use super::session_switch::{
    SwitchError, SwitchKind, SwitchPhase, Switched, hold_switch_spawn,
};
#[cfg(all(test, feature = "test-support"))]
use super::worker_hooks;
use super::{
    App, Arc, AttachmentId, CommandError, CommandInvocation, CommandOutcome,
    Committed, Context, Desktop, DesktopRuntime, HierarchyState, Ordering,
    RuntimeError, SessionId, TerminalCommand, Window, WorkspaceId,
    WorkspaceView, checkpoints, projected, session_switch, terminating,
};

/// The workspace a window that leaves `left` follows to, and whether it was
/// chosen because it held tabs: the window's navigation memory first, then
/// session order, the first that holds tabs, else the session's first.
pub(super) fn follow_target(
    state: &HierarchyState,
    session: SessionId,
    left: Option<WorkspaceId>,
    remembered: impl IntoIterator<Item = WorkspaceId>,
) -> Option<(WorkspaceId, bool)> {
    let order = state.session_workspaces(session).unwrap_or_default();
    let candidate = |workspace: &WorkspaceId| {
        Some(*workspace) != left && order.contains(workspace)
    };
    let holds_tabs = |workspace: &WorkspaceId| {
        state
            .workspace_tabs(*workspace)
            .is_some_and(|tabs| !tabs.is_empty())
    };
    remembered
        .into_iter()
        .filter(candidate)
        .chain(order.iter().copied().filter(candidate))
        .find(holds_tabs)
        .map(|workspace| (workspace, true))
        .or_else(|| {
            order
                .iter()
                .copied()
                .find(candidate)
                .map(|workspace| (workspace, false))
        })
}

/// The workspace `steps` places from `current` in session order, wrapping;
/// `None` with fewer than two workspaces.
pub(super) fn cycle_target(
    order: &[WorkspaceId],
    current: Option<WorkspaceId>,
    forward: bool,
) -> Option<WorkspaceId> {
    if order.len() < 2 {
        return None;
    }
    let at = current
        .and_then(|current| order.iter().position(|id| *id == current))
        .unwrap_or(0);
    let next = if forward {
        (at + 1) % order.len()
    } else {
        (at + order.len() - 1) % order.len()
    };
    order.get(next).copied()
}

impl DesktopRuntime {
    /// Creates a workspace in the session of `attachment` with one shell
    /// running `command`, under one lock; a failed shell closes the
    /// workspace again, so nothing is left behind.
    fn create_workspace(
        &self,
        attachment: AttachmentId,
        name: Option<&str>,
        command: &TerminalCommand,
    ) -> Committed<Result<Switched, SwitchError>> {
        let mut mux = self.lock();
        let result = if self.terminating.load(Ordering::Acquire) {
            Err(SwitchError::Runtime(RuntimeError::Stopped.into()))
        } else {
            mux.attachment_session(attachment)
                .map_err(|_| SwitchError::SourceGone)
                .and_then(|session| {
                    mux.create_workspace_with_tab(session, name, command)
                        .map(|(workspace, opened)| Switched {
                            session,
                            workspace,
                            spawned: Some(opened.tab.first_pane().terminal_id),
                        })
                        .map_err(SwitchError::Spawn)
                })
        };
        Committed {
            result,
            seq: mux.hierarchy_seq(),
        }
    }
}

impl WorkspaceView {
    /// The workspaces of this window's session in session order.
    pub(super) fn session_workspaces(&self, cx: &App) -> Vec<WorkspaceId> {
        self.attached_session(cx)
            .and_then(|session| {
                cx.global::<Desktop>()
                    .hierarchy
                    .state()
                    .session_workspaces(session)
            })
            .map(<[WorkspaceId]>::to_vec)
            .unwrap_or_default()
    }

    /// Whether the projection no longer holds the window's workspace in
    /// the session the window attaches to, while it holds the session: the
    /// workspace closed or moved to another session.
    pub(super) fn shown_workspace_gone(&self, cx: &App) -> bool {
        let Some((session, workspace)) =
            self.attached_session(cx).zip(self.workspace_id(cx))
        else {
            return false;
        };
        let state = cx.global::<Desktop>().hierarchy.state();
        state.session(session).is_some()
            && state.workspace_session(workspace) != Some(session)
    }

    /// Refuses the session's next and previous workspace with fewer than
    /// two workspaces, on top of the switch guard.
    pub(super) fn check_cycle_available(
        &self,
        cx: &App,
    ) -> Result<(), CommandError> {
        self.check_switch_available(cx)?;
        if self.session_workspaces(cx).len() < 2 {
            return Err(CommandError::Unavailable(
                "session has one workspace".to_owned(),
            ));
        }
        Ok(())
    }

    /// Switches this window to `workspace` of its session: the switch
    /// guard, then the install and settle a session switch runs. Showing
    /// the current workspace changes nothing; a workspace that is gone or
    /// belongs to another session is refused.
    pub(super) fn switch_workspace(
        &mut self,
        workspace: WorkspaceId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.cancel_reorder(window, cx);
        self.cancel_pane_drag(window, cx);
        self.clear_incoming(true, window, cx);
        self.cancel_divider_drag(cx);
        self.resizing_sidebar = false;
        self.check_switch_available(cx)?;
        if Some(workspace) == self.workspace_id(cx) {
            return Ok(CommandOutcome::Completed);
        }
        let session = self.attached_session(cx).ok_or_else(|| {
            CommandError::Unavailable("window has no session".to_owned())
        })?;
        if cx
            .global::<Desktop>()
            .hierarchy
            .state()
            .workspace_session(workspace)
            != Some(session)
        {
            return Err(CommandError::Unavailable(
                "Workspace no longer exists".to_owned(),
            ));
        }
        self.busy = true;
        self.switch = Some(SwitchPhase::Installing);
        hold_switch_spawn(cx, self.window);
        checkpoints::set_switching(cx, self.window, true);
        let switched = Switched {
            session,
            workspace,
            spawned: None,
        };
        self.install_session(switched, None, false, window, cx);
        Ok(CommandOutcome::Accepted)
    }

    /// Switches to the next or previous workspace of the session, wrapping.
    pub(super) fn cycle_workspace(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.check_cycle_available(cx)?;
        let target = cycle_target(
            &self.session_workspaces(cx),
            self.workspace_id(cx),
            forward,
        )
        .ok_or_else(|| {
            CommandError::Unavailable("session has one workspace".to_owned())
        })?;
        self.switch_workspace(target, window, cx)
    }

    /// Creates a workspace with one shell in this window's session and
    /// switches the window to it. It counts as a shell-producing switch:
    /// Quit waits for it from the guard to the settle.
    pub(super) fn new_workspace(
        &mut self,
        name: Option<String>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.cancel_reorder(window, cx);
        self.cancel_pane_drag(window, cx);
        self.clear_incoming(true, window, cx);
        self.cancel_divider_drag(cx);
        self.resizing_sidebar = false;
        self.check_switch_available(cx)?;
        let attachment = self.attachment_id(cx).ok_or_else(|| {
            CommandError::Unavailable("window has no session".to_owned())
        })?;
        #[cfg_attr(
            not(all(test, feature = "test-support")),
            expect(unused_mut, reason = "only the test hook fails the shell")
        )]
        let mut command = self.session_shell(window, cx)?;
        self.busy = true;
        self.switch = Some(SwitchPhase::Preparing);
        hold_switch_spawn(cx, self.window);
        checkpoints::set_switching(cx, self.window, true);
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let task = cx.background_executor().spawn(async move {
            #[cfg(all(test, feature = "test-support"))]
            if runtime
                .worker_hooks
                .enter(worker_hooks::Stage::Switch)
                .await
            {
                // A shell that cannot start, so core closes the workspace
                // it created for it.
                command.program = "/huterm-injected-missing-shell".into();
            }
            let committed =
                runtime.create_workspace(attachment, name.as_deref(), &command);
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
                    view.finish_switch(
                        SwitchKind::NewWorkspace,
                        result,
                        resolution,
                        window,
                        cx,
                    );
                }
            });
            // A window removed before its completion never ended the switch
            // itself.
            if result.is_some() {
                app.update(|cx| {
                    checkpoints::set_switching(cx, id, false);
                    session_switch::release_switch_spawn(cx, id);
                });
            }
        })
        .detach();
        cx.notify();
        Ok(CommandOutcome::Accepted)
    }

    /// Moves this window off the workspace it shows, after a committed
    /// `LeaveWorkspace` or own-workspace close, on its existing attachment:
    /// to [`follow_target`], keeping the commit's switch hold until the
    /// install settles. A window with nowhere to go releases the hold and
    /// takes its emptied close.
    pub(super) fn follow_left_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let left = self.workspace_id(cx);
        let target = self.attached_session(cx).and_then(|session| {
            let desktop = cx.global::<Desktop>();
            let remembered: Vec<WorkspaceId> = desktop
                .windows
                .record(self.window)
                .map(|record| {
                    record
                        .memory
                        .iter()
                        .filter(|memory| memory.session == session)
                        .map(|memory| memory.workspace)
                        .collect()
                })
                .unwrap_or_default();
            follow_target(desktop.hierarchy.state(), session, left, remembered)
                .map(|target| (session, target))
        });
        let Some((session, (workspace, recovers))) =
            target.filter(|_| !terminating(cx))
        else {
            self.release_close_hold(cx);
            self.reconcile_own(window, cx);
            if self.tabs.is_empty() {
                self.owe_emptied_close(cx);
            }
            self.resume_close(window, cx);
            cx.notify();
            return;
        };
        // The install's `end_switch` releases the hold and the checkpoint
        // deferral.
        self.close_hold = false;
        hold_switch_spawn(cx, self.window);
        checkpoints::set_switching(cx, self.window, true);
        self.busy = true;
        let switched = Switched {
            session,
            workspace,
            spawned: None,
        };
        self.install_session(switched, None, recovers, window, cx);
    }

    /// Ends a committed Close Workspace: a window whose own workspace went
    /// follows directly, even when the commit reported a teardown failure
    /// after core removed it; one whose workspace survived, and any other
    /// window, carries on.
    pub(super) fn finish_workspace_close(
        &mut self,
        own: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if own && self.shown_workspace_gone(cx) {
            self.follow_left_workspace(window, cx);
            return;
        }
        self.release_close_hold(cx);
        self.reconcile_own(window, cx);
        self.resume_close(window, cx);
        cx.notify();
    }

    /// The Close Workspace target the command names, or the window's own.
    pub(super) fn close_workspace_target(
        &self,
        invocation: Option<&CommandInvocation>,
        cx: &App,
    ) -> Result<WorkspaceId, CommandError> {
        let workspace = invocation
            .and_then(|invocation| invocation.workspace("workspace"))
            .or_else(|| self.workspace_id(cx))
            .ok_or_else(|| {
                CommandError::Unavailable("window has no workspace".to_owned())
            })?;
        let state = cx.global::<Desktop>().hierarchy.state();
        let session = state.workspace_session(workspace).ok_or_else(|| {
            CommandError::Unavailable("Workspace no longer exists".to_owned())
        })?;
        if state
            .session_workspaces(session)
            .is_none_or(|workspaces| workspaces.len() < 2)
        {
            return Err(CommandError::Unavailable(
                "a session's only workspace closes with the session".to_owned(),
            ));
        }
        self.check_close_workspace_available(workspace, cx)?;
        Ok(workspace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use huterm_protocol::{
        RuntimeId, TabId, TabInfo, TerminalId, WorkspaceInfo,
    };

    #[test]
    fn cycling_wraps_in_session_order_and_needs_two_workspaces() {
        let runtime = RuntimeId::new(1);
        let ids: Vec<WorkspaceId> = (1..=3)
            .map(|n| WorkspaceId::in_runtime(runtime, n))
            .collect();
        assert_eq!(cycle_target(&ids, Some(ids[2]), true), Some(ids[0]));
        assert_eq!(cycle_target(&ids, Some(ids[0]), false), Some(ids[2]));
        assert_eq!(cycle_target(&ids, Some(ids[0]), true), Some(ids[1]));
        assert_eq!(cycle_target(&ids[..1], Some(ids[0]), true), None);
    }

    #[test]
    fn a_window_follows_memory_then_session_order_to_tabs_else_the_first() {
        let runtime = RuntimeId::new(1);
        let session = SessionId::in_runtime(runtime, 1);
        let w = |n| WorkspaceId::in_runtime(runtime, n);
        let workspace = |n: u64| WorkspaceInfo {
            id: w(n),
            custom_name: None,
            automatic_name: format!("Workspace {n}"),
        };
        let tab = |n: u64| TabInfo {
            id: TabId::in_runtime(runtime, n),
            custom_name: None,
            layout: huterm_protocol::PaneLayout::single(
                huterm_protocol::PaneInfo {
                    id: huterm_protocol::PaneId::in_runtime(runtime, n + 100),
                    terminal_id: TerminalId::new(n + 200),
                    fallback_name: "sh".to_owned(),
                    custom_name: None,
                },
            ),
            zoomed: None,
            input_sync: false,
        };
        let state = |populated: &[u64]| {
            HierarchyState::from_sessions(
                huterm_protocol::StreamId::new(runtime),
                1,
                [(
                    huterm_protocol::SessionInfo {
                        id: session,
                        custom_name: None,
                        automatic_name: "Session 1".to_owned(),
                    },
                    (1..=4).map(move |n| {
                        let tabs: Vec<TabInfo> = if populated.contains(&n) {
                            vec![tab(n)]
                        } else {
                            Vec::new()
                        };
                        (workspace(n), tabs.into_iter())
                    }),
                )],
                std::iter::empty(),
            )
            .unwrap()
        };
        // Memory wins over session order when it holds tabs.
        assert_eq!(
            follow_target(&state(&[2, 4]), session, Some(w(1)), [w(4), w(3)]),
            Some((w(4), true))
        );
        // A remembered empty workspace yields to one with tabs.
        assert_eq!(
            follow_target(&state(&[2]), session, Some(w(1)), [w(3)]),
            Some((w(2), true))
        );
        // With no tabs anywhere, the first workspace other than the left.
        assert_eq!(
            follow_target(&state(&[]), session, Some(w(1)), [w(3)]),
            Some((w(2), false))
        );
        // A workspace the window left is never the target.
        assert_eq!(
            follow_target(&state(&[1]), session, Some(w(1)), []),
            Some((w(2), false))
        );
    }
}
