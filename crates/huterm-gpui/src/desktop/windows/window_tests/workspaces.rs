//! Tier 2 tests for workspace switching and lifecycle (#27 PR A): a window
//! shows another workspace of its session and returns to its own
//! navigation there, New Workspace and Close Workspace, the window that
//! leaves a workspace that empties or goes (L1), the empty state, and the
//! Quit, close, and switch interleavings W1 to W31 and W34 to W38 of
//! `docs/plans/workspace-panels.md`. Ordering comes from worker hooks and
//! sequence waits, never sleeps.

use super::super::empty_state::EmptyStateModel;
use super::super::session_switch::quit_underway;
use super::pane_moves::active;
use super::sessions::{
    confirm_quit, confirmation, installing, shown_session, shown_tabs,
    switching, undo_label, wait_quit_dialog,
};
use super::worker_hooks::Stage;
use super::*;

impl DesktopHarness {
    /// Appends a workspace to the harness session with one tab running
    /// `command` per count, as another client would.
    pub(super) fn add_workspace_running(
        &self,
        command: &str,
        tabs: usize,
    ) -> (WorkspaceId, Vec<TabId>) {
        let mut mux = self.runtime.lock();
        let workspace = mux.create_workspace(self.session, None).unwrap();
        let tabs = (0..tabs)
            .map(|_| mux.open_tab(workspace, &fixture(command)).unwrap().tab.id)
            .collect();
        (workspace, tabs)
    }

    /// [`Self::add_workspace_running`] with echo tabs.
    pub(super) fn add_workspace(
        &self,
        tabs: usize,
    ) -> (WorkspaceId, Vec<TabId>) {
        self.add_workspace_running(ECHO, tabs)
    }

    /// Closes `tab` of `workspace` through Mux, as another client would.
    fn close_tab_in(&self, workspace: WorkspaceId, tab: TabId) {
        self.runtime.lock().close_tab(workspace, tab).unwrap();
    }

    fn select_workspace(
        &mut self,
        window: AnyWindowHandle,
        workspace: WorkspaceId,
    ) -> Result<CommandOutcome, CommandError> {
        self.invoke_with(
            window,
            &workspace_pick(ids::SELECT_WORKSPACE, workspace),
        )
    }

    /// Switches `window` to `workspace` and waits until the switch settled
    /// showing exactly `tabs`.
    pub(super) fn show_workspace(
        &mut self,
        window: AnyWindowHandle,
        workspace: WorkspaceId,
        tabs: &[TabId],
    ) {
        self.wait_projection();
        self.select_workspace(window, workspace).unwrap();
        self.wait_workspace(window, workspace, tabs);
    }

    /// Waits until `window` settled on `workspace` showing exactly `tabs`.
    pub(super) fn wait_workspace(
        &mut self,
        window: AnyWindowHandle,
        workspace: WorkspaceId,
        tabs: &[TabId],
    ) {
        self.wait_for("the window to settle on the workspace", |cx| {
            shown_workspace(cx, window) == Some(workspace)
                && !switching(cx, window)
                && !busy(cx, window)
                && shown_tabs(cx, window) == tabs
        });
    }

    fn workspace_exists(&self, workspace: WorkspaceId) -> bool {
        self.runtime.lock().workspace(workspace).is_some()
    }

    fn workspaces(&self) -> Vec<WorkspaceId> {
        self.runtime
            .lock()
            .session(self.session)
            .map(|session| session.workspaces.clone())
            .unwrap_or_default()
    }

    fn pending_spawns(&mut self) -> usize {
        self.cx.update(|cx| cx.global::<Desktop>().pending_spawns)
    }
}

fn workspace_pick(
    command: huterm_protocol::CommandId,
    workspace: WorkspaceId,
) -> CommandInvocation {
    CommandInvocation::new(
        command,
        vec![CommandArgument::new(
            "workspace",
            CommandValue::Workspace(workspace),
        )],
    )
}

/// The workspace `window`'s record shows.
pub(super) fn shown_workspace(
    cx: &mut App,
    window: AnyWindowHandle,
) -> Option<WorkspaceId> {
    cx.global::<Desktop>()
        .windows
        .record(window.window_id())
        .and_then(|record| record.workspace)
}

/// The attachment `window`'s record holds.
fn attachment_of(
    cx: &mut App,
    window: AnyWindowHandle,
) -> Option<AttachmentId> {
    cx.global::<Desktop>()
        .windows
        .record(window.window_id())
        .and_then(|record| record.attachment)
}

/// The pane `window` focuses in its view of `tab`, whether or not the
/// window is active.
fn focused_pane(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
) -> Option<PaneId> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx)
                .tabs
                .iter()
                .find(|view| view.id == tab)
                .map(|view| view.focused().pane)
        })
        .ok()
        .flatten()
}

/// The empty state `window` shows, if any.
fn empty_state(
    cx: &mut App,
    window: AnyWindowHandle,
) -> Option<EmptyStateModel> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx).empty_state_model(cx)
        })
        .ok()
        .flatten()
}

fn quitting(cx: &App) -> bool {
    cx.global::<Desktop>().quitting
}

#[gpui::test]
fn switching_workspaces_keeps_each_workspaces_shells_navigation_and_undo(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let split = harness.open_tab(ECHO);
    let left = harness.first_pane(split);
    let right = harness.split(split, left, SplitDirection::Right, ECHO);
    let window = harness.attach_showing(&[first, split]);
    harness.answers(window, first, "a1");
    harness.focus(window, split, right);
    // A pane swap the window can undo, in this workspace only.
    harness.invoke(window, ids::SWAP_PANE_NEXT).unwrap();
    harness.wait_for("the swap to be undoable", |cx| {
        undo_label(cx, window).is_some()
    });
    let (second, others) = harness.add_workspace(2);

    harness.show_workspace(window, second, &others);
    assert!(
        harness.cx.update(|cx| undo_label(cx, window)).is_none(),
        "undo reached the workspace the window left"
    );
    harness.answers(window, others[1], "b2");
    // A switch closes nothing: both workspaces keep their shells.
    assert_eq!(harness.workspaces(), [harness.workspace, second]);

    harness.show_workspace(window, harness.workspace, &[first, split]);
    harness.cx.update(|cx| {
        assert_eq!(active(cx, window), Some(split));
        assert_eq!(focused_pane(cx, window, split), Some(right));
        let record = cx.global::<Desktop>().windows.record(window.window_id());
        assert_eq!(record.unwrap().history.first(), Some(&split));
        assert_eq!(undo_label(cx, window).as_deref(), Some("Swap Panes"));
    });
    harness.answers(window, first, "a1-again");

    // The other workspace comes back on the tab the window left it on.
    harness.show_workspace(window, second, &others);
    harness
        .cx
        .update(|cx| assert_eq!(active(cx, window), Some(others[1])));
    harness.answers(window, others[1], "b2-again");

    // A session switch returns to the workspace the window left last in
    // that session.
    let other = harness.other_session("other", 1);
    harness.switch_to(window, other.session, &other.tabs);
    harness.switch_to(window, harness.session, &others);
    harness.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, window), Some(second));
        assert_eq!(active(cx, window), Some(others[1]));
    });
}

#[gpui::test]
fn select_workspace_by_index_shows_that_position_or_the_last_and_skips_an_empty_slot(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let (second, others) = harness.add_workspace(1);
    // Another session's workspace never counts toward a position.
    harness.other_session("other", 1);
    harness.wait_projection();
    let at = |index| {
        CommandInvocation::new(
            ids::SELECT_WORKSPACE,
            vec![CommandArgument::new("index", CommandValue::Integer(index))],
        )
    };
    harness.invoke_with(window, &at(2)).unwrap();
    harness.wait_workspace(window, second, &others);
    // An empty slot completes without switching, as select_tab's does.
    assert!(matches!(
        harness.invoke_with(window, &at(3)),
        Ok(CommandOutcome::Completed)
    ));
    harness.cx.update(|cx| {
        assert!(!switching(cx, window));
        assert_eq!(shown_workspace(cx, window), Some(second));
    });
    harness.invoke_with(window, &at(1)).unwrap();
    harness.wait_workspace(window, harness.workspace, &[first]);
    // The ninth position is the last workspace, however many there are,
    // as select_tab's is.
    harness.invoke_with(window, &at(9)).unwrap();
    harness.wait_workspace(window, second, &others);
}

#[gpui::test]
fn two_windows_on_one_session_switch_workspaces_independently(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let split = harness.open_tab(ECHO);
    let left = harness.first_pane(split);
    let right = harness.split(split, left, SplitDirection::Right, ECHO);
    let one = harness.attach_showing(&[first, split]);
    let two = harness.attach_showing(&[first, split]);
    // The second workspace comes first: the panel it shows in every window
    // on the session resizes their terminals, and a grid change clears a
    // selection (#373).
    let (second, others) = harness.add_workspace(1);
    harness.activate(one, first);
    // Window two selects text in its view of `first` once the terminal
    // took the views' size, which a later resize would clear, then moves
    // to the split tab's right pane.
    harness.activate(two, first);
    harness.wait_text(two, first, "READY");
    let grid = harness
        .cx
        .update(|cx| terminal(cx, two, first).unwrap().read(cx).requested_grid);
    harness.wait_canonical(first, "the views' size", |snapshot| {
        snapshot.size == grid
    });
    let selection = harness.cx.update(|cx| terminal(cx, two, first)).unwrap();
    harness
        .cx
        .update(|cx| selection.update(cx, TerminalView::select_all));
    harness.focus(two, split, right);
    let untouched = |harness: &mut DesktopHarness| {
        harness.cx.update(|cx| {
            assert_eq!(shown_workspace(cx, two), Some(harness.workspace));
            assert_eq!(active(cx, two), Some(split));
            assert_eq!(focused_pane(cx, two, split), Some(right));
            assert!(selected(cx, two, first), "the selection went");
            assert_eq!(
                terminal(cx, two, first).map(|view| view.entity_id()),
                Some(selection.entity_id()),
                "the view was rebuilt"
            );
        });
    };

    harness.show_workspace(one, second, &others);
    untouched(&mut harness);
    harness.answers(one, others[0], "one");
    untouched(&mut harness);
    harness.show_workspace(one, harness.workspace, &[first, split]);
    untouched(&mut harness);

    // The other way round: window one keeps its workspace and tab.
    harness.show_workspace(two, second, &others);
    harness.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, one), Some(harness.workspace));
        assert_eq!(active(cx, one), Some(first));
    });
    harness.answers(two, others[0], "two");
}

