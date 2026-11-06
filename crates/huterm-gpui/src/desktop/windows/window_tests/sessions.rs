//! Tier 2 tests for session switching: a window shows another session and
//! comes back to its own navigation, two windows on one session switch
//! independently, each failure leaves the window as it was, Close Session
//! and Open in New Window, the last window's reopen and close, and the
//! session commands in the palette.

use huterm_core::RestoreDirection;

use super::pane_moves::{active, pane_focused};
use super::worker_hooks::Stage;
use super::*;

/// A session with its own workspace and one echo tab per name, named
/// `name`, as another client would make it.
pub(super) struct Other {
    pub(super) session: SessionId,
    pub(super) workspace: WorkspaceId,
    pub(super) tabs: Vec<TabId>,
}

impl DesktopHarness {
    pub(super) fn other_session(&self, name: &str, tabs: usize) -> Other {
        let mut mux = self.runtime.lock();
        let session = mux.create_session(Some(name)).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let tabs = (0..tabs)
            .map(|_| mux.open_tab(workspace, &fixture(ECHO)).unwrap().tab.id)
            .collect();
        Other {
            session,
            workspace,
            tabs,
        }
    }

    /// Runs `select_session` for `session` in `window`.
    fn select_session(
        &mut self,
        window: AnyWindowHandle,
        session: SessionId,
    ) -> Result<CommandOutcome, CommandError> {
        self.invoke_with(window, &pick(ids::SELECT_SESSION, session))
    }

    /// Switches `window` to `session` and waits until the switch settled
    /// with every tab in `tabs` installed.
    pub(super) fn switch_to(
        &mut self,
        window: AnyWindowHandle,
        session: SessionId,
        tabs: &[TabId],
    ) {
        self.select_session(window, session).unwrap();
        self.wait_switched(window, session, tabs);
    }

    pub(super) fn wait_switched(
        &mut self,
        window: AnyWindowHandle,
        session: SessionId,
        tabs: &[TabId],
    ) {
        self.wait_for("the switch to settle", |cx| {
            shown_session(cx, window) == Some(session)
                && !switching(cx, window)
                && shown_tabs(cx, window) == tabs
        });
    }

    /// Answers `line` in `tab` of `window` through the view: the shell is
    /// alive and this window's view of it works.
    fn answer(&mut self, window: AnyWindowHandle, tab: TabId, line: &str) {
        self.answers(window, tab, line);
    }

    /// The session's attachments, as the projection holds them.
    fn attachments_of(&mut self, session: SessionId) -> usize {
        self.cx.update(|cx| {
            cx.global::<Desktop>()
                .hierarchy
                .state()
                .session_attachments(session)
                .count()
        })
    }
}

fn pick(
    command: huterm_protocol::CommandId,
    session: SessionId,
) -> CommandInvocation {
    CommandInvocation::new(
        command,
        vec![CommandArgument::new(
            "session",
            CommandValue::Session(session),
        )],
    )
}

/// The session `window`'s record shows.
pub(super) fn shown_session(
    cx: &mut App,
    window: AnyWindowHandle,
) -> Option<SessionId> {
    cx.global::<Desktop>()
        .windows
        .record(window.window_id())
        .and_then(|record| record.session)
}

/// Whether `window` is between a switch's guard and its settle.
fn switching(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>()
                .is_ok_and(|view| view.read(cx).switch.is_some())
        })
        .unwrap_or(false)
}

/// The tabs `window` has views for, in order.
fn shown_tabs(cx: &mut App, window: AnyWindowHandle) -> Vec<TabId> {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>()
                .map(|view| {
                    view.read(cx).tabs.iter().map(|tab| tab.id).collect()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// What `window` would undo next.
fn undo_label(
    cx: &mut App,
    window: AnyWindowHandle,
) -> Option<gpui::SharedString> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx)
                .layout_restore_label(RestoreDirection::Undo, cx)
        })
        .ok()
        .flatten()
}

