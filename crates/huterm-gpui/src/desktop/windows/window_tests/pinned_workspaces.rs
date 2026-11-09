//! Tier 2 tests for #27 PR C: pinned workspaces, which stay and show the
//! empty state when their last tab goes, the directory a workspace's last
//! tab leaves for its next new tab, and the workspace rows' status dots
//! with per-window bell baselines. Interleavings W45 to W57 of
//! `docs/plans/workspace-panels.md`. Ordering comes from worker hooks and
//! sequence waits, never sleeps.

use super::super::workspace_status::Dot;
use super::worker_hooks::Stage;
use super::workspaces::{
    LastTabGoes, attachment_of, empty_state, shown_workspace, workspace_pick,
};
use super::*;

/// A child that prints `READY`, then rings a bell for each line it reads.
const BELLS: &str =
    "printf READY; while read line; do printf '\\007RANG'; done";

impl DesktopHarness {
    /// Pins or unpins `workspace` as another client would.
    fn pin(&self, workspace: WorkspaceId, pinned: bool) {
        self.runtime
            .lock()
            .set_workspace_pinned(workspace, pinned)
            .unwrap();
    }

    fn pinned(&self, workspace: WorkspaceId) -> bool {
        self.runtime
            .lock()
            .workspace(workspace)
            .is_some_and(huterm_core::Workspace::pinned)
    }

    /// Waits until `window` shows the empty state of `workspace`.
    fn wait_empty_state(
        &mut self,
        window: AnyWindowHandle,
        workspace: WorkspaceId,
    ) {
        self.wait_for("the empty state", |cx| {
            shown_workspace(cx, window) == Some(workspace)
                && empty_state(cx, window).is_some()
        });
    }

    /// `workspace`'s row dot as `window` draws it.
    fn dot(
        &mut self,
        window: AnyWindowHandle,
        workspace: WorkspaceId,
    ) -> Option<Dot> {
        self.cx.update(|cx| row_dot(cx, window, workspace))
    }
}

/// W45: the pinned workspace's last tab goes in the way `goes` names,
/// beside a populated sibling; the window stays on it, empty.
fn emptying_a_pinned_workspace_shows_its_empty_state(
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
    let (sibling, _) = harness.add_workspace(1);
    harness.pin(harness.workspace, true);
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
    harness.wait_empty_state(window, harness.workspace);
    harness.update_view(window, |view, _, cx| {
        assert_eq!(view.close.emptied, EmptiedClose::None);
        assert!(view.close.current.is_none());
        assert_eq!(cx.global::<Desktop>().pending_spawns, 0);
    });
    harness
        .cx
        .update(|cx| assert!(cx.windows().contains(&window)));
    assert_eq!(harness.close_failures(), 0, "a refusal raises no notice");
    assert_eq!(harness.workspaces(), [harness.workspace, sibling]);
    assert!(harness.pinned(harness.workspace));
}

#[gpui::test]
fn closing_a_pinned_workspaces_last_tab_shows_its_empty_state(
    cx: &mut TestAppContext,
) {
    emptying_a_pinned_workspace_shows_its_empty_state(cx, LastTabGoes::Closed);
}

#[gpui::test]
fn an_exit_close_in_a_pinned_workspace_shows_its_empty_state(
    cx: &mut TestAppContext,
) {
    emptying_a_pinned_workspace_shows_its_empty_state(
        cx,
        LastTabGoes::ExitCloses,
    );
}

#[gpui::test]
fn closing_a_pinned_workspaces_lone_pane_shows_its_empty_state(
    cx: &mut TestAppContext,
) {
    emptying_a_pinned_workspace_shows_its_empty_state(
        cx,
        LastTabGoes::LonePaneClosed,
    );
}

#[gpui::test]
fn moving_a_pinned_workspaces_last_tab_away_shows_its_empty_state(
    cx: &mut TestAppContext,
) {
    emptying_a_pinned_workspace_shows_its_empty_state(
        cx,
        LastTabGoes::MovedToAnotherSession,
    );
}