#[gpui::test]
fn quit_waits_for_a_workspace_switch_and_saves_both_workspaces(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (second, others) = harness.add_workspace(1);
    harness.wait_projection();
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Subscribe);
    harness.select_workspace(window, second).unwrap();
    harness.wait_for("the switch to install its views", |cx| {
        runtime.worker_hooks.held(Stage::Subscribe) > 0
            && installing(cx, window)
    });
    harness.resume_quits();
    harness.cx.update(request_quit);
    harness.cx.update(|cx| {
        assert!(cx.global::<Desktop>().quit_pending);
        assert!(!terminating(cx));
    });
    runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_quit(harness.session);
    let (captured, layout) = harness.recorder.saved().remove(0);
    assert_eq!(captured.sessions[0].workspaces.len(), 2);
    let saved = &layout.windows[0];
    assert_eq!(saved.workspace.get(), second.get());
    assert_eq!(saved.active_tab.map(TabId::get), Some(others[0].get()));
    let remembered: Vec<_> = saved
        .remembered
        .iter()
        .map(|memory| (memory.workspace.get(), memory.active_tab))
        .collect();
    assert_eq!(
        remembered,
        [(harness.workspace.get(), Some(TabId::new(tab.get())))]
    );
}

#[gpui::test]
fn quit_from_another_window_waits_for_a_workspace_switch(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let split = harness.open_tab(ECHO);
    let left = harness.first_pane(split);
    let right = harness.split(split, left, SplitDirection::Right, ECHO);
    let one = harness.attach_showing(&[first, split]);
    harness.focus(one, split, right);
    let (second, others) = harness.add_workspace(1);
    harness.show_workspace(one, second, &others);
    let two = harness.attach_showing(&[first, split]);
    harness.show_workspace(two, second, &others);
    let runtime = Arc::clone(&harness.runtime);
    // The switch back installs its views, which wait for their viewers.
    runtime.worker_hooks.hold(Stage::Subscribe);
    harness.select_workspace(one, harness.workspace).unwrap();
    harness.wait_for("the switch to install its views", |cx| {
        runtime.worker_hooks.held(Stage::Subscribe) > 0 && installing(cx, one)
    });
    harness.resume_quits();
    harness.activate_window(two);
    harness.cx.update(request_quit);
    harness.cx.update(|cx| {
        assert!(
            cx.global::<Desktop>().quit_pending,
            "Quit ran before the switch restored its navigation"
        );
        assert!(!terminating(cx));
    });
    runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_quit(harness.session);
    let (_, layout) = harness.recorder.saved().remove(0);
    let saved = layout
        .windows
        .iter()
        .find(|saved| saved.workspace.get() == harness.workspace.get())
        .expect("the switching window was not saved on its workspace");
    assert_eq!(saved.active_tab.map(TabId::get), Some(split.get()));
    assert!(
        saved.focused_panes.iter().any(|focused| {
            focused.tab.get() == split.get()
                && focused.pane.get() == right.get()
        }),
        "{:?}",
        saved.focused_panes
    );
}

#[gpui::test]
fn a_checkpoint_waits_for_a_workspace_switch(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (second, others) = harness.add_workspace(1);
    harness.wait_projection();
    let runtime = Arc::clone(&harness.runtime);
    let recorder = harness.recorder.clone();
    let deferred = |harness: &mut DesktopHarness| {
        let before = recorder.saved().len();
        harness.cx.update(|cx| {
            take_checkpoint(cx);
            assert!(cx.global::<Desktop>().checkpoints.waiting_transfer);
        });
        runtime.persistence.flush();
        assert_eq!(recorder.saved().len(), before, "saved mid-switch");
        before
    };

    // A selected workspace's install.
    runtime.worker_hooks.hold(Stage::Subscribe);
    harness.select_workspace(window, second).unwrap();
    harness.wait_for("the switch to install", |cx| {
        runtime.worker_hooks.held(Stage::Subscribe) > 0
            && installing(cx, window)
    });
    let before = deferred(&mut harness);
    runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_for("the checkpoint after the switch settles", |_| {
        recorder.saved().len() > before
    });
    let (_, layout) = recorder.saved().pop().unwrap();
    assert_eq!(layout.windows[0].workspace.get(), second.get());
    harness.wait_workspace(window, second, &others);

    // An L1 follow-up's install, after the workspace's last tab closed.
    runtime.worker_hooks.hold(Stage::Subscribe);
    harness.close_tab_in(second, others[0]);
    harness.wait_for("the follow-up to install", |cx| {
        runtime.worker_hooks.held(Stage::Subscribe) > 0
            && installing(cx, window)
    });
    let before = deferred(&mut harness);
    runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_for("the checkpoint after the follow-up settles", |_| {
        recorder.saved().len() > before
    });
    let (_, layout) = recorder.saved().pop().unwrap();
    assert_eq!(layout.windows[0].workspace.get(), harness.workspace.get());
    runtime.terminate().unwrap();
}

#[gpui::test]
fn a_window_close_queued_behind_a_workspace_switch_runs_once_it_settles(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (second, _) = harness.add_workspace(1);
    harness.wait_projection();
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Subscribe);
    harness.select_workspace(window, second).unwrap();
    harness.wait_for("the switch to install", |cx| {
        runtime.worker_hooks.held(Stage::Subscribe) > 0
            && installing(cx, window)
    });
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Window, window, cx);
    });
    assert_eq!(runtime.worker_hooks.entered(Stage::Assess), 0, "not queued");
    runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_closed(window);
    // The window's attachment was the session's only one.
    assert!(!session_exists(&harness));
}

#[gpui::test]
fn workspace_changes_are_refused_while_a_quit_runs(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let job = harness.open_tab(RUNS_JOB);
    let window = harness.attach_showing(&[job]);
    harness.wait_text(window, job, "READY");
    let (second, _) = harness.add_workspace(1);
    harness.wait_projection();
    harness.resume_quits();
    let refused = |harness: &mut DesktopHarness, asking: bool| {
        let results = [
            harness.select_workspace(window, second),
            harness.invoke(window, ids::NEW_WORKSPACE),
            harness.invoke(window, ids::NEXT_WORKSPACE),
            harness.invoke(window, ids::PREVIOUS_WORKSPACE),
        ];
        let close = harness.invoke(window, ids::CLOSE_WORKSPACE);
        // While the Quit asks in this window, its dialog refuses first.
        for result in results.into_iter().chain([close]) {
            assert!(
                matches!(&result, Err(CommandError::Unavailable(reason))
                    if asking || reason.contains("quitting")),
                "{result:?}"
            );
        }
    };
    // Requested: the Quit waits for a pending switch.
    let id = window.window_id();
    harness.activate_window(window);
    harness.cx.update(|cx| {
        session_switch::hold_switch_spawn(cx, id);
        request_quit(cx);
        assert!(cx.global::<Desktop>().quit_pending);
    });
    refused(&mut harness, false);
    // Running: the Quit asks.
    harness
        .cx
        .update(|cx| session_switch::release_switch_spawn(cx, id));
    wait_quit_dialog(&mut harness, window);
    refused(&mut harness, true);
    harness.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, window), Some(harness.workspace));
    });
    assert!(harness.workspace_exists(second));
}

#[gpui::test]
fn a_failed_new_workspace_closes_only_what_it_created(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.fail(Stage::Switch);
    harness.invoke(window, ids::NEW_WORKSPACE).unwrap();
    harness.wait_for("the new workspace to fail", |cx| !switching(cx, window));
    harness.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, window), Some(harness.workspace));
        let shown = notices(cx, window);
        assert_eq!(
            shown
                .iter()
                .filter(|notice| notice.contains("Cannot create workspace"))
                .count(),
            1,
            "{shown:?}"
        );
    });
    assert_eq!(harness.workspaces(), [harness.workspace]);
    harness.answers(window, tab, "unchanged");

    // Without the failure, the named workspace shows its fresh shell.
    harness
        .invoke_with(
            window,
            &CommandInvocation::new(
                ids::NEW_WORKSPACE,
                vec![CommandArgument::new(
                    "name",
                    CommandValue::Text("build".into()),
                )],
            ),
        )
        .unwrap();
    let own = harness.workspace;
    harness.wait_for("the new workspace to show", |cx| {
        shown_workspace(cx, window).is_some_and(|workspace| {
            workspace != own
                && cx
                    .global::<Desktop>()
                    .hierarchy
                    .state()
                    .workspace(workspace)
                    .is_some_and(|info| {
                        info.custom_name.as_deref() == Some("build")
                    })
        }) && !switching(cx, window)
            && shown_tabs(cx, window).len() == 1
    });
    assert_eq!(harness.workspaces().len(), 2);
    let fresh = harness.cx.update(|cx| shown_tabs(cx, window))[0];
    harness.cx.update(|cx| assert!(!exited(cx, window, fresh)));
}

