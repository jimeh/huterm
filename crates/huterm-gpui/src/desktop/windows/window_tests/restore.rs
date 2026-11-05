//! Tier 2 tests for session restore and checkpoints: the bootstrap reopens
//! saved windows over a fresh runtime with fresh shells, Quit during it
//! stops what it spawned, restored shells that exit at once still close,
//! and saves follow Quit, the last window's close, and the setting.

use super::super::restore as startup;
use super::*;
use huterm_protocol::{
    PaneInfo, PaneNode, RuntimeCapture, SPLIT_TOTAL, SessionCapture,
    SessionInfo, SplitChild, SplitNode, TabInfo, TerminalCapture,
    TerminalLaunch, WorkspaceCapture, WorkspaceInfo,
};

/// A scratch directory for child PID files and saved pane directories.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static UNIQUE: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "huterm-restore-gpui-{}-{}",
            std::process::id(),
            UNIQUE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("project")).unwrap();
        Self(path)
    }

    /// The shells' reports, one `pid:directory` line each.
    fn reports(&self) -> Vec<(i32, PathBuf)> {
        std::fs::read_to_string(self.0.join("pids"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let (pid, directory) = line.split_once(':')?;
                Some((pid.parse().ok()?, PathBuf::from(directory)))
            })
            .collect()
    }

    /// A child that reports its PID and directory, then echoes like
    /// [`ECHO`].
    fn reporting(&self) -> String {
        format!(
            "printf '%s:%s\\n' \"$$\" \"$(pwd -P)\" >> '{}'; {ECHO}",
            self.0.join("pids").display()
        )
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn pane(id: u64, terminal: u64, name: Option<&str>) -> PaneNode {
    PaneNode::Pane(PaneInfo {
        id: PaneId::new(id),
        terminal_id: TerminalId::new(terminal),
        fallback_name: String::new(),
        custom_name: name.map(str::to_owned),
    })
}

fn terminal_record(id: u64, directory: &Path) -> TerminalCapture {
    TerminalCapture {
        id: TerminalId::new(id),
        directories: vec![directory.to_path_buf()],
        launch: TerminalLaunch::DefaultShell,
    }
}

/// Session 1 ("work") with tab 3, a 60/40 split of panes 5 and 7, and tab
/// 9; session 12 with tab 14, which no saved window shows. Pane 7's folder
/// no longer exists.
fn capture(scratch: &Scratch) -> RuntimeCapture {
    let project = scratch.0.join("project");
    let gone = scratch.0.join("gone");
    let split = PaneNode::Split(SplitNode {
        id: huterm_protocol::SplitId::new(4),
        axis: SplitAxis::Columns,
        children: vec![
            SplitChild {
                weight: SPLIT_TOTAL / 10 * 6,
                node: pane(5, 6, Some("editor")),
            },
            SplitChild {
                weight: SPLIT_TOTAL / 10 * 4,
                node: pane(7, 8, None),
            },
        ],
    });
    let tab = |id: u64, root: PaneNode, name: Option<&str>| TabInfo {
        id: TabId::new(id),
        custom_name: name.map(str::to_owned),
        layout: PaneLayout::new(root).unwrap(),
        zoomed: None,
        input_sync: false,
    };
    let workspace = |id: u64, tabs: Vec<TabInfo>| WorkspaceCapture {
        info: WorkspaceInfo {
            id: WorkspaceId::new(id),
            custom_name: None,
            automatic_name: format!("Workspace {id}"),
        },
        tabs,
    };
    RuntimeCapture {
        socket_name: huterm_core::DEFAULT_SOCKET_NAME.into(),
        next_id: 16,
        session_ordinal: 2,
        workspace_ordinal: 2,
        sessions: vec![
            SessionCapture {
                info: SessionInfo {
                    id: SessionId::new(1),
                    custom_name: Some("work".into()),
                    automatic_name: "Session 1".into(),
                },
                workspaces: vec![workspace(
                    2,
                    vec![
                        tab(3, split, Some("build")),
                        tab(9, pane(10, 11, None), None),
                    ],
                )],
            },
            SessionCapture {
                info: SessionInfo {
                    id: SessionId::new(12),
                    custom_name: None,
                    automatic_name: "Session 2".into(),
                },
                workspaces: vec![workspace(
                    13,
                    vec![tab(14, pane(15, 16, None), None)],
                )],
            },
        ],
        terminals: vec![
            terminal_record(6, &project),
            terminal_record(8, &gone),
            terminal_record(11, &project),
            terminal_record(16, &project),
        ],
    }
}

/// One window for session 1, which last activated tab 3 and focused its
/// pane 7.
fn layout() -> huterm_state::DesktopLayout {
    huterm_state::DesktopLayout {
        windows: vec![huterm_state::WindowLayout {
            session: SessionId::new(1),
            workspace: WorkspaceId::new(2),
            active_tab: Some(TabId::new(3)),
            tab_history: vec![TabId::new(9), TabId::new(3)],
            focused_panes: vec![huterm_state::FocusedPane {
                tab: TabId::new(3),
                pane: PaneId::new(7),
            }],
            bounds: huterm_state::WindowBounds {
                x: 40.0,
                y: 60.0,
                width: 900.0,
                height: 640.0,
            },
            mode: huterm_state::WindowMode::Windowed,
            sidebar_width: 210.0,
            quake_profile: None,
            appearance: None,
        }],
        active_window: Some(0),
    }
}

impl DesktopHarness {
    /// A desktop that restores `capture` and `layout` at launch, each pane
    /// running `script` in its saved directory.
    fn restoring(
        cx: &mut TestAppContext,
        configure: impl FnOnce(&mut Config),
        capture: RuntimeCapture,
        layout: Option<huterm_state::DesktopLayout>,
        script: &str,
    ) -> Self {
        let harness =
            Self::installed(cx, configure, persistence::State::Bootstrapping);
        let base = TerminalCommand {
            working_directory: std::env::temp_dir(),
            ..fixture(script)
        };
        harness.cx.update(|cx| {
            startup::start_with(cx, startup::Saved::new(capture, layout), base);
        });
        harness
    }

    fn runtime_id(&self) -> RuntimeId {
        self.runtime.lock().runtime_id()
    }

    fn wait_settled(&mut self) {
        self.wait_for("the restore to settle", |cx| {
            !startup::bootstrapping(cx)
        });
    }

    /// The open window attached to `session`.
    fn window_of(&mut self, session: SessionId) -> AnyWindowHandle {
        self.cx.update(|cx| {
            let desktop = cx.global::<Desktop>();
            let id = desktop
                .windows
                .records()
                .find(|record| record.session == Some(session))
                .map(|record| record.id)
                .expect("a window shows the session");
            cx.windows()
                .into_iter()
                .find(|handle| handle.window_id() == id)
                .unwrap()
        })
    }
}

/// Whether `pid` names no process, after its parent reaped it.
fn gone(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None)
        == Err(nix::errno::Errno::ESRCH)
}