#[gpui::test]
fn switching_away_and_back_keeps_both_shells_and_this_windows_navigation(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let split = harness.open_tab(ECHO);
    let left = harness.first_pane(split);
    let right = harness.split(split, left, SplitDirection::Right, ECHO);
    let window = harness.attach_showing(&[first, split]);
    harness.answer(window, first, "a1");
    harness.focus(window, split, right);
    // A pane swap the window can undo, in this session only.
    harness.invoke(window, ids::SWAP_PANE_NEXT).unwrap();
    harness.wait_for("the swap to be undoable", |cx| {
        undo_label(cx, window).is_some()
    });
    let other = harness.other_session("other", 2);

    harness.switch_to(window, other.session, &other.tabs);
    assert!(
        harness.cx.update(|cx| undo_label(cx, window)).is_none(),
        "undo reached the session the window left"
    );
    let (b1, b2) = (other.tabs[0], other.tabs[1]);
    harness.answer(window, b2, "b2");
    // Both sessions run; the switch neither closed nor detached either.
    assert_eq!(harness.runtime.lock().sessions().len(), 2);
    assert!(session_exists(&harness));

    harness.switch_to(window, harness.session, &[first, split]);
    harness.cx.update(|cx| {
        assert_eq!(active(cx, window), Some(split));
        let record = cx.global::<Desktop>().windows.record(window.window_id());
        assert_eq!(record.unwrap().history.first(), Some(&split));
        assert_eq!(undo_label(cx, window).as_deref(), Some("Swap Panes"));
    });
    harness.activate_pane(window, split, right);
    harness.answer(window, first, "a1-again");

    // The other session comes back on the tab the window left it on.
    harness.switch_to(window, other.session, &[b1, b2]);
    harness
        .cx
        .update(|cx| assert_eq!(active(cx, window), Some(b2)));
    harness.answer(window, b2, "b2-again");
}

impl DesktopHarness {
    /// Waits until `window` focuses `pane` of `tab`, once it is active.
    fn activate_pane(
        &mut self,
        window: AnyWindowHandle,
        tab: TabId,
        pane: PaneId,
    ) {
        self.cx
            .update(|cx| {
                window.update(cx, |_, window, _| window.activate_window())
            })
            .unwrap();
        self.wait_for("the remembered pane to take focus", |cx| {
            pane_focused(cx, window, tab, pane)
        });
    }
}

#[gpui::test]
fn two_windows_on_one_session_switch_independently(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let second = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[first, second]);
    let two = harness.attach_showing(&[first, second]);
    harness.activate(one, first);
    harness
        .update_view(two, |view, window, cx| view.select(second, window, cx));
    let other = harness.other_session("other", 1);

    harness.switch_to(one, other.session, &other.tabs);
    // The other window keeps the session, its view, and its shell.
    harness.answer(two, second, "still-here");
    harness.switch_to(two, other.session, &other.tabs);
    harness.answer(one, other.tabs[0], "both");
    harness.wait_text(two, other.tabs[0], "got:both");

    harness.switch_to(one, harness.session, &[first, second]);
    harness.switch_to(two, harness.session, &[first, second]);
    harness.cx.update(|cx| {
        assert_eq!(active(cx, one), Some(first));
        assert_eq!(active(cx, two), Some(second));
    });
    assert_eq!(harness.runtime.lock().sessions().len(), 2);
}

#[gpui::test]
fn new_session_switches_to_a_fresh_session_and_keeps_the_old_one(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness
        .invoke_with(
            window,
            &CommandInvocation::new(
                ids::NEW_SESSION,
                vec![CommandArgument::new(
                    "name",
                    CommandValue::Text("build".into()),
                )],
            ),
        )
        .unwrap();
    harness.wait_for("the new session to show", |cx| {
        shown_session(cx, window).is_some_and(|session| {
            cx.global::<Desktop>()
                .hierarchy
                .state()
                .session(session)
                .is_some_and(|info| {
                    info.custom_name.as_deref() == Some("build")
                })
        }) && !switching(cx, window)
            && shown_tabs(cx, window).len() == 1
    });
    assert!(session_exists(&harness));
    assert_eq!(harness.runtime.lock().sessions().len(), 2);
    harness.switch_to(window, harness.session, &[tab]);
    harness.answer(window, tab, "back");
}

