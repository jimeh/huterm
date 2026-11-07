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
        self.other_session_running(name, ECHO, tabs)
    }

    /// [`Self::other_session`] with tabs that run `command`.
    pub(super) fn other_session_running(
        &self,
        name: &str,
        command: &str,
        tabs: usize,
    ) -> Other {
        let mut mux = self.runtime.lock();
        let session = mux.create_session(Some(name)).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let tabs = (0..tabs)
            .map(|_| mux.open_tab(workspace, &fixture(command)).unwrap().tab.id)
            .collect();
        Other {
            session,
            workspace,
            tabs,
        }
    }

    /// Runs `select_session` for `session` in `window`.
    pub(super) fn select_session(
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

    /// Registers the application action a Quit that waited resumes
    /// through, as the production startup does.
    pub(super) fn resume_quits(&mut self) {
        self.cx.update(|cx| {
            cx.on_action(|action: &InvokeApp, cx| {
                let _ = Desktop::invoke(cx, &action.0, None);
            });
        });
    }

    pub(super) fn activate_window(&mut self, window: AnyWindowHandle) {
        self.cx.update(|cx| {
            window
                .update(cx, |_, window, _| window.activate_window())
                .unwrap();
        });
    }

    /// The session's attachments, as the projection holds them.
    pub(super) fn attachments_of(&mut self, session: SessionId) -> usize {
        self.cx.update(|cx| {
            cx.global::<Desktop>()
                .hierarchy
                .state()
                .session_attachments(session)
                .count()
        })
    }
}

pub(super) fn pick(
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
pub(super) fn switching(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>()
                .is_ok_and(|view| view.read(cx).switch.is_some())
        })
        .unwrap_or(false)
}

/// The tabs `window` has views for, in order.
pub(super) fn shown_tabs(cx: &mut App, window: AnyWindowHandle) -> Vec<TabId> {
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
pub(super) fn undo_label(
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
    if Platform::current() == Platform::Linux {
        // The last window quits rather than leave the target running with
        // no window, and the capture shows the target was never touched.
        harness.wait_quit(other.session);
    } else {
        harness.wait_closed(window);
        // The target was never touched.
        assert!(runtime.lock().session(other.session).is_some());
    }
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
    if Platform::current() == Platform::Linux {
        // The last window quits rather than leave its old session running
        // with no window, and the capture keeps that session.
        let session = harness.session;
        harness.wait_quit(session);
    } else {
        harness.wait_closed(window);
        assert!(session_exists(&harness));
    }
    assert_eq!(harness.close_failures(), 0);
}

#[gpui::test]
fn a_target_closed_after_the_commit_on_linux_quits_and_keeps_its_old_session(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.as_linux(window);
    let other = harness.other_session("other", 1);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Switched);
    harness.select_session(window, other.session).unwrap();
    harness.wait_for("the switch to commit", |_| {
        runtime.worker_hooks.held(Stage::Switched) == 1
    });
    runtime.lock().close_session(other.session).unwrap();
    runtime.worker_hooks.release(Stage::Switched);
    // The session-gone path cannot assess its emptied close, whose
    // attachment went with the session, yet the last window still quits
    // rather than leave its old session running with no window.
    let session = harness.session;
    harness.wait_quit(session);
    harness.cx.update(|cx| {
        assert_eq!(
            cx.global::<Desktop>().emptied_quit,
            Some(window.window_id())
        );
    });
    assert_eq!(harness.runtime.worker_hooks.entered(Stage::Commit), 1);
    assert_eq!(harness.close_failures(), 0);
}

#[gpui::test]
fn a_refused_emptied_close_whose_session_then_closes_on_linux_quits(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.as_linux(window);
    let unviewed = harness.unviewed_session();
    harness.wait_projection();
    let runtime = Arc::clone(&harness.runtime);
    let hooks = &runtime.worker_hooks;
    hooks.hold(Stage::Assess);
    harness.close_tab(tab);
    harness.wait_for("the emptied close to assess", |_| {
        hooks.held(Stage::Assess) == 1
    });
    // A tab another client opens makes the runtime refuse the close, and
    // the session closes before the window settles the refusal.
    harness.open_tab(ECHO);
    hooks.hold(Stage::Assessed);
    hooks.release(Stage::Assess);
    harness
        .wait_for("the refusal to hold", |_| hooks.held(Stage::Assessed) == 1);
    harness.close_session();
    hooks.release(Stage::Assessed);
    // The window still quits rather than strand the unviewed session.
    harness.wait_quit(unviewed);
    assert_eq!(hooks.entered(Stage::Commit), 1);
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
    harness.resume_quits();
    runtime.worker_hooks.release(Stage::Switch);
    if Platform::current() == Platform::Linux {
        // The last window would strand the session it left: the close
        // becomes Quit as it commits, and the capture keeps both.
        let session = harness.session;
        harness.wait_quit(session);
        let (captured, _) = harness.recorder.saved().remove(0);
        assert_eq!(captured.sessions.len(), 2);
        return;
    }
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
fn quit_waits_for_a_committing_own_session_close_and_its_follow_up(
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
    let third = harness.other_session("third", 1);
    let two = harness.attach_showing(&[first, split]);
    harness.switch_to(two, third.session, &third.tabs);
    let runtime = Arc::clone(&harness.runtime);
    // Window one closes its own session and will follow to the one it
    // left; its commit has started and waits before the runtime call.
    runtime.worker_hooks.hold(Stage::Commit);
    harness.invoke(one, ids::CLOSE_SESSION).unwrap();
    harness.wait_for("the close to commit", |_| {
        runtime.worker_hooks.held(Stage::Commit) == 1
    });
    harness.resume_quits();
    harness.activate_window(two);
    harness.cx.update(request_quit);
    harness.cx.update(|cx| {
        assert!(
            cx.global::<Desktop>().quit_pending,
            "Quit started while Close Session was committing"
        );
        assert!(!terminating(cx));
    });
    // The follow-up commits, then its views wait for their viewers.
    runtime.worker_hooks.hold(Stage::Subscribe);
    runtime.worker_hooks.release(Stage::Commit);
    harness.wait_for("the follow-up to install its views", |cx| {
        runtime.worker_hooks.held(Stage::Subscribe) > 0
            && one
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>()
                        .is_ok_and(|view| view.read(cx).switch_installing())
                })
                .unwrap_or(false)
    });
    harness.cx.update(|cx| {
        assert!(
            cx.global::<Desktop>().quit_pending,
            "Quit ran before the follow-up restored its navigation"
        );
        assert!(!terminating(cx));
    });
    runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_quit(third.session);
    assert!(runtime.lock().session(other.session).is_none());
    let (_, layout) = harness.recorder.saved().remove(0);
    let saved = layout
        .windows
        .iter()
        .find(|saved| saved.session.get() == harness.session.get())
        .expect("the window that followed was not saved on its new session");
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
fn a_quit_cancels_an_assessing_own_session_close(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    let two = harness.attach_showing(&[tab]);
    harness.switch_to(two, other.session, &other.tabs);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Assess);
    harness.invoke(one, ids::CLOSE_SESSION).unwrap();
    harness.wait_for("the close to assess", |_| {
        runtime.worker_hooks.held(Stage::Assess) == 1
    });
    harness.resume_quits();
    harness.activate_window(two);
    harness.cx.update(request_quit);
    harness.cx.update(|cx| {
        let desktop = cx.global::<Desktop>();
        assert!(
            !desktop.quit_pending,
            "Quit waited for a close that had not started committing"
        );
        assert_eq!(desktop.pending_spawns, 0);
    });
    harness.update_view(one, |view, _, _| {
        assert_eq!(view.close.current, None, "the close was not cancelled");
        assert!(!view.busy);
    });
    // The cancelled assessment answers into nothing; the Quit's proceeds.
    runtime.worker_hooks.release(Stage::Assess);
    let session = harness.session;
    harness.wait_quit(session);
    assert_eq!(
        runtime.worker_hooks.entered(Stage::Commit),
        1,
        "only the Quit committed"
    );
    let (_, layout) = harness.recorder.saved().remove(0);
    assert!(
        layout
            .windows
            .iter()
            .any(|saved| saved.session.get() == session.get()),
        "the cancelled close's window was not saved on its session"
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

    // With no session left at all, closing its session closes it, as
    // settled decision 5 says; no fresh session opens.
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    harness.wait_closed(window);
    assert_eq!(harness.runtime.lock().sessions(), []);
}