#[gpui::test]
fn a_restore_reopens_saved_windows_with_fresh_shells_and_saved_focus(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut harness = DesktopHarness::restoring(
        cx,
        |_| {},
        capture(&scratch),
        Some(layout()),
        &scratch.reporting(),
    );
    harness.wait_settled();
    let runtime = harness.runtime_id();
    let scoped_tab = |id| TabId::in_runtime(runtime, id);
    let work = SessionId::in_runtime(runtime, 1);
    let unviewed = SessionId::in_runtime(runtime, 12);
    let window = harness.window_of(work);
    // The session no saved window showed opens in a window of its own.
    let other = harness.window_of(unviewed);
    assert_ne!(window, other);
    harness.wait_text(window, scoped_tab(3), "READY");
    harness.wait_text(other, scoped_tab(14), "READY");
    harness.cx.update(|cx| {
        let desktop = cx.global::<Desktop>();
        let record = desktop.windows.record(window.window_id()).unwrap();
        assert_eq!(
            record.tabs.iter().map(|entry| entry.id).collect::<Vec<_>>(),
            [scoped_tab(3), scoped_tab(9)]
        );
        assert_eq!(record.active, Some(scoped_tab(3)));
        assert_eq!(record.history, [scoped_tab(3), scoped_tab(9)]);
        assert_eq!(record.layout.sidebar_width, px(210.0));
        let state = desktop.hierarchy.state();
        assert_eq!(
            state.session(work).unwrap().custom_name.as_deref(),
            Some("work")
        );
        let tab = state.tab(scoped_tab(3)).unwrap();
        assert_eq!(tab.custom_name.as_deref(), Some("build"));
        let PaneNode::Split(split) = tab.layout.root() else {
            panic!("the split came back");
        };
        assert_eq!(
            split.children.iter().map(|c| c.weight).collect::<Vec<_>>(),
            [SPLIT_TOTAL / 10 * 6, SPLIT_TOTAL / 10 * 4]
        );
    });
    let focused = harness.update_view(window, |view, _, _| {
        view.tabs
            .iter()
            .find(|tab| tab.id == scoped_tab(3))
            .map(|tab| tab.focused().pane)
    });
    assert_eq!(focused, Some(PaneId::in_runtime(runtime, 7)));
    // Fresh shells: one per pane, each in its saved folder, else the
    // default one.
    harness.wait_for("every shell to report", |_| scratch.reports().len() == 4);
    let project = scratch.0.join("project").canonicalize().unwrap();
    let fallback = std::env::temp_dir().canonicalize().unwrap();
    let mut directories: Vec<_> =
        scratch.reports().into_iter().map(|(_, dir)| dir).collect();
    directories.sort();
    let mut expected =
        vec![project.clone(), project.clone(), project, fallback];
    expected.sort();
    assert_eq!(directories, expected);
    // The pane that started elsewhere says so in its window only.
    let notices = |harness: &mut DesktopHarness, window| {
        harness.update_view(window, |view, _, _| {
            view.notices
                .contents()
                .map(|content| content.message.clone())
                .collect::<Vec<_>>()
        })
    };
    let shown = notices(&mut harness, window);
    assert!(
        shown
            .iter()
            .any(|message| message.starts_with("1 pane started in ")
                && message.ends_with("because its folder no longer exists.")),
        "{shown:?}"
    );
    assert_eq!(notices(&mut harness, other), Vec::<String>::new());
    // Settling writes one checkpoint of what was restored.
    let recorder = harness.recorder.clone();
    harness.wait_for("the settle checkpoint", |_| !recorder.saved().is_empty());
    let (saved, layout) = harness.recorder.saved().remove(0);
    assert_eq!(saved.sessions.len(), 2);
    assert_eq!(layout.windows.len(), 2);
    assert_eq!(harness.recorder.recorded().marked, 1);
    assert_eq!(harness.recorder.recorded().cleared, 1);
    harness.runtime.terminate().unwrap();
    for (pid, _) in scratch.reports() {
        assert!(gone(pid), "shell {pid} outlived Quit");
    }
}