#[gpui::test]
fn quit_waits_for_a_new_workspace(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Switch);
    harness.invoke(window, ids::NEW_WORKSPACE).unwrap();
    harness.wait_for("the new workspace to wait for its worker", |_| {
        runtime.worker_hooks.held(Stage::Switch) == 1
    });
    harness.resume_quits();
    harness.cx.update(request_quit);
    harness.cx.update(|cx| {
        assert!(cx.global::<Desktop>().quit_pending);
        assert!(!terminating(cx));
    });
    runtime.worker_hooks.release(Stage::Switch);
    // The new workspace's shell is the configured one, whose own jobs may
    // make the Quit ask.
    harness.wait_for("the Quit to commit or ask", |cx| {
        terminating(cx)
            || cx.windows().into_iter().any(|handle| {
                handle
                    .update(cx, |root, _, cx| {
                        root.downcast::<WorkspaceView>().is_ok_and(|view| {
                            view.read(cx).close.confirmation
                                == Some(CloseTarget::Application)
                        })
                    })
                    .unwrap_or(false)
            })
    });
    if !harness.cx.update(|cx| terminating(cx)) {
        confirm_quit(&mut harness, window);
    }
    harness.wait_quit(harness.session);
    let (captured, layout) = harness.recorder.saved().remove(0);
    let saved: Vec<u64> = captured.sessions[0]
        .workspaces
        .iter()
        .map(|workspace| workspace.info.id.get())
        .collect();
    assert_eq!(saved.len(), 2, "the new workspace was not saved");
    assert_eq!(layout.windows[0].workspace.get(), saved[1]);
}

#[gpui::test]
fn selecting_a_closed_workspace_leaves_the_window_unchanged(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (second, _) = harness.add_workspace(1);
    let other = harness.other_session("other", 1);
    harness.wait_projection();
    // A pick made before the workspace closed.
    let picked = workspace_pick(ids::SELECT_WORKSPACE, second);
    harness.runtime.lock().close_workspace(second).unwrap();
    harness.wait_projection();
    for invocation in [
        picked,
        workspace_pick(ids::SELECT_WORKSPACE, other.workspace),
    ] {
        let result = harness.invoke_with(window, &invocation);
        assert!(
            matches!(&result, Err(CommandError::Unavailable(reason))
                if reason == "Workspace no longer exists"),
            "{result:?}"
        );
    }
    harness.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, window), Some(harness.workspace));
        assert_eq!(shown_session(cx, window), Some(harness.session));
        assert!(!switching(cx, window));
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
    harness.answers(window, tab, "unchanged");
}

#[gpui::test]
fn closing_a_workspaces_last_tab_moves_to_the_previous_workspace(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let (third, thirds) = harness.add_workspace(1);
    let (second, seconds) = harness.add_workspace(1);
    // Memory runs third, then the harness workspace, while session order
    // puts the harness workspace first.
    harness.show_workspace(window, third, &thirds);
    harness.show_workspace(window, second, &seconds);
    let attachment = harness.cx.update(|cx| attachment_of(cx, window));
    harness.activate(window, seconds[0]);
    harness.invoke(window, ids::CLOSE_TAB).unwrap();
    harness.wait_workspace(window, third, &thirds);
    harness.cx.update(|cx| {
        assert_eq!(attachment_of(cx, window), attachment);
        assert!(!terminating(cx) && !quitting(cx));
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
    assert!(
        !harness.workspace_exists(second),
        "the empty workspace stays"
    );
    assert_eq!(harness.workspaces(), [harness.workspace, third]);
    harness.answers(window, thirds[0], "moved-on");
}

#[gpui::test]
fn an_exit_close_of_a_workspaces_last_tab_moves_on(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let (second, exits) = harness.add_workspace_running(EXITS, 1);
    harness.show_workspace(window, second, &exits);
    harness.exit_tab(window, exits[0]);
    harness.wait_workspace(window, harness.workspace, &[first]);
    assert!(!harness.workspace_exists(second));
    harness.answers(window, first, "back");
}

#[gpui::test]
fn an_emptied_window_on_its_sessions_only_workspace_closes_with_the_session(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.activate(window, tab);
    harness.invoke(window, ids::CLOSE_TAB).unwrap();
    harness.wait_closed(window);
    // The window's attachment was the session's last, and the session
    // had no other workspace, so it closed as on `main`.
    assert!(!session_exists(&harness));
}

/// How a window's last tab goes away.
#[derive(Clone, Copy)]
enum LastTabGoes {
    Closed,
    ExitCloses,
    LonePaneClosed,
    MovedToAnotherSession,
}

/// Empties the harness workspace, which `window` shows beside an empty
/// sibling, in the way `goes` names, and checks that the window shows the
/// sibling's empty state and the session survives.
fn emptying_beside_an_empty_workspace_shows_it(
    cx: &mut TestAppContext,
    goes: LastTabGoes,
) {
    let mut harness = DesktopHarness::new(cx);
    let script = match goes {
        LastTabGoes::ExitCloses => EXITS,
        _ => ECHO,
    };
    let tab = harness.open_tab(script);
    let window = harness.attach_showing(&[tab]);
    let (empty, _) = harness.add_workspace(0);
    harness.wait_projection();
    match goes {
        LastTabGoes::Closed | LastTabGoes::LonePaneClosed => {
            harness.activate(window, tab);
            let command = match goes {
                LastTabGoes::Closed => ids::CLOSE_TAB,
                _ => ids::CLOSE_PANE,
            };
            harness.invoke(window, command).unwrap();
        }
        LastTabGoes::ExitCloses => harness.exit_tab(window, tab),
        LastTabGoes::MovedToAnotherSession => {
            let other = harness.other_session("other", 0);
            harness
                .runtime
                .lock()
                .move_tab(harness.workspace, tab, other.workspace, None)
                .unwrap();
        }
    }
    harness.wait_workspace(window, empty, &[]);
    harness.wait_for("the empty state", |cx| empty_state(cx, window).is_some());
    harness.cx.update(|cx| {
        assert!(cx.windows().contains(&window));
        assert!(!terminating(cx) && !quitting(cx));
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
    assert!(session_exists(&harness));
    assert_eq!(harness.workspaces(), [empty]);
}

#[gpui::test]
fn closing_the_last_tab_beside_an_empty_workspace_shows_it(
    cx: &mut TestAppContext,
) {
    emptying_beside_an_empty_workspace_shows_it(cx, LastTabGoes::Closed);
}

#[gpui::test]
fn an_exit_close_beside_an_empty_workspace_shows_it(cx: &mut TestAppContext) {
    emptying_beside_an_empty_workspace_shows_it(cx, LastTabGoes::ExitCloses);
}

#[gpui::test]
fn closing_the_last_lone_pane_beside_an_empty_workspace_shows_it(
    cx: &mut TestAppContext,
) {
    emptying_beside_an_empty_workspace_shows_it(
        cx,
        LastTabGoes::LonePaneClosed,
    );
}

#[gpui::test]
fn moving_the_last_tab_away_beside_an_empty_workspace_shows_it(
    cx: &mut TestAppContext,
) {
    emptying_beside_an_empty_workspace_shows_it(
        cx,
        LastTabGoes::MovedToAnotherSession,
    );
}

#[gpui::test]
fn a_linux_last_window_emptied_beside_an_empty_workspace_stays_open(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.as_linux(window);
    let (empty, _) = harness.add_workspace(0);
    // A session no window shows would make an emptied window close Quit.
    let unviewed = harness.unviewed_session();
    harness.wait_projection();
    harness.resume_quits();
    harness.activate(window, tab);
    harness.invoke(window, ids::CLOSE_TAB).unwrap();
    harness.wait_workspace(window, empty, &[]);
    harness.cx.update(|cx| {
        assert!(cx.windows().contains(&window));
        assert!(!quitting(cx) && !terminating(cx));
        assert_eq!(cx.global::<Desktop>().emptied_quit, None);
    });
    assert!(session_exists(&harness));
    assert!(harness.runtime.lock().session(unviewed).is_some());
}

#[gpui::test]
fn two_windows_emptied_beside_an_empty_workspace_both_show_it(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[tab]);
    let two = harness.attach_showing(&[tab]);
    let (empty, _) = harness.add_workspace(0);
    harness.wait_projection();
    harness.activate(one, tab);
    harness.invoke(one, ids::CLOSE_TAB).unwrap();
    harness.wait_workspace(one, empty, &[]);
    harness.wait_workspace(two, empty, &[]);
    assert_eq!(harness.workspaces(), [empty]);
    assert_eq!(harness.attachments_of(harness.session), 2);
}

#[gpui::test]
fn a_new_sibling_turns_a_held_emptied_detach_into_leave_workspace(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    // Another client's attachment makes the emptied close a detach.
    let other_client = harness.runtime.lock().attach_session(harness.session);
    harness.wait_projection();
    let attachment = harness.cx.update(|cx| attachment_of(cx, window));
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Assessed);
    harness.close_tab(tab);
    harness.wait_for("the emptied close to assess", |_| {
        runtime.worker_hooks.held(Stage::Assessed) == 1
    });
    // Another client creates a workspace with a tab before the commit.
    let (sibling, arrived) = harness.add_workspace(1);
    let arrived = arrived[0];
    runtime.worker_hooks.release(Stage::Assessed);
    harness.wait_workspace(window, sibling, &[arrived]);
    harness.cx.update(|cx| {
        assert_eq!(attachment_of(cx, window), attachment, "it detached");
    });
    assert!(!harness.workspace_exists(harness.workspace));
    assert!(other_client.is_ok());
    harness.answers(window, arrived, "arrived");
}

#[gpui::test]
fn a_stale_emptied_detach_on_linux_does_not_quit(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.as_linux(window);
    let unviewed = harness.unviewed_session();
    harness.wait_projection();
    harness.resume_quits();
    // The last window's emptied close of a session with unviewed sessions
    // beside it would become Quit, had nothing changed.
    assert_eq!(
        harness.update_view(window, |view, _, cx| {
            view.emptied_close_target(Platform::Linux, cx)
        }),
        CloseTarget::Application
    );
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Assessed);
    harness.close_tab(tab);
    harness.wait_for("the emptied close to assess", |_| {
        runtime.worker_hooks.held(Stage::Assessed) == 1
    });
    let (sibling, arrived) = harness.add_workspace(1);
    let arrived = arrived[0];
    runtime.worker_hooks.release(Stage::Assessed);
    harness.wait_workspace(window, sibling, &[arrived]);
    harness.cx.update(|cx| {
        assert!(!quitting(cx) && !terminating(cx), "the stale ticket quit");
        assert_eq!(cx.global::<Desktop>().emptied_quit, None);
    });
    assert!(runtime.lock().session(unviewed).is_some());
}

/// Whether `captured` saved the session `session` names.
fn saves_session(
    captured: &huterm_protocol::RuntimeCapture,
    session: SessionId,
) -> bool {
    captured
        .sessions
        .iter()
        .any(|kept| kept.info.id.get() == session.get())
}

#[gpui::test]
fn a_linux_emptied_quit_saves_neither_the_emptied_session_nor_its_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.as_linux(window);
    let unviewed = harness.unviewed_session();
    harness.wait_projection();
    harness.resume_quits();
    // Closing the last tab of the session's only workspace in the last
    // window becomes Quit, which ends the session the user just emptied.
    harness.activate(window, tab);
    harness.invoke(window, ids::CLOSE_TAB).unwrap();
    harness.wait_quit(unviewed);
    harness.cx.update(|cx| {
        assert_eq!(
            cx.global::<Desktop>().emptied_quit,
            Some(window.window_id())
        );
    });
    let (captured, layout) = harness.recorder.saved().remove(0);
    assert!(!saves_session(&captured, harness.session), "{captured:?}");
    assert!(layout.windows.is_empty(), "{layout:?}");
}