#[gpui::test]
fn a_target_closed_before_the_commit_leaves_the_window_with_one_notice(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Switch);
    harness.select_session(window, other.session).unwrap();
    harness.wait_for("the switch to wait for its worker", |_| {
        runtime.worker_hooks.held(Stage::Switch) == 1
    });
    // A second switch waits for the first.
    assert!(matches!(
        harness.select_session(window, other.session),
        Err(CommandError::Unavailable(_))
    ));
    runtime.lock().close_session(other.session).unwrap();
    runtime.worker_hooks.release(Stage::Switch);
    harness.wait_for("the switch to end", |cx| !switching(cx, window));
    harness.cx.update(|cx| {
        assert_eq!(shown_session(cx, window), Some(harness.session));
        let shown = notices(cx, window);
        assert_eq!(
            shown
                .iter()
                .filter(|notice| notice.contains("Session no longer exists"))
                .count(),
            1,
            "{shown:?}"
        );
    });
    harness.answer(window, tab, "unchanged");
}

#[gpui::test]
fn a_window_whose_session_closes_before_the_commit_takes_the_session_gone_path(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Switch);
    harness.select_session(window, other.session).unwrap();
    harness.wait_for("the switch to wait for its worker", |_| {
        runtime.worker_hooks.held(Stage::Switch) == 1
    });
    harness.close_session();
    runtime.worker_hooks.release(Stage::Switch);
    harness.wait_closed(window);
    // The target was never touched.
    assert!(runtime.lock().session(other.session).is_some());
    assert_eq!(harness.close_failures(), 0);
}

#[gpui::test]
fn a_failed_shell_closes_only_what_the_switch_created(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let runtime = Arc::clone(&harness.runtime);
    let failed = |harness: &mut DesktopHarness, title: &str| {
        harness.wait_for("the switch to end", |cx| !switching(cx, window));
        harness.cx.update(|cx| {
            assert_eq!(shown_session(cx, window), Some(harness.session));
            let shown = notices(cx, window);
            assert!(
                shown.iter().any(|notice| notice.contains(title)),
                "{shown:?}"
            );
        });
    };

    // New Session's failed shell closes the session it created.
    runtime.worker_hooks.fail(Stage::Switch);
    harness.invoke(window, ids::NEW_SESSION).unwrap();
    failed(&mut harness, "Cannot create session");
    assert_eq!(runtime.lock().sessions().len(), 1);

    // A target whose workspaces hold no tab needs a shell; a failed one
    // leaves the target, and its empty workspace, as they were.
    let (empty, workspace) = {
        let mut mux = runtime.lock();
        let session = mux.create_session(None).unwrap();
        (session, mux.create_workspace(session, None).unwrap())
    };
    runtime.worker_hooks.fail(Stage::Switch);
    harness.select_session(window, empty).unwrap();
    failed(&mut harness, "Cannot switch session");
    {
        let mux = runtime.lock();
        assert_eq!(mux.session(empty).unwrap().workspaces, [workspace]);
        assert_eq!(mux.workspace(workspace).unwrap().tabs.len(), 0);
    }
    harness.answer(window, tab, "unchanged");

    // Without the failure, the switch opens the shell it needs.
    harness.select_session(window, empty).unwrap();
    harness.wait_for("the empty session's new tab to show", |cx| {
        shown_session(cx, window) == Some(empty)
            && !switching(cx, window)
            && shown_tabs(cx, window).len() == 1
    });
    assert_eq!(runtime.lock().workspace(workspace).unwrap().tabs.len(), 1);
}

#[gpui::test]
fn a_target_closed_after_the_commit_removes_the_window_and_keeps_its_old_session(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Switched);
    harness.select_session(window, other.session).unwrap();
    harness.wait_for("the switch to commit", |_| {
        runtime.worker_hooks.held(Stage::Switched) == 1
    });
    runtime.lock().close_session(other.session).unwrap();
    runtime.worker_hooks.release(Stage::Switched);
    harness.wait_closed(window);
    assert!(session_exists(&harness));
    assert_eq!(harness.close_failures(), 0);
}

#[gpui::test]
fn every_install_failing_keeps_the_attachment_and_its_notices(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    harness.runtime.worker_hooks.fail(Stage::Subscribe);
    harness.select_session(window, other.session).unwrap();
    harness.wait_for("the switch to settle without views", |cx| {
        shown_session(cx, window) == Some(other.session)
            && !switching(cx, window)
            && notices(cx, window)
                .iter()
                .any(|notice| notice.contains("Cannot show tab"))
    });
    harness.wait_projection();
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert!(cx.windows().contains(&window), "the window closed");
        assert_eq!(shown_tabs(cx, window), []);
    });
    // The switch's own removals owed no emptied close.
    assert_eq!(harness.runtime.worker_hooks.entered(Stage::Assess), 0);
    // The notice's retry installs the tab.
    harness.update_view(window, |view, _, cx| {
        view.reconcile(&Touched::everything(), false, cx);
    });
    harness.wait_switched(window, other.session, &other.tabs);
}

