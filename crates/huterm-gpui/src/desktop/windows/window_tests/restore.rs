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
            workspace_panel_collapsed: true,
            workspace_panel_width: 230.0,
            display: None,
            quake_profile: None,
            appearance: None,
            remembered: Vec::new(),
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
    harness.wait_text(window, scoped_tab(3), "READY");
    harness.cx.update(|cx| {
        let desktop = cx.global::<Desktop>();
        // The session no saved window showed comes back unviewed, with no
        // window of its own, as it was before Quit.
        assert!(desktop.hierarchy.state().session(unviewed).is_some());
        assert!(
            desktop
                .windows
                .records()
                .all(|record| record.session != Some(unviewed)),
            "no window shows the unviewed session"
        );
        assert_eq!(cx.windows().len(), 1);
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
    // Settling writes one checkpoint of what was restored.
    let recorder = harness.recorder.clone();
    harness.wait_for("the settle checkpoint", |_| !recorder.saved().is_empty());
    let (saved, layout) = harness.recorder.saved().remove(0);
    assert_eq!(saved.sessions.len(), 2);
    assert_eq!(layout.windows.len(), 1);
    assert_eq!(harness.recorder.recorded().marked, 1);
    assert_eq!(harness.recorder.recorded().cleared, 1);
    harness.runtime.terminate().unwrap();
    for (pid, _) in scratch.reports() {
        assert!(gone(pid), "shell {pid} outlived Quit");
    }
}

/// The session restore notices `window` shows.
fn restore_notices(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> Vec<String> {
    harness.update_view(window, |view, _, _| {
        view.notices
            .contents()
            .filter(|content| content.source == NoticeSource::Sessions)
            .map(|content| content.message.clone())
            .collect()
    })
}

#[gpui::test]
fn panes_that_start_in_an_earlier_saved_folder_are_reported(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let project = scratch.0.join("project");
    let gone_folder = scratch.0.join("gone");
    // Panes 5 and 15 lost their last folder but kept an earlier one; pane
    // 7 lost its only folder.
    let mut saved = capture(&scratch);
    for terminal in &mut saved.terminals {
        if matches!(terminal.id.get(), 6 | 16) {
            terminal.directories = vec![gone_folder.clone(), project.clone()];
        }
    }
    let mut harness = DesktopHarness::restoring(
        cx,
        |_| {},
        saved,
        Some(layout()),
        &scratch.reporting(),
    );
    harness.wait_settled();
    let runtime = harness.runtime_id();
    let work = harness.window_of(SessionId::in_runtime(runtime, 1));
    harness.wait_for("every shell to report", |_| scratch.reports().len() == 4);
    let project = project.canonicalize().unwrap();
    let started_in_project = scratch
        .reports()
        .iter()
        .filter(|(_, directory)| *directory == project)
        .count();
    assert_eq!(started_in_project, 3);
    // Session 12 comes back unviewed, so its notice joins the first
    // window's, named after it; the newest notice lists first.
    assert_eq!(
        restore_notices(&mut harness, work),
        [
            "In \"Session 2\": 1 pane started in an earlier folder because its last folder no longer exists.",
            "2 panes started in a different folder because their last folders no longer exist.",
        ]
    );
    harness.runtime.terminate().unwrap();
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

/// Holds a restore's worker after its first shell started, runs `quit` on
/// another thread, and checks that it returned only once that shell was
/// reaped, that nothing else started, and that nothing was saved.
fn quit_while_the_restore_spawns(
    cx: &mut TestAppContext,
    quit: impl FnOnce(&DesktopRuntime) + Send + 'static,
) {
    let scratch = Scratch::new();
    let mut harness = DesktopHarness::installed(
        cx,
        |_| {},
        persistence::State::Bootstrapping,
    );
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::RestoreSpawned);
    let saved = startup::Saved::new(capture(&scratch), Some(layout()));
    let base = fixture(&scratch.reporting());
    harness.cx.update(|cx| startup::start_with(cx, saved, base));
    harness.wait_for("the first restored shell to start", |_| {
        runtime.worker_hooks.held(Stage::RestoreSpawned) == 1
            && scratch.reports().len() == 1
    });
    let pid = scratch.reports()[0].0;
    let quitting = Arc::clone(&runtime);
    let quit = std::thread::spawn(move || {
        quit(&quitting);
        gone(pid)
    });
    harness.wait_for("Quit to wait for the restore worker", |_| {
        quit.is_finished() || runtime.restore_worker.waiting() == 1
    });
    runtime.worker_hooks.release(Stage::RestoreSpawned);
    assert!(
        quit.join().unwrap(),
        "Quit returned while restored shell {pid} still ran"
    );
    harness.wait_settled();
    assert_eq!(scratch.reports().len(), 1, "a shell started after Quit");
    assert_eq!(runtime.lock().terminal_count(), 0);
    let recorded = harness.recorder.recorded();
    assert_eq!(recorded.saved, [], "Quit during a restore saved state");
    assert_eq!(recorded.cleared, 1);
}

#[gpui::test]
fn committing_quit_while_the_restore_spawns_waits_for_its_shells(
    cx: &mut TestAppContext,
) {
    quit_while_the_restore_spawns(cx, |runtime| {
        let assessment =
            runtime.assess(CloseRequest::Application).result.unwrap();
        let windows = Some(huterm_state::DesktopLayout::default());
        runtime.commit(&assessment, true, windows).result.unwrap();
    });
}

#[gpui::test]
fn the_native_quit_cleanup_while_the_restore_spawns_waits_for_its_shells(
    cx: &mut TestAppContext,
) {
    quit_while_the_restore_spawns(cx, |runtime| runtime.terminate().unwrap());
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
    // The window drops the tab after the runtime does; a checkpoint taken
    // in between is followed by one that has the window's change too.
    harness.wait_for("a checkpoint without the closed tab", |_| {
        recorder.saved().len() > before
            && recorder.saved().last().is_some_and(|(capture, layout)| {
                capture.tabs().count() == 2
                    && layout.windows.iter().all(|window| {
                        window.tab_history.iter().all(|tab| tab.get() != 9)
                    })
            })
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

/// Applies a manual reload of the current configuration after `change`,
/// read once `renames` quake profile renames were noted.
fn reload_with(
    harness: &mut DesktopHarness,
    renames: u64,
    change: impl FnOnce(&mut Config),
) {
    harness.cx.update(|cx| {
        let mut changed = cx.global::<Desktop>().config.clone();
        change(&mut changed);
        finish_reload(
            cx,
            ReloadKind::Manual,
            Path::new("huterm-test.toml"),
            config::Reloaded {
                config: Ok(changed),
                inputs: config::Inputs::default(),
                missing: false,
            },
            renames,
        );
    });
}

/// Reloads with `[general] restore_sessions` set to `enabled`.
fn reload(harness: &mut DesktopHarness, enabled: bool) {
    reload_with(harness, 0, |config| {
        config.general.restore_sessions = enabled;
    });
}

#[gpui::test]
fn turning_restore_off_deletes_saved_state_and_on_saves_again(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    harness.wait_text(window, tab, "READY");
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
fn turning_restore_off_and_on_during_a_restore_saves_only_once_it_settles(
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
    harness
        .cx
        .update(|cx| startup::start_with(cx, saved, fixture(ECHO)));
    harness.wait_for("the restore to insert its shells", |_| {
        runtime.worker_hooks.held(Stage::Restored) == 1
    });
    reload(&mut harness, false);
    reload(&mut harness, true);
    harness.cx.run_until_parked();
    runtime.persistence.flush();
    let early = harness.recorder.recorded();
    assert_eq!(early.saved, [], "a checkpoint landed before settling");
    assert_eq!(early.removed, 0);
    assert_eq!(
        runtime.persistence.state(),
        persistence::State::Bootstrapping
    );
    runtime.worker_hooks.release(Stage::Restored);
    harness.wait_settled();
    let recorder = harness.recorder.clone();
    harness.wait_for("the settle checkpoint", |_| !recorder.saved().is_empty());
    assert_eq!(recorder.recorded().removed, 0);
    assert_eq!(runtime.persistence.state(), persistence::State::Running);
    runtime.terminate().unwrap();
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
fn a_checkpoint_whose_capture_meets_a_tear_off_waits_for_it_to_settle(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let second = harness.open_tab(ECHO);
    let source = harness.attach_showing(&[first, second]);
    harness.wait_text(source, first, "READY");
    let runtime = Arc::clone(&harness.runtime);
    // The capture starts before the tear-off reserves anything, and takes
    // the Mux lock only after it did.
    runtime.worker_hooks.hold(Stage::Checkpoint);
    runtime.worker_hooks.hold(Stage::Transfer);
    harness.cx.update(take_checkpoint);
    harness.wait_for("the capture to start", |_| {
        runtime.worker_hooks.held(Stage::Checkpoint) == 1
    });
    harness.invoke(source, ids::MOVE_TAB_TO_NEW_WINDOW).unwrap();
    harness.wait_for("the tear-off to reserve its destination", |cx| {
        !cx.global::<Desktop>().cross.reserved.is_empty()
            && runtime.worker_hooks.held(Stage::Transfer) == 1
    });
    runtime.worker_hooks.release(Stage::Checkpoint);
    harness.cx.run_until_parked();
    runtime.persistence.flush();
    assert_eq!(
        harness.recorder.saved(),
        [],
        "saved a capture that met a tear-off"
    );
    runtime.worker_hooks.release(Stage::Transfer);
    let recorder = harness.recorder.clone();
    harness.wait_for("the checkpoint after the transfer settles", |_| {
        !recorder.saved().is_empty()
    });
    let (capture, layout) = recorder.saved().remove(0);
    assert_eq!(capture.sessions.len(), 2);
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

/// The Quake profile whose session waits for its first summon.
const DROP: &str = "drop";

/// [`layout`] with session 1's window as profile [`DROP`]'s.
fn quake_layout() -> huterm_state::DesktopLayout {
    let mut saved = layout();
    saved.windows[0].quake_profile = Some(DROP.into());
    saved
}

impl DesktopHarness {
    /// A settled desktop with profile [`DROP`], which a registered shortcut
    /// summons, restored from `capture` and `layout`.
    fn restoring_quake(
        cx: &mut TestAppContext,
        capture: RuntimeCapture,
        layout: huterm_state::DesktopLayout,
    ) -> Self {
        let mut harness = Self::installed(
            cx,
            |config| {
                config.quake.profiles.insert(
                    DROP.into(),
                    huterm_config::quake::Profile::default(),
                );
            },
            persistence::State::Bootstrapping,
        );
        harness.cx.update(|cx| {
            let desktop = cx.global_mut::<Desktop>();
            desktop.quake.summoned.insert(DROP.into());
            // Before the restore settles, so no timer waits on the
            // harness's hour-long delay.
            desktop.checkpoints.delay = Duration::from_millis(200);
            let saved = startup::Saved::new(capture, Some(layout));
            startup::start_with(cx, saved, fixture(ECHO));
        });
        harness.wait_settled();
        harness
    }

    /// The profiles whose sessions wait for their first summon.
    fn stashed(&mut self) -> Vec<String> {
        self.cx.update(|cx| {
            let mut names: Vec<String> =
                cx.global::<Desktop>().quake_stash.keys().cloned().collect();
            names.sort();
            names
        })
    }

    /// Whether an open window shows `session`.
    fn showing(&mut self, session: SessionId) -> bool {
        self.cx.update(|cx| {
            cx.global::<Desktop>()
                .windows
                .records()
                .any(|record| record.session == Some(session))
        })
    }

    /// The saved window of session 1, as a capture now writes it.
    fn saved_work_window(&mut self) -> Option<huterm_state::WindowLayout> {
        self.cx.update(|cx| {
            capture_windows(cx)
                .windows
                .into_iter()
                .find(|window| window.session == SessionId::new(1))
        })
    }
}

#[gpui::test]
fn a_stashed_quake_session_is_saved_and_waits_again_after_a_relaunch(
    cx: &mut TestAppContext,
    relaunched: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut harness =
        DesktopHarness::restoring_quake(cx, capture(&scratch), quake_layout());
    assert_eq!(harness.stashed(), [DROP]);
    assert!(!harness.showing(SessionId::in_runtime(harness.runtime_id(), 1)));
    let recorder = harness.recorder.clone();
    harness.wait_for("the settle checkpoint", |_| !recorder.saved().is_empty());
    let (saved, saved_layout) = recorder.saved().remove(0);
    let stashed = saved_layout
        .windows
        .iter()
        .find(|window| window.session == SessionId::new(1));
    assert_eq!(
        stashed.and_then(|window| window.quake_profile.as_deref()),
        Some(DROP),
        "{saved_layout:?}"
    );
    harness.runtime.terminate().unwrap();
    let mut relaunch =
        DesktopHarness::restoring_quake(relaunched, saved, saved_layout);
    assert_eq!(relaunch.stashed(), [DROP]);
    assert!(!relaunch.showing(SessionId::in_runtime(relaunch.runtime_id(), 1)));
    relaunch.runtime.terminate().unwrap();
}

#[gpui::test]
fn a_renamed_quake_profile_keeps_its_stashed_session(cx: &mut TestAppContext) {
    let scratch = Scratch::new();
    let mut harness =
        DesktopHarness::restoring_quake(cx, capture(&scratch), quake_layout());
    harness.cx.update(|cx| {
        let desktop = cx.global_mut::<Desktop>();
        let rename = desktop.windows.quake_rename_started(DROP, "dropdown");
        desktop.windows.quake_rename_settled(rename, true);
        desktop.quake.summoned.insert("dropdown".into());
    });
    reload_with(&mut harness, 1, |config| {
        let profile = config.quake.profiles.remove(DROP).unwrap();
        config.quake.profiles.insert("dropdown".into(), profile);
    });
    assert_eq!(harness.stashed(), ["dropdown"]);
    assert!(!harness.showing(SessionId::in_runtime(harness.runtime_id(), 1)));
    let saved = harness.saved_work_window().expect("the stash is saved");
    assert_eq!(saved.quake_profile.as_deref(), Some("dropdown"));
    // A checkpoint carries the rename, so a crash keeps it.
    let recorder = harness.recorder.clone();
    harness.wait_for("a checkpoint with the renamed profile", |_| {
        recorder.saved().last().is_some_and(|(_, layout)| {
            layout.windows.iter().any(|window| {
                window.session == SessionId::new(1)
                    && window.quake_profile.as_deref() == Some("dropdown")
            })
        })
    });
    harness.runtime.terminate().unwrap();
}

/// Restores session 1 into profile [`DROP`]'s stash, applies `change`, and
/// checks that the session then shows in a regular window.
fn a_stash_its_profile_cannot_summon_opens(
    cx: &mut TestAppContext,
    change: impl FnOnce(&mut DesktopHarness),
) {
    let scratch = Scratch::new();
    let mut harness =
        DesktopHarness::restoring_quake(cx, capture(&scratch), quake_layout());
    change(&mut harness);
    assert_eq!(harness.stashed(), Vec::<String>::new());
    let window =
        harness.window_of(SessionId::in_runtime(harness.runtime_id(), 1));
    harness.cx.update(|cx| {
        let record = cx
            .global::<Desktop>()
            .windows
            .record(window.window_id())
            .unwrap();
        assert!(record.quake.is_none());
    });
    let saved = harness.saved_work_window().expect("the window is saved");
    assert_eq!(saved.quake_profile, None);
    harness.runtime.terminate().unwrap();
}

#[gpui::test]
fn a_stashed_session_whose_profile_is_removed_opens_as_a_regular_window(
    cx: &mut TestAppContext,
) {
    a_stash_its_profile_cannot_summon_opens(cx, |harness| {
        reload_with(harness, 0, |config| {
            config.quake.profiles.remove(DROP);
        });
    });
}

#[gpui::test]
fn a_stashed_session_whose_shortcut_is_gone_opens_as_a_regular_window(
    cx: &mut TestAppContext,
) {
    a_stash_its_profile_cannot_summon_opens(cx, |harness| {
        harness.cx.update(|cx| {
            cx.global_mut::<Desktop>().quake.summoned.clear();
        });
        reload_with(harness, 0, |_| {});
    });
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
        // A window without a shell keeps the desktop from exiting.
        open_window_inner(cx, false);
        let saved = startup::Saved::new(capture(&scratch), Some(layout()));
        startup::start_with(cx, saved, fixture(ECHO));
    });
    harness.wait_settled();
    harness.cx.update(|cx| {
        let latched = &cx.global::<Desktop>().latched;
        assert!(
            latched.iter().any(|notice| notice.message
                == "Couldn't restore your previous windows: injected restore failure."),
            "{latched:?}"
        );
    });
    // Turning the setting on again does not resume saving.
    reload(&mut harness, true);
    harness.cx.run_until_parked();
    harness.runtime.persistence.flush();
    let recorded = harness.recorder.recorded();
    assert_eq!(recorded.saved, [], "saved after a failed restore");
    assert_eq!(recorded.removed, 0, "a failed restore deleted saved state");
    assert_eq!(harness.runtime.persistence.state(), persistence::State::Off);
    harness.runtime.terminate().unwrap();
    let recorded = harness.recorder.recorded();
    assert_eq!(recorded.saved, []);
    // The marker stays, so the next launch sets the files aside. Quit came
    // from Off: a Quit during the restore would have removed the marker.
    assert_eq!(recorded.cleared, 0);
    assert_eq!(
        harness.runtime.persistence.state(),
        persistence::State::Final
    );
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

/// [`layout`] with a second window on session 1, which last showed tab 9
/// and is the one to activate.
fn shared_layout() -> huterm_state::DesktopLayout {
    let mut saved = layout();
    let mut second = saved.windows[0].clone();
    second.active_tab = Some(TabId::new(9));
    second.tab_history = vec![TabId::new(3), TabId::new(9)];
    second.bounds.x = 400.0;
    saved.windows.push(second);
    saved.active_window = Some(1);
    saved
}

impl DesktopHarness {
    /// The open windows attached to `session`, in opening order.
    fn windows_of(&mut self, session: SessionId) -> Vec<AnyWindowHandle> {
        self.cx.update(|cx| {
            let ids: Vec<gpui::WindowId> = cx
                .global::<Desktop>()
                .windows
                .records()
                .filter(|record| record.session == Some(session))
                .map(|record| record.id)
                .collect();
            ids.into_iter()
                .filter_map(|id| {
                    cx.windows()
                        .into_iter()
                        .find(|handle| handle.window_id() == id)
                })
                .collect()
        })
    }
}

#[gpui::test]
fn two_saved_windows_on_one_session_come_back_with_their_own_navigation(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut harness = DesktopHarness::restoring(
        cx,
        |_| {},
        capture(&scratch),
        Some(shared_layout()),
        ECHO,
    );
    harness.wait_settled();
    let runtime = harness.runtime_id();
    let tab = |id| TabId::in_runtime(runtime, id);
    let work = SessionId::in_runtime(runtime, 1);
    let windows = harness.windows_of(work);
    assert_eq!(windows.len(), 2, "both saved windows show the session");
    harness.cx.update(|cx| {
        let desktop = cx.global::<Desktop>();
        let record = |window: AnyWindowHandle| {
            desktop.windows.record(window.window_id()).unwrap()
        };
        let (first, second) = (record(windows[0]), record(windows[1]));
        assert_ne!(first.attachment, second.attachment);
        assert_eq!(first.active, Some(tab(3)));
        assert_eq!(second.active, Some(tab(9)));
        assert_eq!(
            desktop.hierarchy.state().session_attachments(work).count(),
            2
        );
        // The saved active window is the second, by its saved identity.
        assert_eq!(
            cx.active_window().map(|handle| handle.window_id()),
            Some(windows[1].window_id())
        );
    });
    harness.runtime.terminate().unwrap();
}

#[gpui::test]
fn a_restored_window_that_fails_to_open_detaches_only_its_attachment(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut harness = DesktopHarness::installed(
        cx,
        |_| {},
        persistence::State::Bootstrapping,
    );
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().fail_window_opens = 1;
        let saved =
            startup::Saved::new(capture(&scratch), Some(shared_layout()));
        startup::start_with(cx, saved, fixture(ECHO));
    });
    harness.wait_settled();
    let runtime = harness.runtime_id();
    let work = SessionId::in_runtime(runtime, 1);
    let windows = harness.windows_of(work);
    assert_eq!(windows.len(), 1, "the second saved window still opened");
    harness.wait_projection();
    harness.cx.update(|cx| {
        let desktop = cx.global::<Desktop>();
        let state = desktop.hierarchy.state();
        assert!(state.session(work).is_some(), "its session was closed");
        assert_eq!(state.session_attachments(work).count(), 1);
    });
    // The next window to open raised the warning.
    harness.cx.update(|cx| {
        let shown: Vec<String> = cx
            .windows()
            .into_iter()
            .flat_map(|window| notices(cx, window))
            .collect();
        assert!(
            shown
                .iter()
                .any(|notice| notice.contains("A window couldn't open.")),
            "{shown:?}"
        );
    });
    harness.runtime.terminate().unwrap();
}

/// [`layout`] with its window on session 12, remembering where it was in
/// session 1 (tab 3's pane 7, after tab 9) and in a session that is gone.
fn remembering_layout() -> huterm_state::DesktopLayout {
    let mut saved = layout();
    let window = &mut saved.windows[0];
    window.session = SessionId::new(12);
    window.workspace = WorkspaceId::new(13);
    window.active_tab = Some(TabId::new(14));
    window.tab_history = vec![TabId::new(14)];
    window.focused_panes = Vec::new();
    window.remembered = vec![
        huterm_state::SessionNavigation {
            session: SessionId::new(1),
            workspace: WorkspaceId::new(2),
            active_tab: Some(TabId::new(3)),
            tab_history: vec![TabId::new(9), TabId::new(3)],
            focused_panes: vec![huterm_state::FocusedPane {
                tab: TabId::new(3),
                pane: PaneId::new(7),
            }],
        },
        huterm_state::SessionNavigation {
            session: SessionId::new(99),
            workspace: WorkspaceId::new(98),
            active_tab: None,
            tab_history: Vec::new(),
            focused_panes: Vec::new(),
        },
    ];
    saved
}

#[gpui::test]
fn saved_navigation_memory_round_trips_and_a_later_switch_lands_on_it(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut harness = DesktopHarness::restoring(
        cx,
        |_| {},
        capture(&scratch),
        Some(remembering_layout()),
        ECHO,
    );
    harness.wait_settled();
    let runtime = harness.runtime_id();
    let tab = |id| TabId::in_runtime(runtime, id);
    let work = SessionId::in_runtime(runtime, 1);
    let unviewed = SessionId::in_runtime(runtime, 12);
    let window = harness.window_of(unviewed);
    // The capture writes the memory back, without the session that is gone.
    let saved = harness.cx.update(|cx| {
        capture_windows(cx)
            .windows
            .into_iter()
            .find(|saved| saved.session == SessionId::new(12))
            .unwrap()
    });
    assert_eq!(
        saved.remembered,
        remembering_layout().windows[0].remembered[..1]
    );
    // Switching lands on the remembered tab and pane.
    harness.update_view(window, |view, window, cx| {
        view.switch_session(
            session_switch::SwitchTarget::Existing(work),
            window,
            cx,
        )
        .unwrap();
    });
    harness.wait_for("the remembered session to show", |cx| {
        cx.global::<Desktop>()
            .windows
            .record(window.window_id())
            .is_some_and(|record| record.session == Some(work))
            && window
                .update(cx, |root, _, cx| {
                    let view = root.downcast::<WorkspaceView>().unwrap();
                    let view = view.read(cx);
                    view.switch.is_none() && view.tabs.len() == 2
                })
                .unwrap_or(false)
    });
    let (active, focused) = harness.update_view(window, |view, _, cx| {
        let focused = view
            .tabs
            .iter()
            .find(|view| view.id == tab(3))
            .map(|view| view.focused().pane);
        (view.active_tab(cx), focused)
    });
    assert_eq!(active, Some(tab(3)));
    assert_eq!(focused, Some(PaneId::in_runtime(runtime, 7)));
    harness.runtime.terminate().unwrap();
}

#[gpui::test]
fn a_stash_whose_session_closes_is_dropped(cx: &mut TestAppContext) {
    let scratch = Scratch::new();
    let mut harness =
        DesktopHarness::restoring_quake(cx, capture(&scratch), quake_layout());
    assert_eq!(harness.stashed(), [DROP]);
    let work = SessionId::in_runtime(harness.runtime_id(), 1);
    harness.runtime.lock().close_session(work).unwrap();
    harness.wait_projection();
    assert_eq!(harness.stashed(), Vec::<String>::new());
    harness.runtime.terminate().unwrap();
}

#[gpui::test]
fn without_a_saved_window_each_restored_session_opens_its_own(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut harness = DesktopHarness::restoring(
        cx,
        |_| {},
        capture(&scratch),
        None,
        &scratch.reporting(),
    );
    harness.wait_settled();
    let runtime = harness.runtime_id();
    let work = harness.window_of(SessionId::in_runtime(runtime, 1));
    let other = harness.window_of(SessionId::in_runtime(runtime, 12));
    assert_ne!(work, other);
    harness.wait_text(other, TabId::in_runtime(runtime, 14), "READY");
}

/// [`capture`] with session 12's pane 15 started in an earlier folder, so
/// that session raises a restore notice.
fn capture_with_unviewed_notice(scratch: &Scratch) -> RuntimeCapture {
    let mut saved = capture(scratch);
    for terminal in &mut saved.terminals {
        if terminal.id.get() == 16 {
            terminal.directories =
                vec![scratch.0.join("gone"), scratch.0.join("project")];
        }
    }
    saved
}

#[gpui::test]
fn when_every_saved_window_fails_to_open_the_unviewed_sessions_open_instead(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut harness = DesktopHarness::installed(
        cx,
        |_| {},
        persistence::State::Bootstrapping,
    );
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().fail_window_opens = 1;
        let saved = startup::Saved::new(
            capture_with_unviewed_notice(&scratch),
            Some(layout()),
        );
        startup::start_with(cx, saved, fixture(ECHO));
    });
    harness.wait_settled();
    harness.wait_projection();
    let runtime = harness.runtime_id();
    let unviewed = SessionId::in_runtime(runtime, 12);
    let window = harness.window_of(unviewed);
    // No fresh session: the saved window's own session closed with it, and
    // the unviewed one took the window.
    assert_eq!(harness.runtime.lock().sessions().len(), 1);
    harness.wait_text(window, TabId::in_runtime(runtime, 14), "READY");
    let notices = restore_notices(&mut harness, window);
    assert!(
        notices.iter().any(|notice| notice
            == "1 pane started in an earlier folder because its last folder no longer exists."),
        "{notices:?}"
    );
    harness.runtime.terminate().unwrap();
}

#[gpui::test]
fn unviewed_sessions_notices_go_to_the_first_window_that_opened(
    cx: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut harness = DesktopHarness::installed(
        cx,
        |_| {},
        persistence::State::Bootstrapping,
    );
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().fail_window_opens = 1;
        let saved = startup::Saved::new(
            capture_with_unviewed_notice(&scratch),
            Some(shared_layout()),
        );
        startup::start_with(cx, saved, fixture(ECHO));
    });
    harness.wait_settled();
    let runtime = harness.runtime_id();
    let windows = harness.windows_of(SessionId::in_runtime(runtime, 1));
    assert_eq!(windows.len(), 1, "the second saved window opened");
    // Session 12 stays unviewed, as it was before Quit.
    let records = harness
        .cx
        .update(|cx| cx.global::<Desktop>().windows.records().count());
    assert_eq!(records, 1);
    let notices = restore_notices(&mut harness, windows[0]);
    assert!(
        notices.iter().any(|notice| notice.starts_with(
            "In \"Session 2\": 1 pane started in an earlier folder"
        )),
        "{notices:?}"
    );
    harness.runtime.terminate().unwrap();
}

/// [`capture`] with a second workspace in session 1: workspace 17 with tab
/// 18, whose pane 19 runs terminal 20.
fn two_workspace_capture(scratch: &Scratch) -> RuntimeCapture {
    let mut saved = capture(scratch);
    saved.next_id = 21;
    saved.workspace_ordinal = 3;
    saved.sessions[0].workspaces.push(WorkspaceCapture {
        info: WorkspaceInfo {
            id: WorkspaceId::new(17),
            custom_name: None,
            automatic_name: "Workspace 3".into(),
        },
        tabs: vec![TabInfo {
            id: TabId::new(18),
            custom_name: None,
            layout: PaneLayout::new(pane(19, 20, None)).unwrap(),
            zoomed: None,
            input_sync: false,
        }],
    });
    saved
        .terminals
        .push(terminal_record(20, &scratch.0.join("project")));
    saved
}

/// A saved navigation entry for `workspace` of `session`.
fn navigation(
    session: u64,
    workspace: u64,
    active: Option<u64>,
) -> huterm_state::SessionNavigation {
    huterm_state::SessionNavigation {
        session: SessionId::new(session),
        workspace: WorkspaceId::new(workspace),
        active_tab: active.map(TabId::new),
        tab_history: active.map(TabId::new).into_iter().collect(),
        focused_panes: Vec::new(),
    }
}

#[gpui::test]
fn restore_drops_stale_workspace_memory(cx: &mut TestAppContext) {
    let scratch = Scratch::new();
    let mut saved = layout();
    saved.windows[0].remembered = vec![
        navigation(1, 17, Some(18)),
        // A workspace that did not come back.
        navigation(1, 99, None),
        // A workspace saved under a session it does not belong to.
        navigation(12, 17, None),
        // An older entry for a workspace already remembered.
        navigation(1, 17, None),
        // The workspace the window shows, whose own navigation applies.
        navigation(1, 2, Some(9)),
    ];
    // A second window whose shown workspace is gone falls back to the
    // session's first.
    let mut fallback = saved.windows[0].clone();
    fallback.workspace = WorkspaceId::new(99);
    fallback.remembered = Vec::new();
    saved.windows.push(fallback);
    let mut harness = DesktopHarness::restoring(
        cx,
        |_| {},
        two_workspace_capture(&scratch),
        Some(saved),
        ECHO,
    );
    harness.wait_settled();
    let runtime = harness.runtime_id();
    let work = SessionId::in_runtime(runtime, 1);
    let windows = harness.windows_of(work);
    assert_eq!(windows.len(), 2);
    harness.cx.update(|cx| {
        let desktop = cx.global::<Desktop>();
        let first = desktop.windows.record(windows[0].window_id()).unwrap();
        let remembered: Vec<_> = first
            .memory
            .iter()
            .map(|memory| (memory.session, memory.workspace, memory.active))
            .collect();
        assert_eq!(
            remembered,
            [(
                work,
                WorkspaceId::in_runtime(runtime, 17),
                Some(TabId::in_runtime(runtime, 18))
            )]
        );
        let second = desktop.windows.record(windows[1].window_id()).unwrap();
        assert_eq!(second.workspace, Some(WorkspaceId::in_runtime(runtime, 2)));
        assert_eq!(second.active, Some(TabId::in_runtime(runtime, 3)));
    });
    // The capture writes back only what applied.
    let saved = harness.saved_work_window().unwrap();
    assert_eq!(saved.remembered, [navigation(1, 17, Some(18))]);
    harness.runtime.terminate().unwrap();
}

impl DesktopHarness {
    /// Switches `window` to `workspace` and selects `active` there once it
    /// shows.
    fn show_selecting(
        &mut self,
        window: AnyWindowHandle,
        workspace: WorkspaceId,
        active: TabId,
    ) {
        self.wait_projection();
        self.invoke_with(
            window,
            &CommandInvocation::new(
                ids::SELECT_WORKSPACE,
                vec![CommandArgument::new(
                    "workspace",
                    CommandValue::Workspace(workspace),
                )],
            ),
        )
        .unwrap();
        self.wait_for("the workspace to show", |cx| {
            cx.global::<Desktop>()
                .windows
                .record(window.window_id())
                .is_some_and(|record| record.workspace == Some(workspace))
                && terminal(cx, window, active).is_some()
                && !busy(cx, window)
        });
        self.update_view(window, |view, window, cx| {
            view.select(active, window, cx);
        });
    }

    /// The workspace `window` shows, its active tab, and each remembered
    /// workspace with its active tab.
    #[expect(
        clippy::type_complexity,
        reason = "a test's view of one window's navigation"
    )]
    fn navigation_of(
        &mut self,
        window: AnyWindowHandle,
    ) -> (
        Option<WorkspaceId>,
        Option<TabId>,
        Vec<(WorkspaceId, Option<TabId>)>,
    ) {
        self.cx.update(|cx| {
            let record = cx
                .global::<Desktop>()
                .windows
                .record(window.window_id())
                .unwrap();
            (
                record.workspace,
                record.active,
                record
                    .memory
                    .iter()
                    .map(|memory| (memory.workspace, memory.active))
                    .collect(),
            )
        })
    }
}