/// Holds the emptied close of the harness session's only workspace at
/// `stage`, with the last-window rule of `platform`, takes a checkpoint,
/// and checks that it saves neither the session nor its window, while it
/// saves a session no window shows.
fn a_checkpoint_during_an_emptied_close_saves_neither(
    cx: &mut TestAppContext,
    platform: Platform,
    stage: Stage,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.update_view(window, |view, _, _| {
        view.test_platform = Some(platform);
    });
    let unviewed = harness.unviewed_session();
    harness.wait_projection();
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(stage);
    harness.close_tab(tab);
    harness.wait_for("the emptied close to hold", |_| {
        runtime.worker_hooks.held(stage) == 1
    });
    let recorder = harness.recorder.clone();
    let before = recorder.saved().len();
    harness.cx.update(take_checkpoint);
    harness.runtime.persistence.flush();
    harness.wait_for("the checkpoint", |_| recorder.saved().len() > before);
    let (captured, layout) = recorder.saved().pop().unwrap();
    assert!(!saves_session(&captured, harness.session), "{captured:?}");
    assert!(saves_session(&captured, unviewed), "{captured:?}");
    assert!(layout.windows.is_empty(), "{layout:?}");
    runtime.worker_hooks.release(stage);
    if platform == Platform::Linux {
        // The last window would strand the unviewed session, so the close
        // becomes Quit (W36), whose capture saves neither either.
        harness.wait_quit(unviewed);
        let (captured, layout) = recorder.saved().pop().unwrap();
        assert!(!saves_session(&captured, harness.session), "{captured:?}");
        assert!(layout.windows.is_empty(), "{layout:?}");
    } else {
        harness.wait_closed(window);
    }
}

#[gpui::test]
fn a_checkpoint_while_an_emptied_close_assesses_saves_neither(
    cx: &mut TestAppContext,
) {
    a_checkpoint_during_an_emptied_close_saves_neither(
        cx,
        Platform::MacOs,
        Stage::Assess,
    );
}

#[gpui::test]
fn a_checkpoint_after_an_emptied_close_assessed_saves_neither(
    cx: &mut TestAppContext,
) {
    a_checkpoint_during_an_emptied_close_saves_neither(
        cx,
        Platform::MacOs,
        Stage::Assessed,
    );
}

#[gpui::test]
fn a_checkpoint_while_an_emptied_close_commits_saves_neither(
    cx: &mut TestAppContext,
) {
    a_checkpoint_during_an_emptied_close_saves_neither(
        cx,
        Platform::MacOs,
        Stage::Commit,
    );
}

/// On Linux the held commit is the check that turns the close into Quit.
#[gpui::test]
fn a_checkpoint_while_a_linux_emptied_close_checks_for_quit_saves_neither(
    cx: &mut TestAppContext,
) {
    a_checkpoint_during_an_emptied_close_saves_neither(
        cx,
        Platform::Linux,
        Stage::Commit,
    );
}

#[gpui::test]
fn a_checkpoint_before_a_refused_emptied_close_installs_saves_the_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.wait_projection();
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Assess);
    harness.close_tab(tab);
    harness.wait_for("the emptied close to hold", |_| {
        runtime.worker_hooks.held(Stage::Assess) == 1
    });
    // A tab another client opens in the window's only workspace makes the
    // runtime refuse the close; its install holds before it fills the
    // window's record.
    runtime.worker_hooks.hold(Stage::Subscribe);
    let arrived = harness.open_tab(ECHO);
    runtime.worker_hooks.release(Stage::Assess);
    harness.wait_for("the refused close and the held install", |cx| {
        runtime.worker_hooks.held(Stage::Subscribe) == 1
            && window
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>().is_ok_and(|view| {
                        let view = view.read(cx);
                        view.tabs.is_empty()
                            && view.close.current.is_none()
                            && view.close.emptied == EmptiedClose::Refused
                    })
                })
                .unwrap_or(false)
    });
    let recorder = harness.recorder.clone();
    let before = recorder.saved().len();
    harness.cx.update(take_checkpoint);
    harness.runtime.persistence.flush();
    harness.wait_for("the checkpoint", |_| recorder.saved().len() > before);
    let (captured, layout) = recorder.saved().pop().unwrap();
    assert!(saves_session(&captured, harness.session), "{captured:?}");
    assert_eq!(layout.windows.len(), 1, "{layout:?}");
    assert_eq!(layout.windows[0].workspace.get(), harness.workspace.get());
    runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_workspace(window, harness.workspace, &[arrived]);
}

/// Holds `window`'s emptied close of the harness workspace, assessed as
/// `LeaveWorkspace` beside a populated sibling, on Linux with a session no
/// window shows. Returns the sibling, its tab, and that session.
fn hold_a_leave_beside_a_populated_sibling(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    tab: TabId,
) -> (WorkspaceId, TabId, SessionId) {
    harness.as_linux(window);
    let (sibling, siblings) = harness.add_workspace(1);
    let unviewed = harness.unviewed_session();
    harness.wait_projection();
    harness.resume_quits();
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Assessed);
    harness.close_tab(tab);
    harness.wait_for("the leave to assess", |_| {
        runtime.worker_hooks.held(Stage::Assessed) == 1
    });
    (sibling, siblings[0], unviewed)
}

#[gpui::test]
fn the_only_sibling_closing_turns_a_held_leave_workspace_into_a_window_close(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (sibling, _, unviewed) =
        hold_a_leave_beside_a_populated_sibling(&mut harness, window, tab);
    // The only sibling closes before the commit.
    harness.runtime.lock().close_workspace(sibling).unwrap();
    harness.runtime.worker_hooks.release(Stage::Assessed);
    // Assessed again, the emptied close removes the window, and the
    // last-window rule applies: on Linux it becomes Quit while a session
    // no window shows remains.
    harness.wait_quit(unviewed);
    harness.cx.update(|cx| {
        assert_eq!(
            cx.global::<Desktop>().emptied_quit,
            Some(window.window_id())
        );
    });
}

#[gpui::test]
fn a_sibling_emptying_keeps_a_held_leave_workspace_leaving(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (sibling, sibling_tab, unviewed) =
        hold_a_leave_beside_a_populated_sibling(&mut harness, window, tab);
    // The only populated sibling empties before the commit.
    harness.close_tab_in(sibling, sibling_tab);
    harness.runtime.worker_hooks.release(Stage::Assessed);
    // Assessed again, the window still leaves, now for the empty sibling.
    harness.wait_workspace(window, sibling, &[]);
    harness.cx.update(|cx| {
        assert!(cx.windows().contains(&window));
        assert!(!quitting(cx) && !terminating(cx));
        assert_eq!(cx.global::<Desktop>().emptied_quit, None);
    });
    assert_eq!(harness.workspaces(), [sibling]);
    assert!(harness.runtime.lock().session(unviewed).is_some());
}

#[gpui::test]
fn leaving_a_workspace_never_becomes_quit_on_linux(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    harness.as_linux(window);
    let (second, seconds) = harness.add_workspace(1);
    let unviewed = harness.unviewed_session();
    harness.show_workspace(window, second, &seconds);
    harness.resume_quits();
    harness.activate(window, seconds[0]);
    harness.invoke(window, ids::CLOSE_TAB).unwrap();
    harness.wait_workspace(window, harness.workspace, &[first]);
    harness.cx.update(|cx| {
        assert!(!quitting(cx) && !terminating(cx));
        assert_eq!(cx.global::<Desktop>().emptied_quit, None);
    });
    assert!(harness.runtime.lock().session(unviewed).is_some());
}