#[gpui::test]
fn a_switch_held_at_teardown_changes_nothing(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Switch);
    harness.select_session(window, other.session).unwrap();
    harness.wait_for("the switch to wait for its worker", |_| {
        runtime.worker_hooks.held(Stage::Switch) == 1
    });
    runtime.terminating.store(true, Ordering::Release);
    runtime.worker_hooks.release(Stage::Switch);
    harness.wait_for("the switch to end", |cx| !switching(cx, window));
    harness.cx.update(|cx| {
        assert_eq!(shown_session(cx, window), Some(harness.session));
    });
    runtime.terminate().unwrap();
}

#[gpui::test]
fn a_switch_drops_every_old_view_without_an_emptied_close_and_skips_an_empty_remembered_workspace(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Assess);
    harness.switch_to(window, other.session, &other.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    // No emptied close started, though each switch dropped every view.
    assert_eq!(runtime.worker_hooks.entered(Stage::Assess), 0);
    runtime.worker_hooks.release(Stage::Assess);

    // The remembered workspace lost its tab; another workspace holds one.
    let (later, later_tab) = {
        let mut mux = runtime.lock();
        let later = mux.create_workspace(other.session, None).unwrap();
        let tab = mux.open_tab(later, &fixture(ECHO)).unwrap().tab.id;
        mux.close_tab(other.workspace, other.tabs[0]).unwrap();
        (later, tab)
    };
    harness.switch_to(window, other.session, &[later_tab]);
    harness.cx.update(|cx| {
        let record = cx.global::<Desktop>().windows.record(window.window_id());
        assert_eq!(record.unwrap().workspace, Some(later));
    });
    // No shell opened in the empty workspace.
    assert_eq!(
        runtime
            .lock()
            .workspace(other.workspace)
            .unwrap()
            .tabs
            .len(),
        0
    );
}

#[gpui::test]
fn a_window_close_queued_behind_a_switch_runs_once_it_settles(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Switch);
    harness.select_session(window, other.session).unwrap();
    harness.wait_for("the switch to wait for its worker", |_| {
        runtime.worker_hooks.held(Stage::Switch) == 1
    });
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Window, window, cx);
    });
    assert_eq!(runtime.worker_hooks.entered(Stage::Assess), 0);
    runtime.worker_hooks.release(Stage::Switch);
    harness.wait_closed(window);
    // The close ran on the session the switch reached: the window's own
    // attachment there was the only one, so that session closed, and the
    // session it left stays for another window.
    assert!(runtime.lock().session(other.session).is_none());
    assert!(session_exists(&harness));
}

#[gpui::test]
fn quit_waits_for_a_held_switch_and_saves_both_sessions(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Switch);
    harness.select_session(window, other.session).unwrap();
    harness.wait_for("the switch to wait for its worker", |_| {
        runtime.worker_hooks.held(Stage::Switch) == 1
    });
    // The Quit that waited resumes through the application action, which
    // the production startup registers.
    harness.cx.update(|cx| {
        cx.on_action(|action: &InvokeApp, cx| {
            let _ = Desktop::invoke(cx, &action.0, None);
        });
    });
    harness.cx.update(request_quit);
    harness.cx.update(|cx| {
        assert!(cx.global::<Desktop>().quit_pending);
        assert!(!terminating(cx));
    });
    runtime.worker_hooks.release(Stage::Switch);
    harness.wait_quit(harness.session);
    let (captured, layout) = harness.recorder.saved().remove(0);
    assert_eq!(captured.sessions.len(), 2);
    let saved = &layout.windows[0];
    assert_eq!(saved.session.get(), other.session.get());
    assert_eq!(
        saved
            .remembered
            .iter()
            .map(|memory| memory.session.get())
            .collect::<Vec<_>>(),
        [harness.session.get()]
    );
}