#[gpui::test]
fn two_windows_on_two_workspaces_restore_their_workspaces(
    cx: &mut TestAppContext,
    relaunched: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (a, b) = (harness.open_tab(ECHO), harness.open_tab(ECHO));
    let (second, c, d) = {
        let mut mux = harness.runtime.lock();
        let second = mux.create_workspace(harness.session, None).unwrap();
        let c = mux.open_tab(second, &fixture(ECHO)).unwrap().tab.id;
        let d = mux.open_tab(second, &fixture(ECHO)).unwrap().tab.id;
        (second, c, d)
    };
    // Window one ends on the first workspace at b, remembering c in the
    // second; window two ends on the second at d, remembering a.
    let one = harness.attach_showing(&[a, b]);
    harness.show_selecting(one, second, c);
    let first_workspace = harness.workspace;
    harness.show_selecting(one, first_workspace, b);
    let two = harness.attach_showing(&[a, b]);
    harness.show_selecting(two, second, d);
    harness.resume_quits();
    harness.cx.update(request_quit);
    harness.wait_quit(harness.session);
    let (capture, saved) = harness.recorder.saved().remove(0);

    let mut relaunch = DesktopHarness::restoring(
        relaunched,
        |_| {},
        capture,
        Some(saved),
        ECHO,
    );
    relaunch.wait_settled();
    let runtime = relaunch.runtime_id();
    let tab = |id: TabId| TabId::in_runtime(runtime, id.get());
    let workspace =
        |id: WorkspaceId| WorkspaceId::in_runtime(runtime, id.get());
    let windows = relaunch
        .windows_of(SessionId::in_runtime(runtime, harness.session.get()));
    assert_eq!(windows.len(), 2);
    let (first, other) = if relaunch.navigation_of(windows[0]).0
        == Some(workspace(harness.workspace))
    {
        (windows[0], windows[1])
    } else {
        (windows[1], windows[0])
    };
    assert_eq!(
        relaunch.navigation_of(first),
        (
            Some(workspace(harness.workspace)),
            Some(tab(b)),
            vec![(workspace(second), Some(tab(c)))]
        )
    );
    assert_eq!(
        relaunch.navigation_of(other),
        (
            Some(workspace(second)),
            Some(tab(d)),
            vec![(workspace(harness.workspace), Some(tab(a)))]
        )
    );
    // Each lands on its own remembered tab when it switches.
    relaunch.show_selecting(first, workspace(second), tab(c));
    relaunch.show_selecting(other, workspace(harness.workspace), tab(a));
    relaunch.cx.update(|cx| {
        let records = &cx.global::<Desktop>().windows;
        assert_eq!(
            records.record(first.window_id()).unwrap().active,
            Some(tab(c))
        );
        assert_eq!(
            records.record(other.window_id()).unwrap().active,
            Some(tab(a))
        );
    });
    relaunch.runtime.terminate().unwrap();
}