#[gpui::test]
fn an_l1_follow_up_whose_target_went_resolves_again(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let (second, seconds) = harness.add_workspace(1);
    let (third, thirds) = harness.add_workspace(1);
    harness.show_workspace(window, second, &seconds);
    harness.show_workspace(window, third, &thirds);
    let runtime = Arc::clone(&harness.runtime);
    // The follow-up chooses the second workspace, which it remembers and
    // which holds a tab, and waits for its viewers there.
    runtime.worker_hooks.hold(Stage::Subscribe);
    harness.close_tab_in(third, thirds[0]);
    harness.wait_for("the follow-up to install the second workspace", |cx| {
        shown_workspace(cx, window) == Some(second)
            && installing(cx, window)
            && runtime.worker_hooks.held(Stage::Subscribe) > 0
    });
    // Its target empties before the follow-up settles.
    harness.close_tab_in(second, seconds[0]);
    runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_workspace(window, harness.workspace, &[first]);
    assert_eq!(harness.workspaces(), [harness.workspace]);
    harness.answers(window, first, "resolved");
}

#[gpui::test]
fn quit_waits_for_a_leave_workspace_follow_up(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let (second, seconds) = harness.add_workspace(1);
    harness.show_workspace(window, second, &seconds);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Commit);
    harness.close_tab_in(second, seconds[0]);
    harness.wait_for("the leave to commit", |_| {
        runtime.worker_hooks.held(Stage::Commit) == 1
    });
    harness.resume_quits();
    harness.cx.update(|cx| {
        take_checkpoint(cx);
        assert!(cx.global::<Desktop>().checkpoints.waiting_transfer);
        request_quit(cx);
        assert!(
            cx.global::<Desktop>().quit_pending,
            "Quit ran while the leave committed"
        );
    });
    runtime.worker_hooks.hold(Stage::Subscribe);
    runtime.worker_hooks.release(Stage::Commit);
    harness.wait_for("the follow-up to install", |cx| {
        installing(cx, window)
            && runtime.worker_hooks.held(Stage::Subscribe) > 0
    });
    harness.cx.update(|cx| {
        assert!(cx.global::<Desktop>().quit_pending);
        assert!(!terminating(cx));
    });
    runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_quit(harness.session);
    let (captured, layout) = harness.recorder.saved().pop().unwrap();
    assert_eq!(captured.sessions[0].workspaces.len(), 1);
    assert_eq!(layout.windows[0].workspace.get(), harness.workspace.get());
    assert_eq!(
        layout.windows[0].active_tab.map(TabId::get),
        Some(first.get())
    );
}

#[gpui::test]
fn a_cancelled_quit_lets_an_owed_leave_workspace_run(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let job = harness.open_tab(RUNS_JOB);
    let window = harness.attach_showing(&[job]);
    harness.wait_text(window, job, "READY");
    let (second, seconds) = harness.add_workspace(1);
    harness.show_workspace(window, second, &seconds);
    harness.resume_quits();
    harness.activate_window(window);
    // The Quit asks, because the workspace the window left runs a job.
    harness.cx.update(request_quit);
    wait_quit_dialog(&mut harness, window);
    harness.close_tab_in(second, seconds[0]);
    harness.wait_for("the emptied close to be owed", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    let view = view.read(cx);
                    view.tabs.is_empty()
                        && view.close.emptied == EmptiedClose::Owed
                })
            })
            .unwrap_or(false)
    });
    // The Quit's dialog holds the owed close back.
    assert_eq!(
        confirmation(&mut harness, window),
        Some(CloseTarget::Application)
    );
    assert!(harness.workspace_exists(second));
    harness.update_view(window, |view, window, cx| {
        view.cancel_confirmation(window, cx);
    });
    harness.wait_workspace(window, harness.workspace, &[job]);
    assert!(!harness.workspace_exists(second));
    harness.cx.update(|cx| assert!(!quitting(cx)));
}

#[gpui::test]
fn two_windows_on_an_emptied_workspace_both_move_on(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let (second, seconds) = harness.add_workspace(1);
    let (shared, shareds) = harness.add_workspace(1);
    let one = harness.attach_showing(&[first]);
    harness.show_workspace(one, shared, &shareds);
    let two = harness.attach_showing(&[first]);
    harness.show_workspace(two, second, &seconds);
    harness.show_workspace(two, shared, &shareds);
    harness.activate(one, shareds[0]);
    harness.invoke(one, ids::CLOSE_TAB).unwrap();
    harness.wait_workspace(one, harness.workspace, &[first]);
    harness.wait_workspace(two, second, &seconds);
    assert!(!harness.workspace_exists(shared));
    assert_eq!(harness.workspaces(), [harness.workspace, second]);
    assert_eq!(harness.attachments_of(harness.session), 2);
}

#[gpui::test]
fn leaving_a_workspace_leaves_other_windows_alone(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let (second, seconds) = harness.add_workspace(1);
    let (leaving, leavings) = harness.add_workspace(1);
    let one = harness.attach_showing(&[first]);
    harness.show_workspace(one, leaving, &leavings);
    let two = harness.attach_showing(&[first]);
    harness.show_workspace(two, second, &seconds);
    let view = harness
        .cx
        .update(|cx| terminal(cx, two, seconds[0]))
        .unwrap();
    harness.activate(one, leavings[0]);
    harness.invoke(one, ids::CLOSE_TAB).unwrap();
    harness.wait_workspace(one, harness.workspace, &[first]);
    harness.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, two), Some(second));
        assert_eq!(active(cx, two), Some(seconds[0]));
        assert_eq!(
            terminal(cx, two, seconds[0]).map(|shown| shown.entity_id()),
            Some(view.entity_id())
        );
    });
    harness.answers(two, seconds[0], "untouched");
}

#[gpui::test]
fn a_quit_cancels_an_own_workspace_close(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let (second, jobs) = harness.add_workspace_running(RUNS_JOB, 1);
    harness.show_workspace(window, second, &jobs);
    harness.wait_text(window, jobs[0], "READY");
    harness.resume_quits();
    harness.activate_window(window);
    let own = CloseTarget::Workspace(second);

    // Confirming: the Quit's dialog replaces the close's.
    harness.invoke(window, ids::CLOSE_WORKSPACE).unwrap();
    harness.wait_for("the close to ask", |cx| confirming(cx, window));
    assert_eq!(confirmation(&mut harness, window), Some(own.clone()));
    harness.cx.update(request_quit);
    wait_quit_dialog(&mut harness, window);
    assert_eq!(harness.pending_spawns(), 0);
    harness.update_view(window, |view, window, cx| {
        view.cancel_confirmation(window, cx);
    });
    harness.cx.update(|cx| assert!(!quitting(cx)));
    assert!(harness.workspace_exists(second));

    // Assessing: the Quit cancels the check, which answers into nothing.
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Assess);
    harness.invoke(window, ids::CLOSE_WORKSPACE).unwrap();
    harness.wait_for("the close to assess", |_| {
        runtime.worker_hooks.held(Stage::Assess) == 1
    });
    harness.cx.update(request_quit);
    harness.update_view(window, |view, _, _| {
        assert_ne!(view.close.current, Some(own.clone()), "not cancelled");
    });
    harness.cx.update(|cx| {
        assert!(!cx.global::<Desktop>().quit_pending);
    });
    runtime.worker_hooks.release(Stage::Assess);
    wait_quit_dialog(&mut harness, window);
    confirm_quit(&mut harness, window);
    harness.wait_quit(harness.session);
    let (captured, layout) = harness.recorder.saved().pop().unwrap();
    assert!(
        captured.sessions[0]
            .workspaces
            .iter()
            .any(|workspace| workspace.info.id.get() == second.get()),
        "the workspace the cancelled close named was not saved"
    );
    assert_eq!(layout.windows[0].workspace.get(), second.get());
}

#[gpui::test]
fn quit_waits_for_a_committing_workspace_close_and_its_follow_up(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let split = harness.open_tab(ECHO);
    let left = harness.first_pane(split);
    let right = harness.split(split, left, SplitDirection::Right, ECHO);
    let one = harness.attach_showing(&[first, split]);
    harness.focus(one, split, right);
    let (second, seconds) = harness.add_workspace(1);
    harness.show_workspace(one, second, &seconds);
    let other = harness.other_session("other", 1);
    let two = harness.attach_showing(&[first, split]);
    harness.switch_to(two, other.session, &other.tabs);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Commit);
    harness.invoke(one, ids::CLOSE_WORKSPACE).unwrap();
    harness.wait_for("the close to commit", |_| {
        runtime.worker_hooks.held(Stage::Commit) == 1
    });
    harness.resume_quits();
    harness.activate_window(two);
    harness.cx.update(request_quit);
    harness.cx.update(|cx| {
        assert!(cx.global::<Desktop>().quit_pending);
        assert!(!terminating(cx));
    });
    runtime.worker_hooks.hold(Stage::Subscribe);
    runtime.worker_hooks.release(Stage::Commit);
    harness.wait_for("the follow-up to install", |cx| {
        installing(cx, one) && runtime.worker_hooks.held(Stage::Subscribe) > 0
    });
    harness.cx.update(|cx| {
        assert!(
            cx.global::<Desktop>().quit_pending,
            "Quit ran before the follow-up restored its navigation"
        );
    });
    runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_quit(other.session);
    let (captured, layout) = harness.recorder.saved().pop().unwrap();
    let workspaces: Vec<u64> = captured
        .sessions
        .iter()
        .find(|session| session.info.id.get() == harness.session.get())
        .unwrap()
        .workspaces
        .iter()
        .map(|workspace| workspace.info.id.get())
        .collect();
    assert_eq!(workspaces, [harness.workspace.get()]);
    let saved = layout
        .windows
        .iter()
        .find(|saved| saved.session.get() == harness.session.get())
        .expect("the window that followed was not saved");
    assert_eq!(saved.workspace.get(), harness.workspace.get());
    assert_eq!(saved.active_tab.map(TabId::get), Some(split.get()));
    assert!(saved.focused_panes.iter().any(|focused| {
        focused.tab.get() == split.get() && focused.pane.get() == right.get()
    }));
}