#[gpui::test]
fn quit_from_another_window_waits_for_a_switch_to_restore_its_navigation(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let split = harness.open_tab(ECHO);
    let left = harness.first_pane(split);
    let right = harness.split(split, left, SplitDirection::Right, ECHO);
    let one = harness.attach_showing(&[first, split]);
    harness.focus(one, split, right);
    let other = harness.other_session("other", 1);
    harness.switch_to(one, other.session, &other.tabs);
    let two = harness.attach_showing(&[first, split]);
    harness.switch_to(two, other.session, &other.tabs);
    let runtime = Arc::clone(&harness.runtime);
    // The switch back commits, then its views wait for their viewers.
    runtime.worker_hooks.hold(Stage::Subscribe);
    harness.select_session(one, harness.session).unwrap();
    harness.wait_for("the switch to install its views", |cx| {
        runtime.worker_hooks.held(Stage::Subscribe) > 0
            && one
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>()
                        .is_ok_and(|view| view.read(cx).switch_installing())
                })
                .unwrap_or(false)
    });
    harness.cx.update(|cx| {
        cx.on_action(|action: &InvokeApp, cx| {
            let _ = Desktop::invoke(cx, &action.0, None);
        });
        two.update(cx, |_, window, _| window.activate_window())
            .unwrap();
    });
    harness.cx.update(request_quit);
    harness.cx.update(|cx| {
        assert!(
            cx.global::<Desktop>().quit_pending,
            "Quit ran before the switch restored its navigation"
        );
        assert!(!terminating(cx));
    });
    runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_quit(other.session);
    let (_, layout) = harness.recorder.saved().remove(0);
    let saved = layout
        .windows
        .iter()
        .find(|saved| saved.session.get() == harness.session.get())
        .expect("the switching window was not saved");
    assert_eq!(
        saved.active_tab.map(huterm_protocol::TabId::get),
        Some(split.get())
    );
    assert_eq!(
        saved
            .tab_history
            .iter()
            .map(|tab| tab.get())
            .collect::<Vec<_>>(),
        [first.get(), split.get()]
    );
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
fn a_checkpoint_waits_for_a_held_switch(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Switched);
    harness.select_session(window, other.session).unwrap();
    harness.wait_for("the switch to commit", |_| {
        runtime.worker_hooks.held(Stage::Switched) == 1
    });
    harness.cx.update(|cx| {
        take_checkpoint(cx);
        assert!(cx.global::<Desktop>().checkpoints.waiting_transfer);
    });
    runtime.persistence.flush();
    assert!(harness.recorder.saved().is_empty(), "saved mid-switch");
    runtime.worker_hooks.release(Stage::Switched);
    let recorder = harness.recorder.clone();
    harness.wait_for("the checkpoint after the switch settles", |_| {
        !recorder.saved().is_empty()
    });
    let (_, layout) = recorder.saved().remove(0);
    assert_eq!(layout.windows[0].session.get(), other.session.get());
    runtime.terminate().unwrap();
}

#[gpui::test]
fn transfers_and_arrangements_are_refused_against_a_held_switch(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let second = harness.open_tab(ECHO);
    let left = harness.first_pane(second);
    harness.split(second, left, SplitDirection::Right, ECHO);
    let window = harness.attach_showing(&[first, second]);
    harness.update_view(window, |view, window, cx| {
        view.select(second, window, cx);
    });
    let other = harness.other_session("other", 1);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Switch);
    harness.select_session(window, other.session).unwrap();
    harness.wait_for("the switch to wait for its worker", |_| {
        runtime.worker_hooks.held(Stage::Switch) == 1
    });
    for command in [ids::MOVE_TAB_TO_NEW_WINDOW, ids::BALANCE_PANES] {
        assert!(
            matches!(
                harness.invoke(window, command),
                Err(CommandError::Unavailable(_))
            ),
            "{command}"
        );
    }
    harness
        .cx
        .update(|cx| assert!(cx.global::<Desktop>().cross.reserved.is_empty()));
    runtime.worker_hooks.release(Stage::Switch);
    harness.wait_switched(window, other.session, &other.tabs);
    assert_eq!(runtime.worker_hooks.entered(Stage::Transfer), 0);
    assert_eq!(runtime.worker_hooks.entered(Stage::Layout), 0);
}