#[gpui::test]
fn a_close_session_follow_up_whose_first_candidate_closed_lands_on_the_next(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let left = harness.other_session("left", 1);
    harness.switch_to(window, left.session, &left.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    let unviewed = harness.other_session("unviewed", 1);
    harness.wait_projection();
    let runtime = Arc::clone(&harness.runtime);
    // The window would follow to the session it left, which closes after
    // the commit and before the follow-up attaches.
    runtime.worker_hooks.hold(Stage::CloseFollow);
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    harness.wait_for("the follow-up to wait", |_| {
        runtime.worker_hooks.held(Stage::CloseFollow) == 1
    });
    runtime.lock().close_session(left.session).unwrap();
    runtime.worker_hooks.release(Stage::CloseFollow);
    harness.wait_switched(window, unviewed.session, &unviewed.tabs);
    assert!(!session_exists(&harness));
    harness.answer(window, unviewed.tabs[0], "followed");
    harness.cx.update(|cx| {
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
}

#[gpui::test]
fn a_close_session_follow_up_that_cannot_attach_opens_a_fresh_session(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let left = harness.other_session("left", 1);
    harness.switch_to(window, left.session, &left.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    let unviewed = harness.other_session("unviewed", 1);
    harness.wait_projection();
    let runtime = Arc::clone(&harness.runtime);
    let fell_back = |cx: &mut App| {
        notices(cx, window)
            .iter()
            .any(|notice| notice.contains("Opened a new session"))
    };

    // The session it left cannot be attached; the window moves on to the
    // next candidate without a notice.
    runtime.worker_hooks.fail(Stage::CloseFollow);
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    harness.wait_switched(window, unviewed.session, &unviewed.tabs);
    assert!(!session_exists(&harness));
    harness.cx.update(|cx| assert!(!fell_back(cx)));

    // Its only candidate fails too: a fresh session, with a notice.
    runtime.worker_hooks.fail(Stage::CloseFollow);
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    harness.wait_for("a fresh session to show", |cx| {
        shown_session(cx, window).is_some_and(|session| {
            session != left.session && session != unviewed.session
        }) && !switching(cx, window)
            && shown_tabs(cx, window).len() == 1
    });
    harness.cx.update(|cx| {
        let shown = notices(cx, window);
        assert!(
            shown.iter().any(|notice| {
                notice.contains("Cannot show session \"left\"")
                    && notice.contains("Opened a new session")
            }),
            "{shown:?}"
        );
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
    let mux = runtime.lock();
    assert!(mux.session(left.session).is_some(), "the failed one stays");
    assert!(mux.session(unviewed.session).is_none());
    assert_eq!(mux.sessions().len(), 2);
}

#[gpui::test]
fn a_close_session_follow_up_that_opens_nothing_follows_the_last_window_rule(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let unviewed = harness.other_session("unviewed", 1);
    harness.wait_projection();
    let runtime = Arc::clone(&harness.runtime);
    // The unviewed session's attach fails, and so does the fresh session's
    // shell.
    runtime.worker_hooks.fail_times(Stage::CloseFollow, 2);
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    if Platform::current() == Platform::Linux {
        // The last window quits rather than strand the unviewed session.
        harness.wait_quit(unviewed.session);
    } else {
        harness.wait_closed(window);
        assert!(
            runtime.lock().session(unviewed.session).is_some(),
            "macOS keeps the unviewed session for a Dock reopen"
        );
        // The next window shows why this one closed.
        harness.cx.update(|cx| {
            let latched = &cx.global::<Desktop>().latched;
            assert!(
                latched.iter().any(|notice| {
                    notice.message.contains("Cannot open a new session")
                }),
                "{latched:?}"
            );
        });
    }
    assert!(!session_exists(&harness));
    harness.cx.update(|cx| {
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
}

#[gpui::test]
fn closing_a_shared_session_moves_the_closing_window_and_closes_the_other(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[tab]);
    let left = harness.other_session("left", 1);
    harness.switch_to(one, left.session, &left.tabs);
    harness.switch_to(one, harness.session, &[tab]);
    let two = harness.attach_showing(&[tab]);
    harness.invoke(one, ids::CLOSE_SESSION).unwrap();
    harness.wait_closed(two);
    harness.wait_switched(one, left.session, &left.tabs);
    assert!(!session_exists(&harness));
    harness.answer(one, left.tabs[0], "followed");
    assert_eq!(harness.close_failures(), 0);
    harness.cx.update(|cx| {
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
}

#[gpui::test]
fn a_checkpoint_waits_for_a_close_session_follow_up(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let left = harness.other_session("left", 1);
    harness.switch_to(window, left.session, &left.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    let runtime = Arc::clone(&harness.runtime);
    // The follow-up's views wait for their viewers.
    runtime.worker_hooks.hold(Stage::Subscribe);
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    harness.wait_for("the follow-up to install its views", |cx| {
        runtime.worker_hooks.held(Stage::Subscribe) > 0
            && window
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>()
                        .is_ok_and(|view| view.read(cx).switch_installing())
                })
                .unwrap_or(false)
    });
    harness.cx.update(|cx| {
        take_checkpoint(cx);
        assert!(cx.global::<Desktop>().checkpoints.waiting_transfer);
    });
    runtime.persistence.flush();
    assert!(harness.recorder.saved().is_empty(), "saved mid-follow-up");
    runtime.worker_hooks.release(Stage::Subscribe);
    let recorder = harness.recorder.clone();
    harness.wait_for("the checkpoint after the follow-up settles", |_| {
        !recorder.saved().is_empty()
    });
    let (_, layout) = recorder.saved().remove(0);
    assert_eq!(layout.windows[0].session.get(), left.session.get());
    runtime.terminate().unwrap();
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
fn an_own_session_close_confirmation_holds_no_quit(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(RUNS_JOB);
    let window = harness.attach_showing(&[tab]);
    harness.other_session("other", 1);
    harness.wait_projection();
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    harness.wait_for("the confirmation", |cx| confirming(cx, window));
    harness.cx.update(|cx| {
        assert_eq!(
            cx.global::<Desktop>().pending_spawns,
            0,
            "the close holds Quit only once it commits"
        );
    });
    harness.update_view(window, |view, window, cx| {
        view.cancel_confirmation(window, cx);
    });
    harness.cx.update(|cx| {
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
    assert!(session_exists(&harness));
}

/// Two windows: `one` shows a tab that runs a job in the harness session,
/// so closing that session asks first, and `two` shows another session's
/// echo tab and is active. The Quit's resume action is registered, as the
/// production startup does.
fn own_session_close_windows(
    harness: &mut DesktopHarness,
) -> (AnyWindowHandle, AnyWindowHandle, Other) {
    let tab = harness.open_tab(RUNS_JOB);
    let one = harness.attach_showing(&[tab]);
    harness.wait_text(one, tab, "READY");
    let other = harness.other_session("other", 1);
    let two = harness.attach_showing(&[tab]);
    harness.switch_to(two, other.session, &other.tabs);
    harness.cx.update(|cx| {
        cx.on_action(|action: &InvokeApp, cx| {
            let _ = Desktop::invoke(cx, &action.0, None);
        });
        two.update(cx, |_, window, _| window.activate_window())
            .unwrap();
    });
    (one, two, other)
}

/// The confirmation `window` shows, if any.
pub(super) fn confirmation(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> Option<CloseTarget> {
    harness.update_view(window, |view, _, _| view.close.confirmation.clone())
}

/// Waits until `window` shows Quit's confirmation.
pub(super) fn wait_quit_dialog(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) {
    harness.wait_for("the Quit dialog", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx).close.confirmation
                        == Some(CloseTarget::Application)
                })
            })
            .unwrap_or(false)
    });
}

#[gpui::test]
fn a_quit_replaces_an_own_session_close_confirmation_in_its_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (one, two, _) = own_session_close_windows(&mut harness);
    let own = CloseTarget::Session(harness.session);
    harness.invoke(one, ids::CLOSE_SESSION).unwrap();
    harness.wait_for("the confirmation", |cx| confirming(cx, one));
    assert_eq!(confirmation(&mut harness, one), Some(own));
    // A Quit from the same window dismisses the Close Session dialog and
    // shows its own there.
    harness.activate_window(one);
    harness.cx.update(request_quit);
    wait_quit_dialog(&mut harness, one);
    harness.cx.update(|cx| {
        let desktop = cx.global::<Desktop>();
        assert!(!desktop.quit_pending);
        assert_eq!(desktop.pending_spawns, 0);
    });
    assert_eq!(confirmation(&mut harness, two), None);
    harness.update_view(one, |view, window, cx| {
        let generation = view.close.generation;
        view.confirm_confirmation(
            CloseTarget::Application,
            generation,
            window,
            cx,
        );
    });
    // The Quit, not the Close Session it replaced, ended the session: the
    // capture still holds it.
    let session = harness.session;
    harness.wait_quit(session);
}

#[gpui::test]
fn closing_the_windows_own_session_is_refused_while_a_quit_runs(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (one, two, other) = own_session_close_windows(&mut harness);
    // Window one hosts a Quit that asks first.
    harness.cx.update(|cx| {
        one.update(cx, |_, window, _| window.activate_window())
            .unwrap();
    });
    harness.cx.update(request_quit);
    harness.wait_for("the Quit dialog", |cx| confirming(cx, one));
    // Window two may not close its own session meanwhile, from the
    // command or as its menu row shows it.
    let refused = harness.invoke(two, ids::CLOSE_SESSION);
    assert!(
        matches!(&refused, Err(CommandError::Unavailable(reason))
            if reason.contains("quitting")),
        "{refused:?}"
    );
    let available = harness.update_view(two, |view, _, cx| {
        let target = PaletteTarget {
            session: view.attached_session(cx),
            workspace: view.workspace_id(cx),
            tab: view.active_tab(cx),
            terminal: None,
            terminal_view: None,
            contexts: Vec::new(),
        };
        view.command_availability(ids::CLOSE_SESSION, &target, cx)
    });
    assert!(available.is_err(), "{available:?}");
    assert_eq!(confirmation(&mut harness, two), None);
    assert!(harness.runtime.lock().session(other.session).is_some());
}

#[gpui::test]
fn a_quit_dismisses_every_own_session_close_confirmation(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(RUNS_JOB);
    let one = harness.attach_showing(&[tab]);
    harness.wait_text(one, tab, "READY");
    let busy = harness.other_session_running("busy", RUNS_JOB, 1);
    let two = harness.attach_showing(&[tab]);
    harness.switch_to(two, busy.session, &busy.tabs);
    harness.wait_text(two, busy.tabs[0], "READY");
    harness.resume_quits();
    let mut drawn = Vec::new();
    for window in [one, two] {
        harness.invoke(window, ids::CLOSE_SESSION).unwrap();
        harness.wait_for("the confirmation", |cx| confirming(cx, window));
        drawn.push(harness.update_view(window, |view, _, _| {
            (
                view.close.confirmation.clone().unwrap(),
                view.close.generation,
            )
        }));
    }
    harness.activate_window(two);
    harness.cx.update(request_quit);
    wait_quit_dialog(&mut harness, two);
    assert_eq!(confirmation(&mut harness, one), None);
    harness.cx.update(|cx| {
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
    // A click on either dismissed dialog, drawn before the Quit, commits
    // nothing.
    harness.update_view(one, |view, window, cx| {
        let (target, generation) = drawn[0].clone();
        view.confirm_confirmation(target, generation, window, cx);
    });
    let runtime = Arc::clone(&harness.runtime);
    assert_eq!(runtime.worker_hooks.entered(Stage::Commit), 0);
    assert_eq!(
        confirmation(&mut harness, two),
        Some(CloseTarget::Application)
    );
    harness.update_view(two, |view, window, cx| {
        let generation = view.close.generation;
        view.confirm_confirmation(
            CloseTarget::Application,
            generation,
            window,
            cx,
        );
    });
    let session = harness.session;
    harness.wait_quit(session);
    let (captured, _) = harness.recorder.saved().remove(0);
    assert!(
        captured
            .sessions
            .iter()
            .any(|kept| kept.info.id.get() == busy.session.get())
    );
}

#[gpui::test]
fn a_quit_drops_a_queued_own_session_close(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    let two = harness.attach_showing(&[tab]);
    harness.switch_to(two, other.session, &other.tabs);
    harness.resume_quits();
    let runtime = Arc::clone(&harness.runtime);
    // Window one is busy with a new tab whose shell waits.
    runtime.worker_hooks.hold(Stage::Spawned);
    harness.update_view(one, |view, window, cx| {
        view.new_tab(window, cx).unwrap();
    });
    harness.wait_for("the new tab to spawn", |_| {
        runtime.worker_hooks.held(Stage::Spawned) == 1
    });
    let own = CloseTarget::Session(harness.session);
    harness.update_view(one, |view, window, cx| {
        view.request_close(own.clone(), window, cx);
        assert_eq!(view.close.pending, Some(own.clone()), "not queued");
    });
    harness.activate_window(two);
    harness.cx.update(request_quit);
    harness.update_view(one, |view, _, _| {
        assert_eq!(view.close.pending, None, "the queued close stayed");
    });
    harness.cx.update(|cx| {
        let desktop = cx.global::<Desktop>();
        assert!(desktop.quit_pending, "Quit did not wait for the spawn");
        assert_eq!(desktop.pending_spawns, 1, "only the spawn holds it");
    });
    runtime.worker_hooks.release(Stage::Spawned);
    // The new tab runs the configured login shell, which may still be
    // starting when Quit assesses, so Quit may ask; answer it if it does.
    harness.wait_for("the Quit to commit or ask", |cx| {
        runtime.terminating.load(Ordering::Acquire)
            || [one, two].iter().any(|window| {
                window
                    .update(cx, |root, _, cx| {
                        root.downcast::<WorkspaceView>().is_ok_and(|view| {
                            view.read(cx).close.confirmation
                                == Some(CloseTarget::Application)
                        })
                    })
                    .unwrap_or(false)
            })
    });
    for window in [one, two] {
        if confirmation(&mut harness, window) == Some(CloseTarget::Application)
        {
            confirm_quit(&mut harness, window);
        }
    }
    let session = harness.session;
    harness.wait_quit(session);
    assert_eq!(
        runtime.worker_hooks.entered(Stage::Commit),
        1,
        "only the Quit committed"
    );
}

#[gpui::test]
fn an_own_session_close_refuses_to_commit_while_a_quit_runs(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (one, two, other) = own_session_close_windows(&mut harness);
    let own = CloseTarget::Session(harness.session);
    harness.invoke(one, ids::CLOSE_SESSION).unwrap();
    harness.wait_for("the confirmation", |cx| confirming(cx, one));
    let drawn = harness.update_view(one, |view, _, _| view.close.generation);
    // Window two hosts the Quit, which asks because one's tab runs a job.
    harness.cx.update(request_quit);
    wait_quit_dialog(&mut harness, two);
    assert_eq!(confirmation(&mut harness, one), None);
    let runtime = Arc::clone(&harness.runtime);
    let assessed = runtime.worker_hooks.entered(Stage::Assess);
    // Neither a click drawn before the dismissal nor a commit that reaches
    // the pipeline anyway closes the session or asks again.
    harness.update_view(one, |view, window, cx| {
        view.confirm_confirmation(own.clone(), drawn, window, cx);
        view.finish_close(own.clone(), window, cx);
        assert!(!view.busy);
    });
    harness.cx.run_until_parked();
    assert_eq!(runtime.worker_hooks.entered(Stage::Assess), assessed);
    assert_eq!(runtime.worker_hooks.entered(Stage::Commit), 0);
    assert_eq!(confirmation(&mut harness, one), None);
    harness.cx.update(|cx| {
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
    harness.update_view(two, |view, window, cx| {
        view.cancel_confirmation(window, cx);
    });
    assert!(session_exists(&harness));
    harness.answer(two, other.tabs[0], "after-cancel");
}

#[gpui::test]
fn a_quit_reassesses_after_another_sessions_close_commits(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(RUNS_JOB);
    let one = harness.attach_showing(&[tab]);
    harness.wait_text(one, tab, "READY");
    let two = harness.attach_showing(&[tab]);
    let busy = harness.other_session_running("busy", RUNS_JOB, 1);
    harness.wait_projection();
    harness.resume_quits();
    harness
        .invoke_with(one, &pick(ids::CLOSE_SESSION, busy.session))
        .unwrap();
    harness.wait_for("the confirmation", |cx| confirming(cx, one));
    let other_close = harness.update_view(one, |view, _, _| {
        (
            view.close.confirmation.clone().unwrap(),
            view.close.generation,
        )
    });
    harness.activate_window(two);
    harness.cx.update(request_quit);
    wait_quit_dialog(&mut harness, two);
    // The other session's close is any other close: it still asks, and
    // it commits.
    assert_eq!(
        confirmation(&mut harness, one),
        Some(CloseTarget::Session(busy.session))
    );
    harness.update_view(one, |view, window, cx| {
        let (target, generation) = other_close.clone();
        view.confirm_confirmation(target, generation, window, cx);
    });
    let runtime = Arc::clone(&harness.runtime);
    harness.wait_for("the other session to close", |_| {
        runtime.lock().session(busy.session).is_none()
    });
    harness.wait_projection();
    // The Quit's commit went stale, so it asks again.
    let (asked, assessed) = harness.update_view(two, |view, window, cx| {
        let generation = view.close.generation;
        view.confirm_confirmation(
            CloseTarget::Application,
            generation,
            window,
            cx,
        );
        (generation, runtime.worker_hooks.entered(Stage::Assess))
    });
    harness.wait_for("the Quit to ask again", |cx| {
        two.update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>().is_ok_and(|view| {
                let close = &view.read(cx).close;
                close.confirmation == Some(CloseTarget::Application)
                    && close.generation > asked
            })
        })
        .unwrap_or(false)
    });
    assert!(runtime.worker_hooks.entered(Stage::Assess) > assessed);
    harness.update_view(two, |view, window, cx| {
        let generation = view.close.generation;
        view.confirm_confirmation(
            CloseTarget::Application,
            generation,
            window,
            cx,
        );
    });
    let session = harness.session;
    harness.wait_quit(session);
}

#[gpui::test]
fn a_cancelled_quit_leaves_its_cancelled_closes_cancelled(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (one, two, other) = own_session_close_windows(&mut harness);
    harness.invoke(one, ids::CLOSE_SESSION).unwrap();
    harness.wait_for("the confirmation", |cx| confirming(cx, one));
    harness.cx.update(request_quit);
    wait_quit_dialog(&mut harness, two);
    harness.update_view(two, |view, window, cx| {
        view.cancel_confirmation(window, cx);
    });
    harness.cx.update(|cx| {
        let desktop = cx.global::<Desktop>();
        assert!(!desktop.quitting);
        assert_eq!(desktop.pending_spawns, 0);
    });
    harness.cx.run_until_parked();
    assert_eq!(confirmation(&mut harness, one), None);
    assert!(!harness.update_view(one, |view, _, _| view.busy));
    assert!(session_exists(&harness));
    let runtime = Arc::clone(&harness.runtime);
    assert_eq!(runtime.worker_hooks.entered(Stage::Commit), 0);
    // Both windows keep their sessions and work.
    harness.answer(two, other.tabs[0], "after-cancel");
    harness.cx.update(|cx| {
        assert_eq!(shown_session(cx, one), Some(harness.session));
    });
    harness.invoke(one, ids::CLOSE_SESSION).unwrap();
    harness.wait_for("the confirmation again", |cx| confirming(cx, one));
}

#[gpui::test]
fn session_changes_are_refused_while_a_quit_runs(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let (one, two, _) = own_session_close_windows(&mut harness);
    let session = harness.session;
    let refused = |harness: &mut DesktopHarness| {
        let results = [
            harness.select_session(two, session),
            harness.invoke(two, ids::NEW_SESSION),
            harness.cx.update(|cx| {
                Desktop::invoke(
                    cx,
                    &pick(ids::OPEN_SESSION_IN_NEW_WINDOW, session),
                    None,
                )
            }),
        ];
        for result in results {
            assert!(
                matches!(&result, Err(CommandError::Unavailable(reason))
                    if reason.contains("quitting")),
                "{result:?}"
            );
        }
    };
    // Requested: the Quit waits for a pending spawn.
    let id = one.window_id();
    harness.activate_window(one);
    harness.cx.update(|cx| {
        session_switch::hold_switch_spawn(cx, id);
        request_quit(cx);
        assert!(cx.global::<Desktop>().quit_pending);
    });
    refused(&mut harness);
    // Running: the Quit asks in window one.
    harness
        .cx
        .update(|cx| session_switch::release_switch_spawn(cx, id));
    wait_quit_dialog(&mut harness, one);
    refused(&mut harness);
    harness.cx.update(|cx| {
        assert_eq!(shown_session(cx, two).map(|s| s != session), Some(true));
    });
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
    if Platform::current() == Platform::Linux {
        // Linux quits instead, as it commits; only macOS reopens.
        harness.wait_quit(newer.session);
        return;
    }
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
    if Platform::current() == Platform::Linux {
        // Linux quits instead, as it commits; only macOS reopens.
        harness.wait_quit(left.session);
        return;
    }
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
    if Platform::current() == Platform::Linux {
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
    if Platform::current() == Platform::Linux {
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
fn the_last_windows_close_on_linux_stays_a_close_while_a_quit_waits(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let left = harness.other_session("left", 1);
    harness.switch_to(window, left.session, &left.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    let linux_targets = |harness: &mut DesktopHarness| {
        harness.update_view(window, |view, _, cx| {
            (
                view.window_close_target(Platform::Linux, cx),
                view.emptied_close_target(Platform::Linux, cx),
            )
        })
    };
    assert_eq!(
        linux_targets(&mut harness),
        (CloseTarget::Application, CloseTarget::Application)
    );
    // A Quit waits for a pending spawn: a hold that the window's removal
    // releases. It resumes through the application action, which the
    // production startup registers.
    let id = window.window_id();
    harness.cx.update(|cx| {
        cx.on_action(|action: &InvokeApp, cx| {
            let _ = Desktop::invoke(cx, &action.0, None);
        });
        session_switch::hold_switch_spawn(cx, id);
        request_quit(cx);
        assert!(cx.global::<Desktop>().quit_pending);
    });
    // That Quit covers every session, so neither close turns into one.
    assert_eq!(
        linux_targets(&mut harness),
        (CloseTarget::Window, CloseTarget::Emptied)
    );
    harness.close_tab(tab);
    harness.wait_closed(window);
    harness.wait_quit(left.session);
}

#[gpui::test]
fn the_last_windows_close_on_linux_waits_for_a_pending_new_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let left = harness.other_session("left", 1);
    harness.switch_to(window, left.session, &left.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    harness.resume_quits();
    let runtime = Arc::clone(&harness.runtime);
    // A window on the session the user left is still opening.
    runtime.worker_hooks.hold(Stage::OpenSession);
    harness
        .cx
        .update(|cx| {
            Desktop::invoke(
                cx,
                &pick(ids::OPEN_SESSION_IN_NEW_WINDOW, left.session),
                None,
            )
        })
        .unwrap();
    harness.wait_for("the new window to wait for its worker", |_| {
        runtime.worker_hooks.held(Stage::OpenSession) == 1
    });
    harness.update_view(window, |view, window, cx| {
        view.close_window(Platform::Linux, window, cx);
    });
    harness.wait_for("the Quit to wait for the new window", |cx| {
        cx.global::<Desktop>().quit_pending
    });
    harness.cx.update(|cx| assert!(!terminating(cx)));
    runtime.worker_hooks.release(Stage::OpenSession);
    harness.wait_quit(left.session);
    let (_, layout) = harness.recorder.saved().remove(0);
    assert!(
        layout
            .windows
            .iter()
            .any(|saved| saved.session.get() == left.session.get()),
        "the new window was not saved"
    );
}

#[gpui::test]
fn cancelling_the_emptied_windows_quit_on_linux_shows_the_session_left_last(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    // The session left last runs a job, so the Quit asks.
    let older = harness.other_session("older", 1);
    let left = harness.other_session_running("left", RUNS_JOB, 1);
    harness.switch_to(window, older.session, &older.tabs);
    harness.switch_to(window, left.session, &left.tabs);
    harness.wait_text(window, left.tabs[0], "READY");
    harness.switch_to(window, harness.session, &[tab]);
    harness.resume_quits();
    // The window's last shell goes, as when it exits.
    harness.close_tab(tab);
    if Platform::current() == Platform::MacOs {
        // macOS keeps running for a Dock reopen.
        harness.wait_closed(window);
        harness.cx.update(|cx| assert!(!terminating(cx)));
        return;
    }
    wait_quit_dialog(&mut harness, window);
    harness.update_view(window, |view, window, cx| {
        view.cancel_confirmation(window, cx);
    });
    harness.wait_switched(window, left.session, &left.tabs);
    harness.cx.update(|cx| {
        let desktop = cx.global::<Desktop>();
        assert!(!desktop.quitting);
        assert_eq!(desktop.pending_spawns, 0);
        let record = desktop.windows.record(window.window_id()).unwrap();
        assert!(record.attachment.is_some());
    });
    // The emptied session lost this window's attachment.
    assert_eq!(harness.attachments_of(harness.session), 0);
    assert!(harness.runtime.lock().session(older.session).is_some());
}

#[gpui::test]
fn quit_waits_for_a_new_windows_pending_shell(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    harness.attach_showing(&[tab]);
    harness.resume_quits();
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Spawned);
    harness
        .cx
        .update(|cx| {
            Desktop::invoke(
                cx,
                &CommandInvocation::new(ids::NEW_WINDOW, Vec::new()),
                None,
            )
        })
        .unwrap();
    harness.wait_for("the new window's shell to spawn", |_| {
        runtime.worker_hooks.held(Stage::Spawned) == 1
    });
    harness.cx.update(request_quit);
    harness.cx.update(|cx| {
        assert!(cx.global::<Desktop>().quit_pending);
        assert!(!terminating(cx));
    });
    runtime.worker_hooks.release(Stage::Spawned);
    let session = harness.session;
    harness.wait_quit(session);
    let (captured, _) = harness.recorder.saved().remove(0);
    assert_eq!(captured.sessions.len(), 2, "the new window's session");
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

/// Whether `window` is installing a switch or follow-up.
pub(super) fn installing(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>()
                .is_ok_and(|view| view.read(cx).switch_installing())
        })
        .unwrap_or(false)
}

/// The confirmation any open window shows, with that window.
fn any_confirmation(cx: &mut App) -> Option<(AnyWindowHandle, CloseTarget)> {
    cx.windows().into_iter().find_map(|handle| {
        handle
            .update(cx, |root, _, cx| {
                let view = root.downcast::<WorkspaceView>().ok()?;
                view.read(cx).close.confirmation.clone()
            })
            .ok()
            .flatten()
            .map(|target| (handle, target))
    })
}

/// Confirms the Quit `window` asks about.
pub(super) fn confirm_quit(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) {
    harness.update_view(window, |view, window, cx| {
        let generation = view.close.generation;
        view.confirm_confirmation(
            CloseTarget::Application,
            generation,
            window,
            cx,
        );
    });
}

/// Draws `window`'s title row as on Linux with client decorations and
/// returns the centre of its Close button.
fn drawn_close_button(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> gpui::Point<Pixels> {
    let painted = |cx: &mut App| {
        window
            .update(cx, |root, _, cx| {
                let view = root.downcast::<WorkspaceView>().ok()?;
                view.read(cx).window_button_bounds[WindowButton::Close.index()]
                    .get()
            })
            .ok()
            .flatten()
    };
    harness.update_view(window, |view, window, cx| {
        view.decorations_override = Some(gpui::Decorations::Client {
            tiling: gpui::Tiling::default(),
        });
        view.sync_frame(window);
        view.sync_tab_layout(window, cx);
        cx.notify();
    });
    harness.wait_for("the drawn Close button", |cx| painted(cx).is_some());
    harness.cx.update(painted).unwrap().center()
}

#[gpui::test]
fn the_drawn_close_button_on_linux_quits_while_unviewed_sessions_remain(
    cx: &mut TestAppContext,
) {
    // The title row Huterm draws holds the window buttons.
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.position = crate::config::TabPosition::Titlebar;
    });
    // The tab runs a job, so a Window close and a Quit each ask, and the
    // dialog shows which one the button started.
    let tab = harness.open_tab(RUNS_JOB);
    let window = harness.attach_showing(&[tab]);
    harness.wait_text(window, tab, "READY");
    let left = harness.other_session("left", 1);
    harness.switch_to(window, left.session, &left.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    harness.resume_quits();
    let close = drawn_close_button(&mut harness, window);
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_click(close, gpui::Modifiers::none());
    if Platform::current() == Platform::Linux {
        // The last window quits rather than strand the session it left,
        // as Close Window does, with no Close Window dialog first.
        wait_quit_dialog(&mut harness, window);
        confirm_quit(&mut harness, window);
        harness.wait_quit(left.session);
    } else {
        harness.wait_for("the confirmation", |cx| confirming(cx, window));
        assert_eq!(
            confirmation(&mut harness, window),
            Some(CloseTarget::Window)
        );
    }
}

#[gpui::test]
fn concurrent_window_closes_on_linux_quit_rather_than_strand_a_session(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[tab]);
    let left = harness.other_session("left", 1);
    harness.switch_to(one, left.session, &left.tabs);
    harness.switch_to(one, harness.session, &[tab]);
    let two = harness.attach_showing(&[tab]);
    harness.resume_quits();
    let runtime = Arc::clone(&harness.runtime);
    // Each close is requested while the other window is open, so each
    // stays a Window close; both assess before either commits.
    runtime.worker_hooks.hold(Stage::Assessed);
    for window in [one, two] {
        harness.update_view(window, |view, window, cx| {
            view.close_window(Platform::current(), window, cx);
        });
    }
    harness.wait_for("both closes to assess", |_| {
        runtime.worker_hooks.held(Stage::Assessed) == 2
    });
    runtime.worker_hooks.release(Stage::Assessed);
    if Platform::current() == Platform::Linux {
        // The second to commit is the last window: it quits, and the
        // capture keeps both the session it showed and the one left.
        harness.wait_quit(left.session);
        let (captured, _) = harness.recorder.saved().remove(0);
        assert!(
            captured
                .sessions
                .iter()
                .any(|kept| kept.info.id.get() == harness.session.get())
        );
    } else {
        harness.wait_closed(one);
        harness.wait_closed(two);
        harness.cx.update(|cx| assert!(!terminating(cx)));
        assert!(
            runtime.lock().session(left.session).is_some(),
            "macOS keeps the unviewed session for a Dock reopen"
        );
    }
}

#[gpui::test]
fn an_emptied_close_and_a_window_close_on_linux_quit_rather_than_strand_a_session(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    let two = harness.attach_showing(&[tab]);
    harness.switch_to(two, other.session, &other.tabs);
    let left = harness.other_session("left", 1);
    harness.wait_projection();
    harness.resume_quits();
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Assessed);
    // Window one's last shell goes, as when it exits, while window two
    // closes; both assess before either commits.
    harness.close_tab(tab);
    harness.update_view(two, |view, window, cx| {
        view.close_window(Platform::current(), window, cx);
    });
    harness.wait_for("both closes to assess", |_| {
        runtime.worker_hooks.held(Stage::Assessed) == 2
    });
    runtime.worker_hooks.release(Stage::Assessed);
    if Platform::current() == Platform::Linux {
        harness.wait_quit(left.session);
    } else {
        harness.wait_closed(one);
        harness.wait_closed(two);
        harness.cx.update(|cx| assert!(!terminating(cx)));
        assert!(runtime.lock().session(left.session).is_some());
    }
}

#[gpui::test]
fn a_window_close_queued_behind_a_switch_on_linux_quits_once_it_is_the_last(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[tab]);
    let two = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    harness.resume_quits();
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Switch);
    harness.select_session(one, other.session).unwrap();
    harness.wait_for("the switch to wait for its worker", |_| {
        runtime.worker_hooks.held(Stage::Switch) == 1
    });
    // Window two is still open, so one's close stays a Window close and
    // queues behind the switch; then two closes.
    harness.update_view(one, |view, window, cx| {
        view.close_window(Platform::current(), window, cx);
        assert_eq!(view.close.pending, Some(CloseTarget::Window));
    });
    harness.update_view(two, |view, window, cx| {
        view.close_window(Platform::current(), window, cx);
    });
    harness.wait_closed(two);
    runtime.worker_hooks.release(Stage::Switch);
    let session = harness.session;
    if Platform::current() == Platform::Linux {
        // Window one is the last now and the session it left has no
        // window: its close quits, and the capture keeps that session.
        harness.wait_quit(session);
    } else {
        harness.wait_closed(one);
        harness.cx.update(|cx| assert!(!terminating(cx)));
        assert!(session_exists(&harness));
    }
}

#[gpui::test]
fn a_failed_own_session_commit_still_follows_to_the_session_it_left(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let left = harness.other_session("left", 1);
    harness.switch_to(window, left.session, &left.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    // Core removes the session, then reports a teardown failure.
    harness.runtime.worker_hooks.fail(Stage::Commit);
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    harness.wait_switched(window, left.session, &left.tabs);
    assert!(!session_exists(&harness));
    harness.cx.update(|cx| {
        let shown = notices(cx, window);
        assert!(
            shown.iter().any(|notice| notice.contains("Close failed")),
            "{shown:?}"
        );
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
    harness.answer(window, left.tabs[0], "followed");
}

#[gpui::test]
fn a_cancelled_emptied_quit_that_can_show_nothing_follows_the_last_window_rule(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    // The session left last runs a job, so the Quit asks.
    let older = harness.other_session("older", 1);
    let left = harness.other_session_running("left", RUNS_JOB, 1);
    harness.switch_to(window, older.session, &older.tabs);
    harness.switch_to(window, left.session, &left.tabs);
    harness.wait_text(window, left.tabs[0], "READY");
    harness.switch_to(window, harness.session, &[tab]);
    harness.resume_quits();
    harness.close_tab(tab);
    if Platform::current() == Platform::MacOs {
        // macOS keeps running for a Dock reopen.
        harness.wait_closed(window);
        return;
    }
    wait_quit_dialog(&mut harness, window);
    // Both sessions left and the fresh session fail.
    harness
        .runtime
        .worker_hooks
        .fail_times(Stage::CloseFollow, 3);
    harness.update_view(window, |view, window, cx| {
        view.cancel_confirmation(window, cx);
    });
    // The window cannot stay empty: it says why and, still the last window
    // with sessions no window shows, asks to quit again.
    wait_quit_dialog(&mut harness, window);
    harness.cx.update(|cx| {
        let shown = notices(cx, window);
        assert!(
            shown
                .iter()
                .any(|notice| notice.contains("Cannot open a new session")),
            "{shown:?}"
        );
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
    // Cancelling that Quit tries again, and now shows the session left.
    harness.update_view(window, |view, window, cx| {
        view.cancel_confirmation(window, cx);
    });
    harness.wait_switched(window, left.session, &left.tabs);
}

/// Closes the window's own session twice while no shell command can be
/// built, with the last-window rule of `platform`: the first follow-up
/// passes over a session that needs a shell, and the second has nothing
/// left it can show.
fn close_session_follow_ups_without_a_shell(
    cx: &mut TestAppContext,
    platform: Platform,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.update_view(window, |view, _, _| {
        view.test_platform = Some(platform);
    });
    // Two sessions no window shows: the first has no tab, so showing it
    // needs a shell; the second has one.
    let bare = {
        let mut mux = harness.runtime.lock();
        let session = mux.create_session(Some("bare")).unwrap();
        mux.create_workspace(session, None).unwrap();
        session
    };
    let tabbed = harness.other_session("tabbed", 1);
    harness.wait_projection();
    harness.update_view(window, |view, _, _| view.session_shell_fails = true);
    // No shell command can be built: the bare session cannot be shown, so
    // the window moves on to the one with a tab, without a notice.
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    harness.wait_switched(window, tabbed.session, &tabbed.tabs);
    assert!(!session_exists(&harness));
    harness.cx.update(|cx| {
        assert_eq!(notices(cx, window), Vec::<String>::new());
    });
    // Only the bare session is left, and the fresh session needs a shell
    // too: the window says why and closes by the last-window rule.
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    if platform == Platform::Linux {
        // The bare session would be stranded, so the window quits, keeping
        // the notice: the emptied close its gone session owed does not run
        // first and remove it.
        let runtime = Arc::clone(&harness.runtime);
        harness.wait_for("the application to quit", |_| {
            runtime.terminating.load(Ordering::Acquire)
        });
        harness.cx.update(|cx| {
            let shown = notices(cx, window);
            assert!(
                shown
                    .iter()
                    .any(|notice| notice.contains("Cannot open a new session")),
                "{shown:?}"
            );
            assert_eq!(
                cx.global::<Desktop>().emptied_quit,
                Some(window.window_id())
            );
        });
    } else {
        harness.wait_closed(window);
        harness.cx.update(|cx| {
            let latched = &cx.global::<Desktop>().latched;
            assert!(
                latched.iter().any(|notice| {
                    notice.message.contains("Cannot open a new session")
                }),
                "{latched:?}"
            );
        });
        let mux = harness.runtime.lock();
        assert_eq!(
            mux.workspace(mux.session(bare).unwrap().workspaces[0])
                .unwrap()
                .tabs
                .len(),
            0
        );
    }
}

#[gpui::test]
fn a_close_session_follow_up_without_a_shell_shows_a_session_that_needs_none(
    cx: &mut TestAppContext,
) {
    close_session_follow_ups_without_a_shell(cx, Platform::MacOs);
}

#[gpui::test]
fn a_close_session_follow_up_without_a_shell_on_linux_quits_with_its_notice(
    cx: &mut TestAppContext,
) {
    close_session_follow_ups_without_a_shell(cx, Platform::Linux);
}

#[gpui::test]
fn a_checkpoint_waits_for_an_own_session_close_from_its_commit(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let left = harness.other_session("left", 1);
    harness.switch_to(window, left.session, &left.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    let runtime = Arc::clone(&harness.runtime);
    // The close committed; its follow-up has not attached the window yet.
    runtime.worker_hooks.hold(Stage::CloseFollow);
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    harness.wait_for("the follow-up to wait", |_| {
        runtime.worker_hooks.held(Stage::CloseFollow) == 1
    });
    assert!(!session_exists(&harness));
    harness.cx.update(|cx| {
        take_checkpoint(cx);
        assert!(cx.global::<Desktop>().checkpoints.waiting_transfer);
    });
    runtime.persistence.flush();
    assert!(
        harness.recorder.saved().is_empty(),
        "saved a window on the closed session"
    );
    runtime.worker_hooks.release(Stage::CloseFollow);
    let recorder = harness.recorder.clone();
    harness.wait_for("the checkpoint after the follow-up settles", |_| {
        !recorder.saved().is_empty()
    });
    let (_, layout) = recorder.saved().remove(0);
    assert_eq!(layout.windows[0].session.get(), left.session.get());
    runtime.terminate().unwrap();
}

#[gpui::test]
fn closing_the_windows_own_session_is_refused_while_a_quit_waits(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (one, two, other) = own_session_close_windows(&mut harness);
    let runtime = Arc::clone(&harness.runtime);
    // A new window's shell is still spawning, so the Quit waits for it.
    runtime.worker_hooks.hold(Stage::Spawned);
    harness
        .cx
        .update(|cx| {
            Desktop::invoke(
                cx,
                &CommandInvocation::new(ids::NEW_WINDOW, Vec::new()),
                None,
            )
        })
        .unwrap();
    harness.wait_for("the new window's shell to spawn", |_| {
        runtime.worker_hooks.held(Stage::Spawned) == 1
    });
    harness.cx.update(request_quit);
    harness.cx.update(|cx| {
        assert!(cx.global::<Desktop>().quit_pending);
        assert!(!terminating(cx));
    });
    for window in [one, two] {
        let refused = harness.invoke(window, ids::CLOSE_SESSION);
        assert!(
            matches!(&refused, Err(CommandError::Unavailable(reason))
                if reason.contains("quitting")),
            "{refused:?}"
        );
    }
    assert_eq!(runtime.worker_hooks.entered(Stage::Assess), 0);
    // The Quit resumes and asks; cancelling it leaves every session.
    runtime.worker_hooks.release(Stage::Spawned);
    harness.wait_for("the Quit dialog", |cx| {
        any_confirmation(cx)
            .is_some_and(|(_, target)| target == CloseTarget::Application)
    });
    let host = harness.cx.update(any_confirmation).unwrap().0;
    harness.update_view(host, |view, window, cx| {
        view.cancel_confirmation(window, cx);
    });
    assert!(session_exists(&harness));
    assert!(harness.runtime.lock().session(other.session).is_some());
}

#[gpui::test]
fn the_last_windows_emptied_close_on_linux_waits_for_a_pending_new_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let left = harness.other_session("left", 1);
    harness.switch_to(window, left.session, &left.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    harness.resume_quits();
    let runtime = Arc::clone(&harness.runtime);
    // A window on the session the user left is still opening.
    runtime.worker_hooks.hold(Stage::OpenSession);
    harness
        .cx
        .update(|cx| {
            Desktop::invoke(
                cx,
                &pick(ids::OPEN_SESSION_IN_NEW_WINDOW, left.session),
                None,
            )
        })
        .unwrap();
    harness.wait_for("the new window to wait for its worker", |_| {
        runtime.worker_hooks.held(Stage::OpenSession) == 1
    });
    // The window's last shell goes, as when it exits.
    harness.close_tab(tab);
    if Platform::current() == Platform::Linux {
        harness.wait_for("the Quit to wait for the new window", |cx| {
            cx.global::<Desktop>().quit_pending
        });
        harness.cx.update(|cx| assert!(!terminating(cx)));
        runtime.worker_hooks.release(Stage::OpenSession);
        harness.wait_quit(left.session);
        let (_, layout) = harness.recorder.saved().remove(0);
        assert!(
            layout
                .windows
                .iter()
                .any(|saved| saved.session.get() == left.session.get()),
            "the new window was not saved"
        );
    } else {
        harness.wait_closed(window);
        harness.cx.update(|cx| assert!(!terminating(cx)));
        runtime.worker_hooks.release(Stage::OpenSession);
        harness.wait_for("the new window on the session left", |cx| {
            cx.global::<Desktop>()
                .windows
                .records()
                .any(|record| record.session == Some(left.session))
        });
    }
}

#[gpui::test]
fn quit_saves_two_windows_on_one_session_with_their_own_navigation(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let split = harness.open_tab(ECHO);
    let left = harness.first_pane(split);
    let right = harness.split(split, left, SplitDirection::Right, ECHO);
    let one = harness.attach_showing(&[first, split]);
    let two = harness.attach_showing(&[first, split]);
    // Window one left the split tab on its left pane for the first tab;
    // window two shows the split tab's right pane.
    harness.focus(one, split, left);
    harness.update_view(one, |view, window, cx| {
        view.select(first, window, cx);
    });
    harness.focus(two, split, right);
    harness.cx.update(request_quit);
    let session = harness.session;
    harness.wait_quit(session);
    let (_, layout) = harness.recorder.saved().remove(0);
    let saved = |active: TabId| {
        layout
            .windows
            .iter()
            .find(|saved| {
                saved.session.get() == session.get()
                    && saved.active_tab.map(huterm_protocol::TabId::get)
                        == Some(active.get())
            })
            .unwrap_or_else(|| panic!("no window on tab {active:?}"))
    };
    let focused = |saved: &huterm_state::WindowLayout| {
        saved
            .focused_panes
            .iter()
            .find(|focused| focused.tab.get() == split.get())
            .map(|focused| focused.pane.get())
    };
    assert_eq!(focused(saved(first)), Some(left.get()));
    assert_eq!(focused(saved(split)), Some(right.get()));
    assert_eq!(
        saved(first)
            .tab_history
            .iter()
            .map(|tab| tab.get())
            .collect::<Vec<_>>(),
        [split.get(), first.get()]
    );
}

#[gpui::test]
fn a_target_closed_while_a_switch_installs_takes_the_session_gone_path(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Subscribe);
    harness.select_session(window, other.session).unwrap();
    harness.wait_for("the switch to install its views", |cx| {
        runtime.worker_hooks.held(Stage::Subscribe) > 0
            && installing(cx, window)
    });
    runtime.lock().close_session(other.session).unwrap();
    runtime.worker_hooks.release(Stage::Subscribe);
    if Platform::current() == Platform::Linux {
        let session = harness.session;
        harness.wait_quit(session);
    } else {
        harness.wait_closed(window);
        assert!(session_exists(&harness));
    }
    assert_eq!(harness.close_failures(), 0);
    harness.cx.update(|cx| {
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
}

#[gpui::test]
fn quit_from_a_sibling_waits_for_a_new_window_held_on_the_projection(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.resume_quits();
    let runtime = Arc::clone(&harness.runtime);
    // The projection stops applying events, as while it resubscribes.
    harness.cx.update(|cx| {
        assert!(cx.global_mut::<Desktop>().hierarchy.begin_resync());
    });
    harness
        .cx
        .update(|cx| {
            Desktop::invoke(
                cx,
                &CommandInvocation::new(ids::NEW_WINDOW, Vec::new()),
                None,
            )
        })
        .unwrap();
    harness.wait_for(
        "the new window's spawn to wait on the projection",
        |cx| {
            let desktop = cx.global::<Desktop>();
            runtime.worker_hooks.entered(Stage::Spawned) == 1
                && desktop.pending_spawns == 1
                && desktop.hierarchy.waiting() > 0
        },
    );
    harness.activate_window(window);
    harness.cx.update(request_quit);
    harness.cx.update(|cx| {
        assert!(cx.global::<Desktop>().quit_pending);
        assert!(!terminating(cx));
    });
    // The projection catches up; the spawn publishes and the Quit runs.
    harness.cx.update(|cx| {
        let (state, subscription) = runtime.subscribe_hierarchy();
        assert!(install_hierarchy(cx, state, subscription));
    });
    let session = harness.session;
    harness.wait_quit(session);
    let (captured, _) = harness.recorder.saved().remove(0);
    assert_eq!(captured.sessions.len(), 2, "the new window's session");
}

#[gpui::test]
fn an_own_session_close_whose_session_another_close_removed_still_follows(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(RUNS_JOB);
    let window = harness.attach_showing(&[tab]);
    harness.wait_text(window, tab, "READY");
    let left = harness.other_session("left", 1);
    harness.switch_to(window, left.session, &left.tabs);
    harness.switch_to(window, harness.session, &[tab]);
    // The window asks before closing its own session, which runs a job.
    harness.invoke(window, ids::CLOSE_SESSION).unwrap();
    harness.wait_for("the confirmation", |cx| confirming(cx, window));
    // Another close removes the session first, so this window's commit is
    // refused as stale while its follow-up still runs.
    let closing = harness.session;
    harness.runtime.lock().close_session(closing).unwrap();
    harness.wait_projection();
    harness.update_view(window, |view, window, cx| {
        let target = view.close.confirmation.clone().unwrap();
        let generation = view.close.generation;
        view.confirm_confirmation(target, generation, window, cx);
    });
    harness.wait_switched(window, left.session, &left.tabs);
    harness.answer(window, left.tabs[0], "followed");
    harness.wait_projection();
    harness.cx.update(|cx| {
        let desktop = cx.global::<Desktop>();
        // The follow-up's attachment is the window's own, and no other is
        // left behind on the session.
        assert_eq!(
            desktop
                .hierarchy
                .state()
                .session_attachments(left.session)
                .count(),
            1
        );
        assert_eq!(desktop.pending_spawns, 0);
        assert_eq!(notices(cx, window), [] as [String; 0]);
    });
}