/// W46: the session's only workspace, pinned, keeps its window and its
/// session; New Tab has focus.
#[gpui::test]
fn a_pinned_only_workspace_keeps_its_window_and_session(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.pin(harness.workspace, true);
    harness.wait_projection();
    harness.activate(window, tab);
    harness.invoke(window, ids::CLOSE_TAB).unwrap();
    harness.wait_empty_state(window, harness.workspace);
    harness.update_view(window, |view, window, cx| {
        let shown = view.empty_state_model(cx).unwrap();
        // The only workspace offers neither switch nor close.
        assert!(!shown.switch && !shown.close);
        assert!(view.empty_state_focused(window), "New Tab has no focus");
    });
    assert!(session_exists(&harness));
}

/// W46 on Linux: a last window whose emptied close would have become Quit
/// stays open instead.
#[gpui::test]
fn a_linux_last_window_on_an_emptied_pinned_workspace_stays_open(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.as_linux(window);
    let unviewed = harness.unviewed_session();
    harness.pin(harness.workspace, true);
    harness.wait_projection();
    harness.resume_quits();
    harness.activate(window, tab);
    harness.invoke(window, ids::CLOSE_TAB).unwrap();
    harness.wait_empty_state(window, harness.workspace);
    harness.cx.update(|cx| {
        assert!(cx.windows().contains(&window));
        assert!(!quitting(cx) && !terminating(cx));
        assert_eq!(cx.global::<Desktop>().emptied_quit, None);
    });
    assert!(session_exists(&harness));
    assert!(harness.runtime.lock().session(unviewed).is_some());
}

/// W47: an emptied close assessed as a detach is held, and another client
/// pins the workspace before it commits; the stale commit assesses again,
/// core refuses, and the window stays on the empty workspace.
#[gpui::test]
fn pinning_while_an_emptied_close_is_held_keeps_the_workspace(
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
    harness.pin(harness.workspace, true);
    runtime.worker_hooks.release(Stage::Assessed);
    harness.wait_empty_state(window, harness.workspace);
    harness.cx.update(|cx| {
        assert!(cx.windows().contains(&window));
        assert_eq!(attachment_of(cx, window), attachment, "still attached");
    });
    assert!(harness.workspace_exists(harness.workspace));
    assert!(other_client.is_ok());
}

/// W55: core refuses the emptied close as pinned, and another client
/// closes the workspace before the window handles the refusal; the window
/// still follows to its sibling.
#[gpui::test]
fn a_workspace_closed_after_its_pinned_refusal_still_moves_its_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (sibling, siblings) = harness.add_workspace(1);
    harness.pin(harness.workspace, true);
    harness.wait_projection();
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Assessed);
    harness.close_tab(tab);
    harness.wait_for("the pinned refusal", |_| {
        runtime.worker_hooks.held(Stage::Assessed) == 1
    });
    runtime.lock().close_workspace(harness.workspace).unwrap();
    harness.wait_projection();
    runtime.worker_hooks.release(Stage::Assessed);
    harness.wait_workspace(window, sibling, &siblings);
    assert_eq!(harness.workspaces(), [sibling]);
    assert_eq!(harness.close_failures(), 0);
}

/// W56: as W55, but the session closes instead; the window goes with it.
#[gpui::test]
fn a_session_closed_after_its_pinned_refusal_closes_its_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    // Keep the application running once the window goes.
    let _unviewed = harness.unviewed_session();
    harness.pin(harness.workspace, true);
    harness.wait_projection();
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Assessed);
    harness.close_tab(tab);
    harness.wait_for("the pinned refusal", |_| {
        runtime.worker_hooks.held(Stage::Assessed) == 1
    });
    runtime.lock().close_session(harness.session).unwrap();
    harness.wait_projection();
    runtime.worker_hooks.release(Stage::Assessed);
    harness.wait_closed(window);
}