#[gpui::test]
fn quit_before_the_restore_spawns_starts_nothing_and_writes_nothing(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let harness = {
        // Hold the worker before its first spawn.
        let mut harness = DesktopHarness::installed(
            cx,
            |_| {},
            persistence::State::Bootstrapping,
        );
        harness.runtime.worker_hooks.hold(Stage::Restore);
        let base = fixture(&scratch.reporting());
        let saved = startup::Saved::new(capture(&scratch), Some(layout()));
        harness.cx.update(|cx| startup::start_with(cx, saved, base));
        let runtime = Arc::clone(&harness.runtime);
        harness.wait_for("the restore worker to start", |_| {
            runtime.worker_hooks.held(Stage::Restore) == 1
        });
        // The native quit hook's cleanup.
        runtime.terminate().unwrap();
        runtime.worker_hooks.release(Stage::Restore);
        harness.wait_settled();
        harness
    };
    assert_eq!(harness.runtime.lock().terminal_count(), 0);
    assert!(scratch.reports().is_empty(), "a shell started after Quit");
    harness.cx.update(|cx| assert_eq!(cx.windows(), []));
    let recorded = harness.recorder.recorded();
    assert!(
        recorded.saved.is_empty(),
        "Quit during a restore saved state"
    );
    // The marker goes, so the next launch restores the same state.
    assert_eq!(recorded.cleared, 1);
    assert_eq!(
        harness.runtime.persistence.state(),
        persistence::State::Final
    );
}