#[gpui::test]
fn closing_another_session_removes_its_windows_and_keeps_this_one(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[tab]);
    let two = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    harness.switch_to(two, other.session, &other.tabs);
    harness
        .invoke_with(one, &pick(ids::CLOSE_SESSION, other.session))
        .unwrap();
    harness.wait_closed(two);
    assert!(harness.runtime.lock().session(other.session).is_none());
    harness.answer(one, tab, "kept");
    assert_eq!(harness.close_failures(), 0);
}

#[gpui::test]
fn closing_the_windows_own_session_moves_it_to_the_session_it_left(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let second = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first, second]);
    harness.update_view(window, |view, window, cx| {
        view.select(second, window, cx);
    });
    let other = harness.other_session("other", 1);
    harness.switch_to(window, other.session, &other.tabs);
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    harness.wait_switched(window, harness.session, &[first, second]);
    assert!(harness.runtime.lock().session(other.session).is_none());
    harness.cx.update(|cx| {
        assert_eq!(active(cx, window), Some(second));
        let record = cx.global::<Desktop>().windows.record(window.window_id());
        assert_eq!(record.unwrap().memory.len(), 0);
    });
    harness.answer(window, second, "moved");

    // With no other session it showed, it moves to one no window shows.
    let unviewed = harness.other_session("unviewed", 1);
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    harness.wait_switched(window, unviewed.session, &unviewed.tabs);
    assert!(!session_exists(&harness));
    harness.answer(window, unviewed.tabs[0], "unviewed");

    // With no session left at all, closing its session closes it.
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    harness.wait_closed(window);
    assert_eq!(harness.runtime.lock().sessions(), []);
}