/// W48: unpinning the empty workspace a window shows closes nothing; it
/// stays as a selected empty workspace would.
#[gpui::test]
fn unpinning_an_empty_shown_workspace_keeps_it(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (sibling, siblings) = harness.add_workspace(1);
    harness.pin(harness.workspace, true);
    harness.wait_projection();
    harness.close_tab(tab);
    harness.wait_empty_state(window, harness.workspace);
    harness
        .invoke_with(
            window,
            &workspace_pick(ids::TOGGLE_WORKSPACE_PIN, harness.workspace),
        )
        .unwrap();
    let workspace = harness.workspace;
    harness.wait_for("the unpin to project", |cx| {
        projected_pin(cx, workspace) == Some(false)
    });
    harness.wait_empty_state(window, harness.workspace);
    harness.update_view(window, |view, _, _| {
        assert_eq!(view.close.emptied, EmptiedClose::None);
    });
    assert_eq!(harness.workspaces(), [harness.workspace, sibling]);
    // Leaving it now leaves it in place: only an emptied close removes an
    // empty workspace, and selecting another is not one.
    harness.show_workspace(window, sibling, &siblings);
    assert!(harness.workspace_exists(harness.workspace));
}

/// W49: Close Workspace still closes a pinned workspace; the pin only stops
/// the automatic close.
#[gpui::test]
fn close_workspace_still_closes_a_pinned_workspace(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (sibling, siblings) = harness.add_workspace(1);
    harness.pin(harness.workspace, true);
    harness.wait_projection();
    harness.invoke(window, ids::CLOSE_WORKSPACE).unwrap();
    harness.wait_workspace(window, sibling, &siblings);
    assert_eq!(harness.workspaces(), [sibling]);
}

/// W50: a window on an empty pinned only-workspace is not departing; a
/// checkpoint and Quit save it, its session, and the pin.
#[gpui::test]
fn an_empty_pinned_workspace_is_saved_by_checkpoints_and_quit(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.pin(harness.workspace, true);
    harness.wait_projection();
    harness.close_tab(tab);
    harness.wait_empty_state(window, harness.workspace);
    let workspace = harness.workspace;
    let saved_pinned = |captured: &huterm_protocol::RuntimeCapture| {
        captured
            .sessions
            .iter()
            .flat_map(|session| &session.workspaces)
            .any(|kept| {
                kept.info.id.get() == workspace.get() && kept.info.pinned
            })
    };
    let recorder = harness.recorder.clone();
    let before = recorder.saved().len();
    harness.cx.update(take_checkpoint);
    harness.runtime.persistence.flush();
    harness.wait_for("the checkpoint", |_| recorder.saved().len() > before);
    let (captured, layout) = recorder.saved().pop().unwrap();
    assert_eq!(layout.windows[0].workspace.get(), workspace.get());
    assert!(saved_pinned(&captured));
    harness.resume_quits();
    harness.cx.update(request_quit);
    harness.wait_quit(harness.session);
    let (captured, layout) = harness.recorder.saved().pop().unwrap();
    assert_eq!(layout.windows[0].workspace.get(), workspace.get());
    assert!(saved_pinned(&captured));
}

/// W51: two windows on a pinned workspace both stay when its last tab
/// closes.
#[gpui::test]
fn two_windows_on_an_emptied_pinned_workspace_both_stay(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[tab]);
    let two = harness.attach_showing(&[tab]);
    harness.pin(harness.workspace, true);
    harness.wait_projection();
    harness.activate(one, tab);
    harness.invoke(one, ids::CLOSE_TAB).unwrap();
    harness.wait_empty_state(one, harness.workspace);
    harness.wait_empty_state(two, harness.workspace);
    assert!(session_exists(&harness));
}

/// W52: New Tab in a workspace with no tab starts where its last tab
/// worked; `new_tab_directory = "default"` ignores that.
#[gpui::test]
fn a_new_tab_in_an_empty_workspace_starts_where_its_last_tab_left(
    cx: &mut TestAppContext,
) {
    new_tab_after_the_last_tab(cx, huterm_config::NewTabDirectory::Inherit);
}

#[gpui::test]
fn default_new_tab_directories_ignore_where_the_last_tab_left(
    cx: &mut TestAppContext,
) {
    new_tab_after_the_last_tab(cx, huterm_config::NewTabDirectory::Default);
}