#[gpui::test]
fn quit_after_the_restore_inserted_its_shells_terminates_them(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut harness = DesktopHarness::installed(
        cx,
        |_| {},
        persistence::State::Bootstrapping,
    );
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Restored);
    let saved = startup::Saved::new(capture(&scratch), Some(layout()));
    let base = fixture(&scratch.reporting());
    harness.cx.update(|cx| startup::start_with(cx, saved, base));
    harness.wait_for("the restored shells to start", |_| {
        runtime.worker_hooks.held(Stage::Restored) == 1
            && scratch.reports().len() == 4
    });
    assert_eq!(runtime.lock().terminal_count(), 4);
    runtime.terminate().unwrap();
    runtime.worker_hooks.release(Stage::Restored);
    harness.wait_settled();
    for (pid, _) in scratch.reports() {
        assert!(gone(pid), "restored shell {pid} outlived Quit");
    }
    harness.cx.update(|cx| assert_eq!(cx.windows(), []));
    assert_eq!(harness.recorder.saved(), []);
}

/// Restores two tabs whose shells exit at once, holding view installs
/// until both exited, so no view existed when they did.
fn restore_exiting(
    cx: &mut TestAppContext,
    close_on_exit: bool,
) -> (DesktopHarness, Scratch) {
    let scratch = Scratch::new();
    let mut harness = DesktopHarness::installed(
        cx,
        |config| config.terminal.close_on_exit = close_on_exit,
        persistence::State::Bootstrapping,
    );
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Subscribe);
    let mut capture = capture(&scratch);
    capture.sessions.truncate(1);
    capture.sessions[0].workspaces[0].tabs.truncate(1);
    capture.terminals.truncate(2);
    let script = format!(
        "printf '%s:%s\\n' \"$$\" \"$(pwd -P)\" >> '{}'; exit 0",
        scratch.0.join("pids").display()
    );
    let saved = startup::Saved::new(capture, Some(layout()));
    harness
        .cx
        .update(|cx| startup::start_with(cx, saved, fixture(&script)));
    harness.wait_for("both shells to exit and be reaped", |_| {
        let reports = scratch.reports();
        reports.len() == 2 && reports.iter().all(|(pid, _)| gone(*pid))
    });
    harness.wait_for("the window to start installing", |_| {
        runtime.worker_hooks.held(Stage::Subscribe) == 1
    });
    runtime.worker_hooks.release(Stage::Subscribe);
    (harness, scratch)
}

#[gpui::test]
fn a_restored_shell_that_exits_before_its_view_closes_under_close_on_exit(
    cx: &mut TestAppContext,
) {
    let (mut harness, _scratch) = restore_exiting(cx, true);
    let runtime = Arc::clone(&harness.runtime);
    harness.wait_for("the exited panes to close", |_| {
        runtime.lock().sessions().is_empty()
    });
}

#[gpui::test]
fn a_restored_shell_that_exits_before_its_view_stays_without_close_on_exit(
    cx: &mut TestAppContext,
) {
    let (mut harness, _scratch) = restore_exiting(cx, false);
    harness.wait_settled();
    let tab = TabId::in_runtime(harness.runtime_id(), 3);
    let window =
        harness.window_of(SessionId::in_runtime(harness.runtime_id(), 1));
    harness.wait_for("the tab to show its exit", |cx| {
        terminal(cx, window, tab).is_some_and(|view| view.read(cx).exited)
    });
    assert_eq!(harness.runtime.lock().sessions().len(), 1);
}