#[gpui::test]
fn a_checkpoint_waits_for_an_own_workspace_close(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let (second, seconds) = harness.add_workspace(1);
    harness.show_workspace(window, second, &seconds);
    let runtime = Arc::clone(&harness.runtime);
    let recorder = harness.recorder.clone();
    runtime.worker_hooks.hold(Stage::Commit);
    harness.invoke(window, ids::CLOSE_WORKSPACE).unwrap();
    harness.wait_for("the close to commit", |_| {
        runtime.worker_hooks.held(Stage::Commit) == 1
    });
    let before = recorder.saved().len();
    harness.cx.update(|cx| {
        take_checkpoint(cx);
        assert!(cx.global::<Desktop>().checkpoints.waiting_transfer);
    });
    runtime.persistence.flush();
    assert_eq!(recorder.saved().len(), before, "saved mid-close");
    runtime.worker_hooks.release(Stage::Commit);
    harness.wait_for("the checkpoint after the follow-up settles", |_| {
        recorder.saved().len() > before
    });
    let (_, layout) = recorder.saved().pop().unwrap();
    assert_eq!(layout.windows[0].workspace.get(), harness.workspace.get());
    runtime.terminate().unwrap();
}

#[gpui::test]
fn closing_a_shared_workspace_moves_both_windows(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let (second, seconds) = harness.add_workspace(1);
    let (shared, shareds) = harness.add_workspace(1);
    let one = harness.attach_showing(&[first]);
    harness.show_workspace(one, shared, &shareds);
    let two = harness.attach_showing(&[first]);
    harness.show_workspace(two, second, &seconds);
    harness.show_workspace(two, shared, &shareds);
    harness.invoke(one, ids::CLOSE_WORKSPACE).unwrap();
    harness.wait_workspace(one, harness.workspace, &[first]);
    harness.wait_workspace(two, second, &seconds);
    assert!(!harness.workspace_exists(shared));
    assert_eq!(harness.close_failures(), 0);
    harness
        .cx
        .update(|cx| assert_eq!(cx.global::<Desktop>().pending_spawns, 0));
}

#[gpui::test]
fn closing_a_workspace_shown_empty_moves_its_window(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let (empty, _) = harness.add_workspace(0);
    let one = harness.attach_showing(&[first]);
    let two = harness.attach_showing(&[first]);
    harness.show_workspace(two, empty, &[]);
    harness
        .cx
        .update(|cx| assert!(empty_state(cx, two).is_some()));
    harness
        .invoke_with(one, &workspace_pick(ids::CLOSE_WORKSPACE, empty))
        .unwrap();
    harness.wait_workspace(two, harness.workspace, &[first]);
    harness.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, one), Some(harness.workspace));
    });
    assert!(!harness.workspace_exists(empty));
    harness.answers(two, first, "two");
}

#[gpui::test]
fn a_failed_workspace_teardown_still_leaves_the_workspace(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let (second, seconds) = harness.add_workspace(1);
    harness.show_workspace(window, second, &seconds);
    harness.runtime.worker_hooks.fail(Stage::Commit);
    harness.invoke(window, ids::CLOSE_WORKSPACE).unwrap();
    harness.wait_workspace(window, harness.workspace, &[first]);
    assert!(!harness.workspace_exists(second));
    assert_eq!(harness.close_failures(), 1);
    harness.cx.update(|cx| {
        let shown = notices(cx, window);
        assert!(
            shown.iter().any(|notice| notice.contains("Close failed")),
            "{shown:?}"
        );
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
}

#[gpui::test]
fn a_stale_own_workspace_close_releases_its_hold(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let (second, jobs) = harness.add_workspace_running(RUNS_JOB, 1);
    harness.show_workspace(window, second, &jobs);
    harness.wait_text(window, jobs[0], "READY");
    let own = CloseTarget::Workspace(second);
    harness.invoke(window, ids::CLOSE_WORKSPACE).unwrap();
    harness.wait_for("the close to ask", |cx| confirming(cx, window));
    // Another client opens a tab in the workspace, so the consent is stale.
    let runtime = Arc::clone(&harness.runtime);
    let arrived = runtime
        .lock()
        .open_tab(second, &fixture(ECHO))
        .unwrap()
        .tab
        .id;
    runtime.worker_hooks.hold(Stage::Commit);
    harness.update_view(window, |view, window, cx| {
        let generation = view.close.generation;
        view.confirm_confirmation(own.clone(), generation, window, cx);
    });
    harness.wait_for("the commit to start", |_| {
        runtime.worker_hooks.held(Stage::Commit) == 1
    });
    assert_eq!(harness.pending_spawns(), 1, "the commit holds Quit");
    runtime.worker_hooks.release(Stage::Commit);
    // Refused as stale, the close assesses again and asks again; nothing
    // followed, and the hold went with the commit.
    harness.wait_for("the close to ask again", |cx| confirming(cx, window));
    assert_eq!(confirmation(&mut harness, window), Some(own));
    assert_eq!(harness.pending_spawns(), 0);
    harness.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, window), Some(second));
        assert!(!switching(cx, window));
    });
    assert!(harness.workspace_exists(second));
    harness.update_view(window, |view, window, cx| {
        view.cancel_confirmation(window, cx);
    });
    harness.wait_workspace(window, second, &[jobs[0], arrived]);
}

#[gpui::test]
fn closing_the_populated_workspace_shows_the_empty_one(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    // The harness workspace stays empty: the window starts there, on the
    // empty state.
    let window = harness.attach_window();
    harness.wait_for("the empty state", |cx| empty_state(cx, window).is_some());
    let (second, seconds) = harness.add_workspace(1);
    harness.show_workspace(window, second, &seconds);
    harness.invoke(window, ids::CLOSE_WORKSPACE).unwrap();
    harness.wait_workspace(window, harness.workspace, &[]);
    harness.wait_for("the empty state again", |cx| {
        empty_state(cx, window).is_some()
    });
    harness.cx.update(|cx| {
        let shown = empty_state(cx, window).unwrap();
        assert_eq!(shown.title(), "No tabs in Workspace 1");
        // The session's only workspace offers neither switch nor close.
        assert!(!shown.switch && !shown.close);
        assert!(cx.windows().contains(&window));
    });
    assert!(session_exists(&harness));
    assert_eq!(harness.workspaces(), [harness.workspace]);
}

#[gpui::test]
fn the_last_workspace_cannot_be_closed_as_a_workspace(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let unavailable = |harness: &mut DesktopHarness, command| {
        harness.update_view(window, |view, _, cx| {
            let target = PaletteTarget {
                session: view.attached_session(cx),
                workspace: view.workspace_id(cx),
                tab: view.active_tab(cx),
                terminal: None,
                terminal_view: None,
                contexts: Vec::new(),
            };
            view.command_availability(command, &target, cx).is_err()
        })
    };
    assert!(unavailable(&mut harness, ids::CLOSE_WORKSPACE));
    assert!(matches!(
        harness.invoke(window, ids::CLOSE_WORKSPACE),
        Err(CommandError::Unavailable(_))
    ));
    // Reached anyway, core refuses it as the session's last.
    let workspace = harness.workspace;
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Workspace(workspace), window, cx);
    });
    harness.wait_for("the refusal", |cx| {
        notices(cx, window)
            .iter()
            .any(|notice| notice.contains("only workspace"))
    });
    harness.wait_for("the close to end", |cx| !busy(cx, window));
    assert!(harness.workspace_exists(workspace));
    harness.answers(window, tab, "kept");
}

#[gpui::test]
fn a_populated_workspace_moved_away_is_left(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let (moved, moveds) = harness.add_workspace(1);
    harness.show_workspace(window, moved, &moveds);
    let other = harness.other_session("other", 0);
    harness
        .runtime
        .lock()
        .move_workspace(harness.session, moved, other.session, None)
        .unwrap();
    harness.wait_workspace(window, harness.workspace, &[first]);
    {
        let mux = harness.runtime.lock();
        let record = mux.workspace(moved).expect("the moved workspace closed");
        assert_eq!(record.session_id, other.session);
        assert_eq!(record.tabs.len(), 1);
    }
    // Both shells keep running.
    assert_eq!(harness.runtime.lock().terminal_count(), 2);
}

#[gpui::test]
fn an_empty_workspace_moved_away_is_left_and_kept(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let (moved, _) = harness.add_workspace(0);
    harness.show_workspace(window, moved, &[]);
    harness
        .cx
        .update(|cx| assert!(empty_state(cx, window).is_some()));
    let other = harness.other_session("other", 0);
    harness
        .runtime
        .lock()
        .move_workspace(harness.session, moved, other.session, None)
        .unwrap();
    harness.wait_workspace(window, harness.workspace, &[first]);
    let mux = harness.runtime.lock();
    assert_eq!(
        mux.session(other.session).unwrap().workspaces,
        [other.workspace, moved]
    );
}