fn new_tab_after_the_last_tab(
    cx: &mut TestAppContext,
    policy: huterm_config::NewTabDirectory,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.terminal.new_tab_directory = policy;
    });
    let scratch = std::env::temp_dir()
        .join(format!("huterm-27c-remembered-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let scratch = scratch.canonicalize().unwrap();
    let mut command = fixture(ECHO);
    command.working_directory.clone_from(&scratch);
    let tab = harness
        .runtime
        .lock()
        .open_tab(harness.workspace, &command)
        .unwrap()
        .tab
        .id;
    let window = harness.attach_showing(&[tab]);
    harness.pin(harness.workspace, true);
    harness.wait_projection();
    harness.close_tab(tab);
    harness.wait_empty_state(window, harness.workspace);
    harness.update_view(window, |view, window, cx| {
        view.new_tab(window, cx).unwrap();
    });
    let workspace = harness.workspace;
    let mut opened = None;
    harness.wait_for("the new tab to open", |cx| {
        opened = cx
            .global::<Desktop>()
            .hierarchy
            .state()
            .workspace_tabs(workspace)
            .and_then(|tabs| tabs.first().copied());
        opened.is_some()
    });
    let launched = {
        let mux = harness.runtime.lock();
        let terminal =
            mux.tab(opened.unwrap()).unwrap().first_pane().terminal_id;
        mux.capture_runtime(&HashSet::new())
            .terminal(huterm_protocol::TerminalId::new(terminal.get()))
            .unwrap()
            .directories
            .last()
            .cloned()
    };
    let _ = std::fs::remove_dir(&scratch);
    match policy {
        huterm_config::NewTabDirectory::Inherit => {
            assert_eq!(launched, Some(scratch));
        }
        huterm_config::NewTabDirectory::Default => {
            assert_ne!(launched, Some(scratch));
        }
    }
}

/// W53: a bell in a workspace a window does not show marks that row in
/// that window only, until the window shows it.
#[gpui::test]
fn a_hidden_workspaces_bell_marks_its_row_until_the_window_shows_it(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[tab]);
    let (bells, rung) = harness.add_workspace_running(BELLS, 1);
    let rung = rung[0];
    let two = harness.attach_showing(&[tab]);
    harness.show_workspace(two, bells, &[rung]);
    harness.wait_text(two, rung, "READY");
    assert_eq!(harness.dot(one, bells), None);
    let _viewer = harness.send_line(rung, "a");
    harness.wait_for_dot(one, bells, Some(Dot::Bell));
    // The window that shows the workspace sees the bell in its tab.
    assert_eq!(harness.dot(two, bells), None);
    // Showing it in the first window sees it, and leaving keeps it seen.
    harness.show_workspace(one, bells, &[rung]);
    harness.show_workspace(one, harness.workspace, &[tab]);
    harness.wait_for_dot(one, bells, None);
}

/// W57: a window that opens on a session the desktop already read counts
/// the bells rung before it opened as seen, and the next one as unseen,
/// though no hierarchy batch follows its opening.
#[gpui::test]
fn a_window_opened_between_reads_marks_the_next_bell(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[tab]);
    let (bells, rung) = harness.add_workspace_running(BELLS, 1);
    let rung = rung[0];
    // Another client rings once, before the second window opens.
    let viewer = harness.send_line(rung, "a");
    let terminal = harness.cx.update(|cx| {
        cx.global::<Desktop>()
            .hierarchy
            .state()
            .tab(rung)
            .unwrap()
            .layout
            .first_pane()
            .terminal_id
    });
    harness.wait_for("the desktop to read the bell", |cx| {
        cx.global::<Desktop>()
            .statuses
            .get(terminal)
            .is_some_and(|status| status.bells == 1)
    });
    harness.wait_for_dot(one, bells, Some(Dot::Bell));
    let two = harness.attach_showing(&[tab]);
    assert_eq!(harness.dot(two, bells), None, "rung before it opened");
    viewer
        .send_input(huterm_protocol::TerminalInput::Text("b\r".into()))
        .unwrap();
    harness.wait_for_dot(two, bells, Some(Dot::Bell));
}