#[gpui::test]
fn a_checkpoint_follows_a_change_and_drops_a_closed_tab(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut harness = DesktopHarness::restoring(
        cx,
        |_| {},
        capture(&scratch),
        Some(layout()),
        ECHO,
    );
    // Before the restore settles, so no timer waits on the default delay.
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().checkpoints.delay =
            Duration::from_millis(200);
    });
    harness.wait_settled();
    let recorder = harness.recorder.clone();
    harness.wait_for("the settle checkpoint", |_| !recorder.saved().is_empty());
    // Let the views' own changes after settling land first.
    harness.wait_for("checkpoints to go quiet", |cx| {
        cx.global::<Desktop>().checkpoints.idle()
    });
    let before = recorder.saved().len();
    let runtime = harness.runtime_id();
    let closed = TabId::in_runtime(runtime, 9);
    harness
        .runtime
        .lock()
        .close_tab(WorkspaceId::in_runtime(runtime, 2), closed)
        .unwrap();
    harness.wait_for("a checkpoint without the closed tab", |_| {
        recorder.saved().len() > before
            && recorder
                .saved()
                .last()
                .is_some_and(|(capture, _)| capture.tabs().count() == 2)
    });
    let (capture, layout) = harness.recorder.saved().pop().unwrap();
    assert!(capture.tabs().all(|tab| tab.id.get() != 9));
    assert!(
        layout
            .windows
            .iter()
            .all(|window| window.tab_history.iter().all(|tab| tab.get() != 9))
    );
    harness.runtime.terminate().unwrap();
}

#[gpui::test]
fn closing_the_last_window_writes_an_empty_set_before_exit(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.wait_text(window, tab, "READY");
    // Checkpoints wait an hour here, so only the exit can write.
    harness.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Window, window, cx);
    });
    let recorder = harness.recorder.clone();
    harness.wait_for("the exit write", |_| !recorder.saved().is_empty());
    let saved = recorder.saved();
    let [(capture, layout)] = saved.as_slice() else {
        panic!("one write: {saved:?}");
    };
    assert_eq!(capture.sessions, []);
    assert_eq!(layout.windows, []);
    assert_eq!(
        harness.runtime.persistence.state(),
        persistence::State::Final
    );
}

#[gpui::test]
fn turning_restore_off_deletes_saved_state_and_on_saves_again(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.wait_text(window, tab, "READY");
    let reload = |harness: &mut DesktopHarness, enabled: bool| {
        harness.cx.update(|cx| {
            let mut changed = cx.global::<Desktop>().config.clone();
            changed.general.restore_sessions = enabled;
            finish_reload(
                cx,
                ReloadKind::Manual,
                Path::new("huterm-test.toml"),
                config::Reloaded {
                    config: Ok(changed),
                    inputs: config::Inputs::default(),
                    missing: false,
                },
                0,
            );
        });
    };
    reload(&mut harness, false);
    let recorder = harness.recorder.clone();
    harness.wait_for("the saved files to go", |_| {
        recorder.recorded().removed == 1
    });
    assert_eq!(harness.runtime.persistence.state(), persistence::State::Off);
    assert_eq!(recorder.saved(), []);
    reload(&mut harness, true);
    harness.wait_for("a save as soon as it turns on", |_| {
        !recorder.saved().is_empty()
    });
    assert_eq!(recorder.saved()[0].0.sessions.len(), 1);
    // A reload that leaves it on deletes nothing.
    reload(&mut harness, true);
    assert_eq!(recorder.recorded().removed, 1);
    harness.runtime.terminate().unwrap();
}

#[gpui::test]
fn a_checkpoint_waits_for_a_reserved_tear_off_and_saves_no_tabless_session(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let second = harness.open_tab(ECHO);
    let source = harness.attach_showing(&[first, second]);
    harness.wait_text(source, first, "READY");
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Transfer);
    harness.invoke(source, ids::MOVE_TAB_TO_NEW_WINDOW).unwrap();
    harness.wait_for("the tear-off to reserve its destination", |cx| {
        !cx.global::<Desktop>().cross.reserved.is_empty()
            && runtime.worker_hooks.held(Stage::Transfer) == 1
    });
    // The destination session exists but holds no tab yet.
    assert_eq!(runtime.lock().sessions().len(), 2);
    harness.cx.update(|cx| {
        take_checkpoint(cx);
        assert!(cx.global::<Desktop>().checkpoints.waiting_transfer);
    });
    runtime.persistence.flush();
    assert!(harness.recorder.saved().is_empty(), "saved mid-transfer");
    runtime.worker_hooks.release(Stage::Transfer);
    let recorder = harness.recorder.clone();
    harness.wait_for("the checkpoint after the transfer settles", |_| {
        !recorder.saved().is_empty()
    });
    let (capture, layout) = recorder.saved().remove(0);
    assert_eq!(capture.sessions.len(), 2);
    assert!(capture.sessions.iter().all(|session| {
        session
            .workspaces
            .iter()
            .any(|workspace| !workspace.tabs.is_empty())
    }));
    assert_eq!(layout.windows.len(), 2);
    harness.runtime.terminate().unwrap();
}