#[gpui::test]
fn a_failed_close_session_follow_up_on_linux_quits_rather_than_strand_sessions(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let unviewed = harness.other_session("unviewed", 1);
    harness.wait_projection();
    let runtime = Arc::clone(&harness.runtime);
    // The window follows to the unviewed session, but that attach fails.
    runtime.worker_hooks.fail(Stage::CloseFollow);
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    if cfg!(target_os = "linux") {
        harness.wait_quit(unviewed.session);
    } else {
        harness.wait_closed(window);
        assert!(
            runtime.lock().session(unviewed.session).is_some(),
            "macOS keeps the unviewed session for a Dock reopen"
        );
    }
    assert!(!session_exists(&harness));
    harness.cx.update(|cx| {
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
}

#[gpui::test]
fn closing_a_session_asks_first_when_its_jobs_run(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let busy = {
        let mut mux = harness.runtime.lock();
        let session = mux.create_session(Some("busy")).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        mux.open_tab(workspace, &fixture(RUNS_JOB)).unwrap();
        session
    };
    harness.wait_projection();
    harness
        .invoke_with(window, &pick(ids::CLOSE_SESSION, busy))
        .unwrap();
    harness.wait_for("the confirmation", |cx| confirming(cx, window));
    let title = harness.update_view(window, |view, _, cx| {
        let target = view.close.confirmation.clone().unwrap();
        build_close_dialog(&view.close_dialog_input(&target, cx)).title
    });
    assert_eq!(title, "Close session \"busy\"?");
    harness.update_view(window, |view, window, cx| {
        view.cancel_confirmation(window, cx);
    });
    harness.wait_for("the confirmation to go", |cx| !confirming(cx, window));
    assert!(harness.runtime.lock().session(busy).is_some());
    harness
        .invoke_with(window, &pick(ids::CLOSE_SESSION, busy))
        .unwrap();
    harness.wait_for("the confirmation", |cx| confirming(cx, window));
    harness.update_view(window, |view, window, cx| {
        let target = view.close.confirmation.clone().unwrap();
        let generation = view.close.generation;
        view.confirm_confirmation(target, generation, window, cx);
    });
    let runtime = Arc::clone(&harness.runtime);
    harness.wait_for("the session to close", |_| {
        runtime.lock().session(busy).is_none()
    });
    harness.answer(window, tab, "kept");
}

#[gpui::test]
fn open_in_new_window_shares_the_session_and_a_failed_open_detaches_only_its_own(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let open = |harness: &mut DesktopHarness| {
        harness.cx.update(|cx| {
            Desktop::invoke(
                cx,
                &pick(ids::OPEN_SESSION_IN_NEW_WINDOW, harness.session),
                None,
            )
        })
    };
    let attached = harness.attachments_of(harness.session);
    let before = harness.cx.update(|cx| cx.windows().len());
    harness
        .cx
        .update(|cx| cx.global_mut::<Desktop>().fail_window_opens = 1);
    open(&mut harness).unwrap();
    let runtime = Arc::clone(&harness.runtime);
    harness.wait_for("the failed open to settle", |cx| {
        cx.global::<Desktop>().pending_spawns == 0
    });
    harness.wait_projection();
    assert_eq!(harness.attachments_of(harness.session), attached);
    assert!(session_exists(&harness));
    assert_eq!(runtime.lock().sessions().len(), 1);

    open(&mut harness).unwrap();
    harness.wait_for("the second window to show the session", |cx| {
        cx.windows().len() == before + 1
            && cx.windows().into_iter().any(|handle| {
                handle != window && terminal(cx, handle, tab).is_some()
            })
    });
    assert_eq!(runtime.lock().sessions().len(), 1);
    assert_eq!(harness.attachments_of(harness.session), attached + 1);

    // A session that closed is a stale pick.
    let other = harness.other_session("other", 1);
    runtime.lock().close_session(other.session).unwrap();
    harness.wait_projection();
    assert_eq!(
        harness.cx.update(|cx| Desktop::invoke(
            cx,
            &pick(ids::OPEN_SESSION_IN_NEW_WINDOW, other.session),
            None,
        )),
        Err(CommandError::StaleTarget)
    );
}

#[gpui::test]
fn a_reopen_after_the_last_window_returns_to_the_session_left_last(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let older = harness.other_session("older", 1);
    let newer = harness.other_session("newer", 1);
    harness.switch_to(window, older.session, &older.tabs);
    harness.switch_to(window, newer.session, &newer.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    // The window's own session closes with it; two the user left remain.
    harness.update_view(window, |view, window, cx| {
        assert_eq!(
            view.window_close_target(Platform::MacOs, cx),
            CloseTarget::Window
        );
        view.request_close(CloseTarget::Window, window, cx);
    });
    harness.wait_closed(window);
    assert!(!session_exists(&harness));
    harness.cx.update(session_switch::reopen_window);
    harness.wait_for("a window on the session left last", |cx| {
        cx.global::<Desktop>()
            .windows
            .records()
            .any(|record| record.session == Some(newer.session))
            && cx
                .windows()
                .into_iter()
                .any(|handle| terminal(cx, handle, newer.tabs[0]).is_some())
    });
    assert_eq!(harness.runtime.lock().sessions().len(), 2);
}

#[gpui::test]
fn a_reopen_whose_session_window_fails_opens_a_fresh_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let left = harness.other_session("left", 1);
    harness.switch_to(window, left.session, &left.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Window, window, cx);
    });
    harness.wait_closed(window);
    harness.runtime.worker_hooks.fail(Stage::OpenSession);
    harness.cx.update(session_switch::reopen_window);
    harness.wait_for("a window on a fresh session", |cx| {
        cx.global::<Desktop>().windows.records().any(|record| {
            record
                .session
                .is_some_and(|session| session != left.session)
        })
    });
    // The session the reopen meant to show keeps running unviewed.
    let mux = harness.runtime.lock();
    assert!(mux.session(left.session).is_some());
    assert_eq!(mux.sessions().len(), 2);
}