#[gpui::test]
fn closing_a_session_shown_on_two_workspaces(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[first]);
    let (second, seconds) = harness.add_workspace(1);
    let two = harness.attach_showing(&[first]);
    harness.show_workspace(two, second, &seconds);
    harness.show_workspace(one, second, &seconds);
    harness.show_workspace(one, harness.workspace, &[first]);
    let other = harness.other_session("other", 1);
    harness.wait_projection();
    harness.invoke(one, ids::CLOSE_SESSION).unwrap();
    harness.wait_for("window one to follow to the other session", |cx| {
        shown_session(cx, one) == Some(other.session)
            && !switching(cx, one)
            && shown_tabs(cx, one) == other.tabs
    });
    harness.wait_closed(two);
    assert!(!session_exists(&harness));
    harness.cx.update(|cx| {
        let record = cx.global::<Desktop>().windows.record(one.window_id());
        assert!(
            record
                .unwrap()
                .memory
                .iter()
                .all(|memory| memory.session != harness.session),
            "memory of the closed session's workspaces stayed"
        );
    });
}

#[gpui::test]
fn selecting_an_empty_workspace_shows_the_empty_state(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (empty, _) = harness.add_workspace(0);
    harness.activate_window(window);
    harness.show_workspace(window, empty, &[]);
    harness.update_view(window, |view, window, cx| {
        assert_eq!(view.close.emptied, EmptiedClose::None);
        let shown = view.empty_state_model(cx).expect("the empty state");
        assert_eq!(shown.title(), "No tabs in Workspace 2");
        assert!(shown.switch && shown.close);
        assert!(view.empty_state_focused(window), "New Tab has no focus");
    });
    // A checkpoint and a Quit save the empty workspace as shown.
    let recorder = harness.recorder.clone();
    let before = recorder.saved().len();
    harness.cx.update(take_checkpoint);
    harness.runtime.persistence.flush();
    harness.wait_for("the checkpoint", |_| recorder.saved().len() > before);
    let (_, layout) = recorder.saved().pop().unwrap();
    assert_eq!(layout.windows[0].workspace.get(), empty.get());
    harness.resume_quits();
    harness.cx.update(request_quit);
    harness.wait_quit(harness.session);
    let (_, layout) = harness.recorder.saved().pop().unwrap();
    assert_eq!(layout.windows[0].workspace.get(), empty.get());
}

/// The empty-state button holding focus in `window`, by element id.
fn empty_state_focus(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> Option<&'static str> {
    harness.update_view(window, |view, window, _| {
        let (new_tab, switch, close) = view.empty_state_handles();
        [
            ("empty-new-tab", new_tab),
            ("empty-switch-workspace", switch),
            ("empty-close-workspace", close),
        ]
        .into_iter()
        .find(|(_, handle)| handle.is_focused(window))
        .map(|(name, _)| name)
    })
}

/// Whether `window` shows the workspace switcher's popover, opened from its
/// empty state's Switch Workspace… button and anchored there.
fn workspace_switcher_shown(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> bool {
    use super::super::workspace_panel::{SwitcherAnchor, SwitcherMode};
    harness.update_view(window, |view, _, _| {
        view.switcher_mode
            == Some(SwitcherMode::Popover(SwitcherAnchor::EmptyState))
    })
}

#[gpui::test]
fn dismissed_overlays_return_focus_to_the_empty_state(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (empty, _) = harness.add_workspace(0);
    harness.activate_window(window);
    harness.show_workspace(window, empty, &[]);
    harness.press(window, "tab");
    assert_eq!(
        empty_state_focus(&mut harness, window),
        Some("empty-switch-workspace")
    );
    // Escape from the workspace switcher Switch Workspace… opened returns
    // focus to it, where Tab still moves through the buttons.
    harness.press(window, "enter");
    assert!(workspace_switcher_shown(&mut harness, window));
    harness.press(window, "escape");
    assert!(
        harness.update_view(window, |view, _, _| view.switcher_mode.is_none())
    );
    assert_eq!(
        empty_state_focus(&mut harness, window),
        Some("empty-switch-workspace")
    );
    harness.press(window, "tab");
    assert_eq!(
        empty_state_focus(&mut harness, window),
        Some("empty-close-workspace")
    );
    // A menu a button did not open returns focus to New Tab.
    harness.invoke(window, ids::OPEN_MENU).unwrap();
    assert!(harness.update_view(window, |view, _, _| view.menu.is_some()));
    harness.press(window, "escape");
    assert!(harness.update_view(window, |view, _, _| view.menu.is_none()));
    assert_eq!(
        empty_state_focus(&mut harness, window),
        Some("empty-new-tab")
    );
    harness.press(window, "tab");
    assert_eq!(
        empty_state_focus(&mut harness, window),
        Some("empty-switch-workspace")
    );
}

#[gpui::test]
fn a_cancelled_quit_returns_focus_to_the_empty_state(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let job = harness.open_tab(RUNS_JOB);
    let window = harness.attach_showing(&[job]);
    harness.wait_text(window, job, "READY");
    let (empty, _) = harness.add_workspace(0);
    harness.activate_window(window);
    harness.show_workspace(window, empty, &[]);
    harness.resume_quits();
    // The Quit asks, because the workspace the window left runs a job.
    harness.cx.update(request_quit);
    wait_quit_dialog(&mut harness, window);
    harness.press(window, "escape");
    assert_eq!(confirmation(&mut harness, window), None);
    harness.cx.update(|cx| assert!(!quitting(cx)));
    assert_eq!(
        empty_state_focus(&mut harness, window),
        Some("empty-new-tab")
    );
    harness.press(window, "tab");
    assert_eq!(
        empty_state_focus(&mut harness, window),
        Some("empty-switch-workspace")
    );
}

#[gpui::test]
fn the_empty_state_cycles_its_buttons_and_opens_a_tab(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (empty, _) = harness.add_workspace(0);
    harness.activate_window(window);
    harness.show_workspace(window, empty, &[]);
    let run = |harness: &mut DesktopHarness, command| {
        harness
            .invoke(window, command)
            .expect("the empty state takes the panel command");
    };
    let focused =
        |harness: &mut DesktopHarness| empty_state_focus(harness, window);
    assert_eq!(focused(&mut harness), Some("empty-new-tab"));
    run(&mut harness, ids::PANEL_FOCUS_NEXT);
    assert_eq!(focused(&mut harness), Some("empty-switch-workspace"));
    run(&mut harness, ids::PANEL_FOCUS_NEXT);
    assert_eq!(focused(&mut harness), Some("empty-close-workspace"));
    run(&mut harness, ids::PANEL_FOCUS_NEXT);
    assert_eq!(focused(&mut harness), Some("empty-new-tab"));
    run(&mut harness, ids::PANEL_FOCUS_PREVIOUS);
    assert_eq!(focused(&mut harness), Some("empty-close-workspace"));
    // Switch Workspace… opens the workspace switcher at itself.
    run(&mut harness, ids::PANEL_FOCUS_PREVIOUS);
    run(&mut harness, ids::PANEL_PRESS);
    assert!(workspace_switcher_shown(&mut harness, window));
    harness.press(window, "escape");
    assert!(
        harness.update_view(window, |view, _, _| view.switcher_mode.is_none())
    );
    // Focus returned to Switch Workspace…; New Tab opens a shell in the
    // empty workspace.
    assert_eq!(focused(&mut harness), Some("empty-switch-workspace"));
    run(&mut harness, ids::PANEL_FOCUS_PREVIOUS);
    assert_eq!(focused(&mut harness), Some("empty-new-tab"));
    run(&mut harness, ids::PANEL_PRESS);
    harness.wait_for("the new tab", |cx| shown_tabs(cx, window).len() == 1);
    let opened = harness.cx.update(|cx| shown_tabs(cx, window))[0];
    harness.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, window), Some(empty));
        assert!(empty_state(cx, window).is_none());
        assert!(!exited(cx, window, opened));
    });
}

#[gpui::test]
fn workspace_switches_respect_the_structural_guard(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let job = harness.open_tab(RUNS_JOB);
    let second_tab = harness.open_tab(ECHO);
    let left = harness.first_pane(second_tab);
    harness.split(second_tab, left, SplitDirection::Right, ECHO);
    let window = harness.attach_showing(&[job, second_tab]);
    harness.wait_text(window, job, "READY");
    let (second, seconds) = harness.add_workspace(1);
    harness.wait_projection();
    let refused = |harness: &mut DesktopHarness| {
        for result in [
            harness.select_workspace(window, second),
            harness.invoke(window, ids::NEXT_WORKSPACE),
            harness.invoke(window, ids::NEW_WORKSPACE),
        ] {
            assert!(
                matches!(&result, Err(CommandError::Unavailable(_))),
                "{result:?}"
            );
        }
        harness.cx.update(|cx| {
            assert_eq!(shown_workspace(cx, window), Some(harness.workspace));
        });
    };

    // A close confirmation shows.
    harness.activate(window, job);
    harness.invoke(window, ids::CLOSE_TAB).unwrap();
    harness.wait_for("the close to ask", |cx| confirming(cx, window));
    refused(&mut harness);
    harness.update_view(window, |view, window, cx| {
        view.cancel_confirmation(window, cx);
    });

    // An arrangement waits behind a resize in the layout queue.
    let runtime = Arc::clone(&harness.runtime);
    harness.update_view(window, |view, window, cx| {
        view.select(second_tab, window, cx);
    });
    harness.activate(window, second_tab);
    runtime.worker_hooks.hold(Stage::Layout);
    harness.invoke(window, ids::RESIZE_PANE_RIGHT).unwrap();
    harness.wait_for("the resize to reach its worker", |_| {
        runtime.worker_hooks.held(Stage::Layout) == 1
    });
    harness.invoke(window, ids::BALANCE_PANES).unwrap();
    refused(&mut harness);
    runtime.worker_hooks.release(Stage::Layout);

    // A switch in flight refuses another.
    runtime.worker_hooks.hold(Stage::Subscribe);
    harness.wait_for("the arrangement to end", |cx| harness_idle(cx, window));
    harness.select_workspace(window, second).unwrap();
    assert!(matches!(
        harness.select_workspace(window, harness.workspace),
        Err(CommandError::Unavailable(_))
    ));
    runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_workspace(window, second, &seconds);
}