#[gpui::test]
fn a_window_whose_quake_profile_is_gone_reopens_as_a_regular_window(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut saved_layout = layout();
    saved_layout.windows[0].quake_profile = Some("gone".into());
    let mut harness = DesktopHarness::restoring(
        cx,
        |_| {},
        capture(&scratch),
        Some(saved_layout),
        ECHO,
    );
    harness.wait_settled();
    let work = SessionId::in_runtime(harness.runtime_id(), 1);
    let window = harness.window_of(work);
    harness.cx.update(|cx| {
        let desktop = cx.global::<Desktop>();
        assert!(desktop.quake_stash.is_empty(), "nothing waits for a summon");
        let record = desktop.windows.record(window.window_id()).unwrap();
        assert!(record.quake.is_none());
    });
    harness.runtime.terminate().unwrap();
}

#[gpui::test]
fn a_restore_that_fails_keeps_the_saved_state_and_stops_saving(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut harness = DesktopHarness::installed(
        cx,
        |_| {},
        persistence::State::Bootstrapping,
    );
    harness.runtime.worker_hooks.fail(Stage::Restore);
    harness.cx.update(|cx| {
        // The fresh window a failed restore opens would start the user's
        // shell; this test only checks what happens to the saved state.
        cx.global_mut::<Desktop>().quitting = true;
        let saved = startup::Saved::new(capture(&scratch), Some(layout()));
        startup::start_with(cx, saved, fixture(ECHO));
    });
    harness.wait_settled();
    harness.runtime.persistence.flush();
    let recorded = harness.recorder.recorded();
    assert_eq!(recorded.removed, 0, "a failed restore deleted saved state");
    assert_eq!(recorded.saved, []);
    // The marker stays, so the next launch sets the files aside. With no
    // window left the desktop quit, from Off: a Quit during the restore
    // would have removed the marker.
    assert_eq!(recorded.cleared, 0);
    assert_eq!(
        harness.runtime.persistence.state(),
        persistence::State::Final
    );
    harness.cx.update(|cx| {
        let latched = &cx.global::<Desktop>().latched;
        assert!(
            latched.iter().any(|notice| notice.message
                == "Couldn't restore your previous windows: injected restore failure."),
            "{latched:?}"
        );
    });
}

#[gpui::test]
fn a_session_whose_panes_and_fresh_tab_all_fail_is_closed_with_a_warning(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut harness = DesktopHarness::installed(
        cx,
        |_| {},
        persistence::State::Bootstrapping,
    );
    let base = TerminalCommand {
        program: "/huterm-nonexistent-shell".into(),
        ..fixture(ECHO)
    };
    harness.cx.update(|cx| {
        // Keeps the fresh window an empty desktop opens from starting the
        // user's shell; the restore itself spawns only `base`.
        cx.global_mut::<Desktop>().quitting = true;
        let saved = startup::Saved::new(capture(&scratch), Some(layout()));
        startup::start_with(cx, saved, base);
    });
    harness.wait_settled();
    assert_eq!(harness.runtime.lock().sessions(), []);
    harness.cx.update(|cx| {
        let latched: Vec<_> = cx
            .global::<Desktop>()
            .latched
            .iter()
            .map(|notice| (notice.severity, notice.message.clone()))
            .collect();
        let closed = latched
            .iter()
            .filter(|(severity, message)| {
                *severity == Severity::Warning
                    && message.starts_with(
                        "A session couldn't start a new tab and was closed: ",
                    )
            })
            .count();
        assert_eq!(closed, 2, "{latched:?}");
    });
}