#[gpui::test]
fn the_last_windows_close_on_linux_quits_while_unviewed_sessions_remain(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let close_target = |harness: &mut DesktopHarness, platform| {
        harness.update_view(window, |view, _, cx| {
            view.window_close_target(platform, cx)
        })
    };
    assert_eq!(
        close_target(&mut harness, Platform::Linux),
        CloseTarget::Window
    );
    let left = harness.other_session("left", 1);
    harness.switch_to(window, left.session, &left.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    assert_eq!(
        close_target(&mut harness, Platform::Linux),
        CloseTarget::Application
    );
    assert_eq!(
        close_target(&mut harness, Platform::MacOs),
        CloseTarget::Window
    );
    // Another window can still reach the session.
    let two = harness.attach_showing(&[tab]);
    assert_eq!(
        close_target(&mut harness, Platform::Linux),
        CloseTarget::Window
    );
    harness.remove_window(two);
    if cfg!(target_os = "linux") {
        harness.invoke(window, ids::CLOSE_WINDOW).unwrap();
        harness.wait_quit(left.session);
    }
}

#[gpui::test]
fn the_last_windows_emptied_close_on_linux_quits_while_unviewed_sessions_remain(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let emptied_target = |harness: &mut DesktopHarness, platform| {
        harness.update_view(window, |view, _, cx| {
            view.emptied_close_target(platform, cx)
        })
    };
    // With no other session, the window simply closes.
    assert_eq!(
        emptied_target(&mut harness, Platform::Linux),
        CloseTarget::Emptied
    );
    let left = harness.other_session("left", 1);
    harness.switch_to(window, left.session, &left.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    assert_eq!(
        emptied_target(&mut harness, Platform::Linux),
        CloseTarget::Application
    );
    assert_eq!(
        emptied_target(&mut harness, Platform::MacOs),
        CloseTarget::Emptied,
        "macOS keeps running for a Dock reopen"
    );
    // The window's last shell goes, as when it exits.
    harness.close_tab(tab);
    if cfg!(target_os = "linux") {
        harness.wait_quit(left.session);
    } else {
        harness.wait_closed(window);
        assert!(
            harness.runtime.lock().session(left.session).is_some(),
            "the unviewed session keeps running"
        );
    }
}

#[gpui::test]
fn session_commands_follow_availability_and_the_palette_prompts_for_a_session(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    harness.wait_projection();
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
    assert!(unavailable(&mut harness, ids::CLEAR_SESSION_NAME));
    for command in [
        ids::NEW_SESSION,
        ids::SELECT_SESSION,
        ids::CLOSE_SESSION,
        ids::OPEN_SESSION_IN_NEW_WINDOW,
    ] {
        assert!(!unavailable(&mut harness, command), "{command}");
    }
    harness
        .runtime
        .lock()
        .rename_session(harness.session, Some("main"))
        .unwrap();
    harness.wait_projection();
    assert!(!unavailable(&mut harness, ids::CLEAR_SESSION_NAME));
    harness.invoke(window, ids::CLEAR_SESSION_NAME).unwrap();
    let runtime = Arc::clone(&harness.runtime);
    let session = harness.session;
    harness.wait_for("the name to clear", |_| {
        runtime
            .lock()
            .session(session)
            .unwrap()
            .custom_name()
            .is_none()
    });

    // The palette asks which session to switch to, and runs the pick.
    harness.invoke(window, ids::SELECT_SESSION).unwrap();
    harness.wait_for("the session prompt", |cx| {
        palette_state(cx, window).is_some_and(|state| {
            state.contains("slots command=select_session")
                && state.contains("other")
        })
    });
    harness.press(window, "o t h e r");
    harness
        .cx
        .dispatch_keystroke(window, Keystroke::parse("enter").unwrap());
    harness.wait_switched(window, other.session, &other.tabs);

    // Busy windows refuse switches.
    harness.runtime.worker_hooks.hold(Stage::Switch);
    harness.select_session(window, session).unwrap();
    assert!(unavailable(&mut harness, ids::NEW_SESSION));
    assert!(unavailable(&mut harness, ids::SELECT_SESSION));
    harness.runtime.worker_hooks.release(Stage::Switch);
    harness.wait_switched(window, session, &[tab]);
}

#[gpui::test]
fn memory_is_pruned_when_its_session_closes_and_counts_other_windows(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let two = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    harness.switch_to(window, other.session, &other.tabs);
    harness.cx.update(|cx| {
        let windows = &cx.global::<Desktop>().windows;
        assert_eq!(
            windows.other_windows_showing(harness.session, window.window_id()),
            1
        );
        assert_eq!(
            windows.other_windows_showing(other.session, window.window_id()),
            0
        );
        let record = windows.record(window.window_id()).unwrap();
        assert_eq!(record.memory.len(), 1);
    });
    harness.remove_window(two);
    harness.close_session();
    harness.wait_projection();
    harness.cx.update(|cx| {
        let record = cx.global::<Desktop>().windows.record(window.window_id());
        assert_eq!(record.unwrap().memory.len(), 0);
    });
}