#[gpui::test]
fn a_session_left_on_an_empty_workspace_survives_quit_and_restore(
    cx: &mut TestAppContext,
    relaunched: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let spare = harness
        .runtime
        .lock()
        .create_workspace(harness.session, Some("spare"))
        .unwrap();
    let window = harness.attach_showing(&[tab]);
    harness.wait_projection();
    // Closing the window's last tab moves it to the empty sibling, so its
    // session holds no tab while the window still shows it.
    harness.activate(window, tab);
    harness.invoke(window, ids::CLOSE_TAB).unwrap();
    let shown = window.window_id();
    harness.wait_for("the window to show the empty workspace", |cx| {
        cx.global::<Desktop>()
            .windows
            .record(shown)
            .is_some_and(|record| record.workspace == Some(spare))
    });
    harness.resume_quits();
    harness.cx.update(request_quit);
    harness.wait_quit(harness.session);
    let (capture, saved) = harness.recorder.saved().remove(0);
    assert_eq!(capture.sessions.len(), 1, "{capture:?}");

    let mut relaunch = DesktopHarness::restoring(
        relaunched,
        |_| {},
        capture,
        Some(saved),
        ECHO,
    );
    relaunch.wait_settled();
    let runtime = relaunch.runtime_id();
    let session = SessionId::in_runtime(runtime, harness.session.get());
    let spare = WorkspaceId::in_runtime(runtime, spare.get());
    // The session and its named workspace come back, with the fresh tab
    // restore gives a session that held none, and the window shows it.
    let windows = relaunch.windows_of(session);
    assert_eq!(windows.len(), 1);
    assert_eq!(relaunch.navigation_of(windows[0]).0, Some(spare));
    {
        let mux = relaunch.runtime.lock();
        let workspace = mux.workspace(spare).unwrap();
        assert_eq!(workspace.custom_name(), Some("spare"));
        assert_eq!(workspace.tabs.len(), 1);
    }
    relaunch.runtime.terminate().unwrap();
}