/// W57 for a session switch: a window switching into a session the desktop
/// already read counts the bells rung there before as seen, and the next
/// one as unseen, though no hierarchy batch follows the switch.
#[gpui::test]
fn a_window_switched_between_reads_marks_the_next_bell(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let other = harness.other_session("other", 1);
    let (bells, terminal) = {
        let mut mux = harness.runtime.lock();
        let bells = mux.create_workspace(other.session, None).unwrap();
        let opened = mux.open_tab(bells, &fixture(BELLS)).unwrap();
        (bells, opened.tab.first_pane().terminal_id)
    };
    // Another client rings once, before the window switches.
    let viewer = {
        let mut mux = harness.runtime.lock();
        let attachment = mux.attach_session(other.session).unwrap();
        mux.subscribe_terminal(
            attachment,
            terminal,
            ViewerOptions::new(ViewerCapabilities::ALL),
        )
        .unwrap()
    };
    let ring = |viewer: &TerminalViewer| {
        viewer
            .send_input(huterm_protocol::TerminalInput::Text("a\r".into()))
            .unwrap();
    };
    harness.wait_for("the shell to start", |cx| {
        cx.global::<Desktop>().statuses.get(terminal).is_some()
    });
    ring(&viewer);
    harness.wait_for("the desktop to read the bell", |cx| {
        cx.global::<Desktop>()
            .statuses
            .get(terminal)
            .is_some_and(|status| status.bells == 1)
    });
    harness.switch_to(window, other.session, &other.tabs);
    assert_eq!(harness.dot(window, bells), None, "rung before it switched");
    ring(&viewer);
    harness.wait_for_dot(window, bells, Some(Dot::Bell));
}

/// W54: a shell that exits in a workspace no window shows marks its row,
/// whose status names the tab: with no view, nothing closes the tab.
#[gpui::test]
fn an_exited_shell_in_an_unviewed_workspace_marks_its_row(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (hidden, exits) = harness.add_workspace_running(EXITS, 1);
    harness.wait_projection();
    // Nothing views the hidden tab; its exit still reaches the row.
    let _viewer = harness.send_line(exits[0], "done");
    harness.wait_for_dot(window, hidden, Some(Dot::Exited));
    let text = harness.update_view(window, |view, _, cx| {
        view.workspace_row_status(hidden, cx)
            .map(|status| status.text)
    });
    assert_eq!(text.as_deref(), Some("Exited: sh"));
}

impl DesktopHarness {
    fn wait_for_dot(
        &mut self,
        window: AnyWindowHandle,
        workspace: WorkspaceId,
        dot: Option<Dot>,
    ) {
        self.wait_for(&format!("the row's dot to be {dot:?}"), |cx| {
            row_dot(cx, window, workspace) == dot
        });
    }
}

/// `workspace`'s row dot as `window` draws it, read without a window
/// update.
fn row_dot(
    cx: &mut App,
    window: AnyWindowHandle,
    workspace: WorkspaceId,
) -> Option<Dot> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx)
                .workspace_row_status(workspace, cx)
                .map(|status| status.dot)
        })
        .ok()
        .flatten()
}

/// The pin command and the row menu pin and unpin a workspace, and the
/// row shows the pin mark.
#[gpui::test]
fn the_pin_command_and_row_menu_pin_and_unpin_a_workspace(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let workspace = harness.workspace;
    harness.invoke(window, ids::TOGGLE_WORKSPACE_PIN).unwrap();
    harness.wait_for("the pin to project", |cx| {
        projected_pin(cx, workspace) == Some(true)
    });
    assert!(harness.pinned(workspace));
    let row = harness.update_view(window, |view, _, cx| {
        view.workspace_rows(cx)
            .into_iter()
            .find(|row| row.workspace == workspace)
            .unwrap()
    });
    assert!(row.pinned);
    harness.update_view(window, |view, window, cx| {
        view.run_workspace_pick(
            &format!("workspace-pin:{}", workspace.get()),
            window,
            cx,
        );
    });
    harness.wait_for("the unpin to project", |cx| {
        projected_pin(cx, workspace) == Some(false)
    });
    assert!(!harness.pinned(workspace));
}

fn projected_pin(cx: &mut App, workspace: WorkspaceId) -> Option<bool> {
    cx.global::<Desktop>()
        .hierarchy
        .state()
        .workspace(workspace)
        .map(|info| info.pinned)
}