/// Whether `window` has no arrangement or close in flight.
fn harness_idle(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>().is_ok_and(|view| {
                view.read(cx).check_switch_available(cx).is_ok()
            })
        })
        .unwrap_or(false)
}

#[gpui::test]
fn a_window_leaving_a_workspace_still_counts_as_open(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[first]);
    let (second, seconds) = harness.add_workspace(1);
    harness.show_workspace(one, second, &seconds);
    let other = harness.other_session("other", 1);
    let two = harness.attach_showing(&[first]);
    harness.switch_to(two, other.session, &other.tabs);
    harness.as_linux(two);
    let unviewed = harness.unviewed_session();
    harness.wait_projection();
    harness.resume_quits();
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Commit);
    harness.close_tab_in(second, seconds[0]);
    harness.wait_for("window one's leave to commit", |_| {
        runtime.worker_hooks.held(Stage::Commit) == 1
    });
    harness.cx.update(|cx| {
        let record = cx.global::<Desktop>().windows.record(one.window_id());
        assert!(!record.unwrap().close_committing, "leaving counts as gone");
    });
    // Window two's close sees window one open, so it stays a close.
    harness.update_view(two, |view, window, cx| {
        assert_eq!(
            view.window_close_target(Platform::Linux, cx),
            CloseTarget::Window
        );
        view.close_window(Platform::Linux, window, cx);
    });
    // Window two's close decides at its commit while window one's leave
    // still holds there: the last-window rule ran again and kept a close.
    harness.wait_for("window two's close to reach its commit", |cx| {
        runtime.worker_hooks.held(Stage::Commit) == 2 || quit_underway(cx)
    });
    harness.cx.update(|cx| {
        assert!(!quit_underway(cx), "the close became Quit");
        let record = cx.global::<Desktop>().windows.record(two.window_id());
        assert!(record.unwrap().close_committing);
    });
    runtime.worker_hooks.release(Stage::Commit);
    harness.wait_closed(two);
    harness.wait_workspace(one, harness.workspace, &[first]);
    harness
        .cx
        .update(|cx| assert!(!quitting(cx) && !terminating(cx)));
    assert!(runtime.lock().session(unviewed).is_some());
}

#[gpui::test]
fn a_leave_workspace_commits_while_a_quit_confirms(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[first]);
    let (second, seconds) = harness.add_workspace(1);
    harness.show_workspace(one, second, &seconds);
    let other = harness.other_session_running("other", RUNS_JOB, 1);
    let two = harness.attach_showing(&[first]);
    harness.switch_to(two, other.session, &other.tabs);
    harness.wait_text(two, other.tabs[0], "READY");
    harness.resume_quits();
    harness.activate_window(two);
    let runtime = Arc::clone(&harness.runtime);
    // Window one's emptied close and the Quit both assess before either
    // goes on.
    runtime.worker_hooks.hold(Stage::Assessed);
    harness.close_tab_in(second, seconds[0]);
    harness.wait_for("the emptied close to assess", |_| {
        runtime.worker_hooks.held(Stage::Assessed) == 1
    });
    harness.cx.update(request_quit);
    harness.wait_for("the Quit to assess", |_| {
        runtime.worker_hooks.held(Stage::Assessed) == 2
    });
    // The leave's follow-up waits for its viewers once it commits.
    runtime.worker_hooks.hold(Stage::Subscribe);
    runtime.worker_hooks.release(Stage::Assessed);
    wait_quit_dialog(&mut harness, two);
    harness.wait_for("the leave to follow", |cx| {
        installing(cx, one) && runtime.worker_hooks.held(Stage::Subscribe) > 0
    });
    assert!(
        !harness.workspace_exists(second),
        "the leave did not commit"
    );
    // Confirmed while the follow-up holds it, the Quit waits for it.
    confirm_quit(&mut harness, two);
    harness.cx.update(|cx| {
        assert!(cx.global::<Desktop>().quit_pending);
        assert!(!terminating(cx));
    });
    runtime.worker_hooks.release(Stage::Subscribe);
    // The Quit asks again once the follow-up settled.
    wait_quit_dialog(&mut harness, two);
    confirm_quit(&mut harness, two);
    harness.wait_quit(other.session);
    let (_, layout) = harness.recorder.saved().pop().unwrap();
    let saved = layout
        .windows
        .iter()
        .find(|saved| saved.session.get() == harness.session.get())
        .unwrap();
    assert_eq!(saved.workspace.get(), harness.workspace.get());
}

#[gpui::test]
fn workspace_commands_follow_availability_and_the_palette_prompts_for_a_workspace(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let unavailable = |harness: &mut DesktopHarness, command| {
        harness.update_view(window, |view, _, cx| {
            let target = PaletteTarget {
                session: view.attached_session(cx),
                workspace: view.workspace_id(cx),
                tab: view.active_tab(cx),
                terminal: None,
                terminal_view: None,
                contexts: Vec::new(),
            };
            view.command_availability(command, &target, cx).is_err()
        })
    };
    // One workspace: nothing to cycle to or close, no custom name.
    for command in [
        ids::NEXT_WORKSPACE,
        ids::PREVIOUS_WORKSPACE,
        ids::CLOSE_WORKSPACE,
        ids::CLEAR_WORKSPACE_NAME,
    ] {
        assert!(unavailable(&mut harness, command), "{command}");
    }
    for command in [
        ids::NEW_WORKSPACE,
        ids::SELECT_WORKSPACE,
        ids::RENAME_WORKSPACE,
    ] {
        assert!(!unavailable(&mut harness, command), "{command}");
    }
    let (second, seconds) = harness.add_workspace(1);
    harness
        .runtime
        .lock()
        .rename_workspace(second, Some("build"))
        .unwrap();
    let other = harness.other_session("other", 1);
    // Workspaces number per session, so the other session's first one
    // shares this one's automatic name; a custom name tells them apart.
    harness
        .runtime
        .lock()
        .rename_workspace(other.workspace, Some("elsewhere"))
        .unwrap();
    harness.wait_projection();
    for command in [
        ids::NEXT_WORKSPACE,
        ids::PREVIOUS_WORKSPACE,
        ids::CLOSE_WORKSPACE,
    ] {
        assert!(!unavailable(&mut harness, command), "{command}");
    }

    // The palette asks which workspace, offering only this session's.
    harness.invoke(window, ids::SELECT_WORKSPACE).unwrap();
    harness.wait_for("the workspace prompt", |cx| {
        palette_state(cx, window).is_some_and(|state| {
            state.contains("slots command=select_workspace")
                && state.contains("build")
        })
    });
    harness.cx.update(|cx| {
        let state = palette_state(cx, window).unwrap();
        let other_name = cx
            .global::<Desktop>()
            .hierarchy
            .state()
            .workspace(other.workspace)
            .unwrap()
            .display_name()
            .to_owned();
        assert!(state.contains("picker=2"), "{state}");
        assert!(!state.contains(&other_name), "{state}");
    });
    harness.press(window, "b u i l d");
    harness
        .cx
        .dispatch_keystroke(window, Keystroke::parse("enter").unwrap());
    harness.wait_workspace(window, second, &seconds);
}

#[gpui::test]
fn workspace_names_default_to_the_windows_workspace_and_cycling_wraps(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (second, seconds) = harness.add_workspace(1);
    harness
        .runtime
        .lock()
        .rename_workspace(second, Some("build"))
        .unwrap();
    harness.show_workspace(window, second, &seconds);
    let unavailable = |harness: &mut DesktopHarness, command| {
        harness.update_view(window, |view, _, cx| {
            let target = PaletteTarget {
                session: view.attached_session(cx),
                workspace: view.workspace_id(cx),
                tab: view.active_tab(cx),
                terminal: None,
                terminal_view: None,
                contexts: Vec::new(),
            };
            view.command_availability(command, &target, cx).is_err()
        })
    };
    // Clear Name and Rename default to the window's workspace.
    assert!(!unavailable(&mut harness, ids::CLEAR_WORKSPACE_NAME));
    harness.invoke(window, ids::CLEAR_WORKSPACE_NAME).unwrap();
    let runtime = Arc::clone(&harness.runtime);
    harness.wait_for("the name to clear", |_| {
        runtime
            .lock()
            .workspace(second)
            .unwrap()
            .custom_name()
            .is_none()
    });
    harness
        .invoke_with(
            window,
            &CommandInvocation::new(
                ids::RENAME_WORKSPACE,
                vec![CommandArgument::new(
                    "name",
                    CommandValue::Text("tests".into()),
                )],
            ),
        )
        .unwrap();
    harness.wait_for("the rename", |_| {
        runtime.lock().workspace(second).unwrap().custom_name() == Some("tests")
    });

    // Next and Previous wrap in session order.
    harness.invoke(window, ids::NEXT_WORKSPACE).unwrap();
    harness.wait_workspace(window, harness.workspace, &[tab]);
    harness.invoke(window, ids::PREVIOUS_WORKSPACE).unwrap();
    harness.wait_workspace(window, second, &seconds);

    // New Workspace creates one and shows it.
    harness.invoke(window, ids::NEW_WORKSPACE).unwrap();
    let own = harness.workspace;
    harness.wait_for("the new workspace", |cx| {
        shown_workspace(cx, window)
            .is_some_and(|workspace| workspace != second && workspace != own)
            && !switching(cx, window)
            && shown_tabs(cx, window).len() == 1
    });
    assert_eq!(harness.workspaces().len(), 3);
}