#[gpui::test]
fn a_quake_stash_on_a_closed_workspace_shows_another(
    cx: &mut TestAppContext,
    relaunched: &mut TestAppContext,
) {
    let scratch = Scratch::new();
    let mut saved = quake_layout();
    let mut remembered = navigation(1, 17, Some(18));
    remembered.focused_panes = vec![huterm_state::FocusedPane {
        tab: TabId::new(18),
        pane: PaneId::new(19),
    }];
    saved.windows[0].remembered = vec![remembered];
    let mut harness = DesktopHarness::restoring_quake(
        cx,
        two_workspace_capture(&scratch),
        saved,
    );
    assert_eq!(harness.stashed(), [DROP]);
    let runtime = harness.runtime_id();
    let kept = WorkspaceId::in_runtime(runtime, 17);
    harness
        .runtime
        .lock()
        .close_workspace(WorkspaceId::in_runtime(runtime, 2))
        .unwrap();
    harness.wait_projection();
    // The stash now names the workspace it remembers, with that
    // workspace's navigation, and a checkpoint saves that.
    assert_eq!(harness.stashed(), [DROP]);
    let recorder = harness.recorder.clone();
    let stashed = |layout: &huterm_state::DesktopLayout| {
        layout
            .windows
            .iter()
            .find(|window| window.quake_profile.as_deref() == Some(DROP))
            .filter(|window| window.workspace == WorkspaceId::new(17))
            .cloned()
    };
    harness.wait_for("a checkpoint naming the kept workspace", |_| {
        recorder
            .saved()
            .last()
            .is_some_and(|(_, layout)| stashed(layout).is_some())
    });
    let (checkpoint, layout) = recorder.saved().pop().unwrap();
    let window = stashed(&layout).unwrap();
    assert_eq!(window.active_tab, Some(TabId::new(18)));
    assert_eq!(window.tab_history, [TabId::new(18)]);
    assert_eq!(
        window.focused_panes,
        [huterm_state::FocusedPane {
            tab: TabId::new(18),
            pane: PaneId::new(19),
        }]
    );
    assert!(window.remembered.is_empty(), "{:?}", window.remembered);
    // A relaunch from that checkpoint stashes the kept workspace with its
    // remembered tab and pane.
    let mut relaunch =
        DesktopHarness::restoring_quake(relaunched, checkpoint, layout);
    assert_eq!(relaunch.stashed(), [DROP]);
    let relaunched_runtime = relaunch.runtime_id();
    relaunch.cx.update(|cx| {
        let reopen = quake_windows::take_stash(cx, DROP).unwrap();
        let tab = TabId::in_runtime(relaunched_runtime, 18);
        assert_eq!(
            reopen.workspace,
            WorkspaceId::in_runtime(relaunched_runtime, 17)
        );
        assert_eq!(reopen.focus.active, Some(tab));
        assert_eq!(
            reopen.focus.panes,
            [(tab, PaneId::in_runtime(relaunched_runtime, 19))]
        );
    });
    relaunch.runtime.terminate().unwrap();
    // The summon takes the stash and shows the remembered workspace.
    let before = harness.cx.update(|cx| cx.windows());
    harness.cx.update(|cx| {
        let reopen = quake_windows::take_stash(cx, DROP).unwrap();
        assert_eq!(reopen.workspace, kept);
        assert_eq!(reopen.focus.active, Some(TabId::in_runtime(runtime, 18)));
        open_window_with_profile(cx, false, None, None, Some(reopen));
    });
    let window = harness.cx.update(|cx| {
        cx.windows()
            .into_iter()
            .find(|handle| !before.contains(handle))
            .unwrap()
    });
    let shown = TabId::in_runtime(runtime, 18);
    harness.wait_for("the kept workspace to show", |cx| {
        terminal(cx, window, shown).is_some()
    });
    harness.cx.update(|cx| {
        let record = cx.global::<Desktop>().windows.record(window.window_id());
        assert_eq!(record.unwrap().workspace, Some(kept));
    });
    harness.runtime.terminate().unwrap();
}
