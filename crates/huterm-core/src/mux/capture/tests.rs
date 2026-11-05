use std::time::{Duration, Instant, SystemTime};

use super::*;
use crate::mux::tests::{command, shell_pid, wait_for_text};
use crate::mux::{HierarchyRecvError, HierarchySubscription};
use crate::test_support::TestClient;
use huterm_protocol::{
    ApplyOutcome, CaptureError, HierarchyState, NodeId, PaneInfo,
    SplitDirection, TerminalInput,
};

fn idle() -> TerminalCommand {
    command("printf READY; read value")
}

/// A missing program, which fails to spawn.
fn failing() -> TerminalCommand {
    let mut failing = idle();
    failing.program = "/huterm-nonexistent-shell".into();
    failing
}

struct Fixture {
    mux: Mux,
    main: SessionId,
    editor: TabId,
    /// The editor tab's zoomed pane.
    zoomed: PaneInfo,
    unviewed: SessionId,
    unviewed_tab: TabId,
    tabless: SessionId,
}

/// Session "main" holds workspace "code", with an "editor" tab split into
/// three panes with custom weights, a renamed pane, shared zoom, and input
/// sync, plus a plain tab, and an automatic workspace with one tab. A
/// tabless session follows, as a tear-off leaves one in flight, then an
/// unviewed session with one tab whose only attachment detached.
fn fixture() -> Fixture {
    let mut mux = Mux::default();
    let main = mux.create_session(Some("main")).unwrap();
    let code = mux.create_workspace(main, Some("code")).unwrap();
    let editor = mux.open_tab(code, &idle()).unwrap().tab.id;
    let first = mux.tab(editor).unwrap().first_pane().id;
    let second = mux
        .split_pane(code, editor, first, SplitDirection::Right, &idle())
        .unwrap()
        .pane;
    let third = mux
        .split_pane(code, editor, second.id, SplitDirection::Down, &idle())
        .unwrap()
        .pane;
    let PaneNode::Split(root) =
        mux.tab(editor).unwrap().layout().root().clone()
    else {
        panic!("the editor tab is not split");
    };
    let children: Vec<NodeId> =
        root.children.iter().map(|child| child.node.id()).collect();
    mux.resize_split(code, editor, root.id, &children, &[300_000, 700_000])
        .unwrap();
    mux.rename_tab(editor, Some("editor")).unwrap();
    mux.rename_pane(third.id, Some("logs")).unwrap();
    mux.set_pane_zoom(code, editor, Some(second.id)).unwrap();
    mux.set_input_sync(code, editor, true).unwrap();
    mux.open_tab(code, &idle()).unwrap();
    let scratch = mux.create_workspace(main, None).unwrap();
    mux.open_tab(scratch, &idle()).unwrap();
    let tabless = mux.create_session(Some("transfer")).unwrap();
    mux.create_workspace(tabless, None).unwrap();
    let unviewed = mux.create_session(None).unwrap();
    let other = mux.create_workspace(unviewed, None).unwrap();
    let unviewed_tab = mux.open_tab(other, &idle()).unwrap().tab.id;
    let attachment = mux.attach_session(unviewed).unwrap();
    mux.detach_session(attachment).unwrap();
    mux.attach_session(main).unwrap();
    Fixture {
        mux,
        main,
        editor,
        zoomed: second,
        unviewed,
        unviewed_tab,
        tabless,
    }
}

/// Picks the viewer scope a refused restore's terminals spawn in.
type Scope = Box<dyn Fn(&Mux) -> RuntimeId>;

fn spawn_all(mux: &Mux, capture: &RuntimeCapture) -> SpawnedTerminals {
    SpawnedTerminals::spawn(mux.runtime_id(), capture, |_| idle(), || false)
        .unwrap()
}

/// Applies every queued event, each of which must apply in order.
fn drain(subscription: &HierarchySubscription, state: &mut HierarchyState) {
    loop {
        match subscription.try_recv() {
            Ok(envelope) => {
                let outcome = state.apply(envelope.clone());
                assert!(
                    matches!(outcome, ApplyOutcome::Applied(_)),
                    "{envelope:?} gave {outcome:?}"
                );
            }
            Err(HierarchyRecvError::Empty) => return,
            Err(error) => panic!("unexpected {error}"),
        }
    }
}

/// Each started terminal's shell process, once it is ready.
fn spawned_pids(spawned: &SpawnedTerminals) -> Vec<nix::unistd::Pid> {
    spawned
        .terminals
        .values()
        .map(|started| {
            let client = TestClient::new(&started.runtime);
            wait_for_text(&client, "READY");
            shell_pid(&client)
        })
        .collect()
}

fn assert_reaped(pids: &[nix::unistd::Pid]) {
    for pid in pids {
        assert_eq!(
            nix::sys::signal::kill(*pid, None),
            Err(nix::errno::Errno::ESRCH),
            "a restored child survived"
        );
    }
}

#[test]
fn capture_then_restore_keeps_structure_identities_names_and_layouts() {
    let mut fixture = fixture();
    let capture = fixture.mux.capture_runtime();
    // The tabless session is left out; the unviewed one is kept.
    let sessions: Vec<u64> =
        capture.sessions.iter().map(|s| s.info.id.get()).collect();
    assert_eq!(sessions, [fixture.main.get(), fixture.unviewed.get()]);
    assert!(!sessions.contains(&fixture.tabless.get()));
    assert_eq!(capture.terminals.len(), 6);
    // Fallback names are process labels, which captures never carry.
    let live = fixture
        .mux
        .tab(fixture.editor)
        .unwrap()
        .first_pane()
        .clone();
    assert_eq!(live.fallback_name, "sh");
    assert!(
        capture
            .tabs()
            .flat_map(|tab| tab.layout.panes())
            .all(|pane| pane.fallback_name.is_empty()),
        "{capture:?}"
    );
    assert_eq!(capture.next_id, fixture.mux.next_id);
    assert_eq!((capture.session_ordinal, capture.workspace_ordinal), (3, 4));

    let mut restored = Mux::default();
    let (mut state, subscription) = restored.subscribe_hierarchy();
    let spawned = spawn_all(&restored, &capture);
    let report = restored.restore(&capture, spawned).unwrap();
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(report.removed_tabs, Vec::<TabId>::new());
    assert_eq!(report.empty_sessions, Vec::<SessionId>::new());
    assert_eq!(restored.capture_runtime(), capture);
    assert_eq!(restored.terminal_count(), 6);

    // Saved identities now name records in the restoring runtime's scope.
    let runtime = restored.runtime_id();
    let main = SessionId::in_runtime(runtime, fixture.main.get());
    let unviewed = SessionId::in_runtime(runtime, fixture.unviewed.get());
    assert_eq!(restored.session(main).unwrap().display_name(), "main");
    assert_eq!(
        restored.session(unviewed).unwrap().display_name(),
        "Session 3"
    );
    assert!(!restored.session_attached(unviewed));
    let editor = restored
        .tab(TabId::in_runtime(runtime, fixture.editor.get()))
        .unwrap();
    assert_eq!(editor.custom_name(), Some("editor"));
    assert_eq!(
        editor.zoomed(),
        Some(PaneId::in_runtime(runtime, fixture.zoomed.id.get()))
    );
    assert!(editor.input_sync());
    assert_eq!(editor.layout().pane_count(), 3);
    assert!(
        restored
            .select_pane(PaneId::in_runtime(runtime, fixture.zoomed.id.get()))
            .is_ok()
    );

    // A subscriber that saw the empty runtime follows the restore.
    drain(&subscription, &mut state);
    let (fresh, _) = restored.subscribe_hierarchy();
    assert_eq!(state, fresh);

    // New allocations of every kind land above the watermark, and
    // automatic names continue from the restored ordinals.
    let watermark = capture.next_id;
    let session = restored.create_session(None).unwrap();
    assert_eq!(
        restored.session(session).unwrap().display_name(),
        "Session 4"
    );
    let workspace = restored.create_workspace(session, None).unwrap();
    assert_eq!(
        restored.workspace(workspace).unwrap().display_name(),
        "Workspace 5"
    );
    let tab = restored.open_tab(workspace, &idle()).unwrap().tab;
    let pane = tab.first_pane().clone();
    let added = restored
        .split_pane(workspace, tab.id, pane.id, SplitDirection::Down, &idle())
        .unwrap()
        .pane;
    let split = restored.tab(tab.id).unwrap().layout().split_ids()[0];
    let attachment = restored.attach_session(session).unwrap();
    for id in [
        session.get(),
        workspace.get(),
        tab.id.get(),
        pane.id.get(),
        pane.terminal_id.get(),
        added.id.get(),
        added.terminal_id.get(),
        split.get(),
        attachment.get(),
    ] {
        assert!(id > watermark, "{id} reuses a restored identity");
    }
    restored.shutdown().unwrap();
    fixture.mux.shutdown().unwrap();
}

#[test]
fn capture_lists_local_directories_in_preference_order_without_repeats() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let launched = std::env::current_dir().unwrap();
    let opened = mux
        .open_test_tab(
            workspace,
            &command(
                "printf '\\033]7;file://localhost/reported\\007READY'; read value; printf '\\033]7;file://remote.example/srv\\007REMOTE'; read value",
            ),
        )
        .unwrap();
    let terminal = opened.tab.first_pane().terminal_id;
    let wait_for_directory = |path: &str, local: bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let status = opened.client.status();
            let directory = status.metadata.directory();
            if directory.is_some_and(|directory| {
                directory.path() == path && directory.is_local() == local
            }) {
                return;
            }
            assert!(Instant::now() < deadline, "directory was {directory:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let directories = |mux: &Mux| {
        mux.capture_runtime()
            .terminal(terminal)
            .unwrap()
            .directories
            .clone()
    };

    // The effective and shell directories agree, so the report appears once,
    // ahead of the launch directory.
    wait_for_directory("/reported", true);
    assert_eq!(
        directories(&mux),
        [PathBuf::from("/reported"), launched.clone()]
    );
    // A remote effective directory is never a candidate; the root shell's
    // last local report still is.
    opened
        .client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    wait_for_text(&opened.client, "REMOTE");
    wait_for_directory("/srv", false);
    assert_eq!(
        directories(&mux),
        [PathBuf::from("/reported"), launched.clone()]
    );
    mux.shutdown().unwrap();
}

#[test]
fn a_failed_spawn_leaves_its_pane_and_an_emptied_tab_and_reports_both() {
    let mut fixture = fixture();
    let capture = fixture.mux.capture_runtime();
    let unviewed_terminal = capture.sessions[1].workspaces[0].tabs[0]
        .layout
        .first_pane()
        .terminal_id;
    let lost = [fixture.zoomed.terminal_id, unviewed_terminal];
    let mut restored = Mux::default();
    let spawned = SpawnedTerminals::spawn(
        restored.runtime_id(),
        &capture,
        |terminal| {
            if lost.contains(&terminal.id) {
                failing()
            } else {
                idle()
            }
        },
        || false,
    )
    .unwrap();
    let failed: Vec<TerminalId> =
        spawned.failures().iter().map(|f| f.terminal).collect();
    assert_eq!(failed, lost);
    assert_eq!(spawned.started().count(), 4);

    let report = restored.restore(&capture, spawned).unwrap();
    let runtime = restored.runtime_id();
    let editor = TabId::in_runtime(runtime, fixture.editor.get());
    let unviewed_tab = TabId::in_runtime(runtime, fixture.unviewed_tab.get());
    let unviewed = SessionId::in_runtime(runtime, fixture.unviewed.get());
    let failures: Vec<(TerminalId, TabId)> = report
        .failed
        .iter()
        .map(|failure| (failure.terminal, failure.tab))
        .collect();
    assert_eq!(failures, [(lost[0], editor), (lost[1], unviewed_tab)]);
    assert_eq!(
        report.failed[0].pane,
        PaneId::in_runtime(runtime, fixture.zoomed.id.get())
    );
    assert_ne!(report.failed[0].error.to_string(), "");
    assert_eq!(report.removed_tabs, [unviewed_tab]);
    assert_eq!(report.empty_sessions, [unviewed]);

    // The rest restored: the editor lost its zoomed pane, and with it the
    // zoom, and the emptied session remains without a tab.
    let tab = restored.tab(editor).unwrap();
    assert_eq!(tab.layout().pane_count(), 2);
    assert_eq!(tab.zoomed(), None);
    assert!(restored.tab(unviewed_tab).is_none());
    assert_eq!(restored.session(unviewed).unwrap().workspaces.len(), 1);
    assert_eq!(restored.terminal_count(), 4);
    restored.shutdown().unwrap();
    fixture.mux.shutdown().unwrap();
}

#[test]
fn a_refused_restore_changes_nothing_and_reaps_every_spawned_child() {
    let mut fixture = fixture();
    let capture = fixture.mux.capture_runtime();

    // An invalid capture is refused before anything spawns.
    let mut invalid = capture.clone();
    invalid.next_id = 1;
    let mut asked = 0;
    let refused = SpawnedTerminals::spawn(
        RuntimeId::new(0),
        &invalid,
        |_| {
            asked += 1;
            idle()
        },
        || false,
    );
    assert!(matches!(
        refused,
        Err(MuxError::InvalidCapture(CaptureError::AboveWatermark(_)))
    ));
    assert_eq!(asked, 0);

    let mut restored = Mux::default();
    let seq = restored.hierarchy_seq();
    let refusals: [(&str, RuntimeCapture, Scope); 3] = [
        ("InvalidCapture", invalid, Box::new(Mux::runtime_id)),
        // Spawned in another runtime's viewer scope.
        (
            "ForeignRuntime",
            capture.clone(),
            Box::new(|_| RuntimeId::new(0)),
        ),
        // Spawned for a different structure.
        (
            "RestoreMismatch",
            {
                let mut fewer = capture.clone();
                fewer.sessions.pop();
                fewer.terminals.pop();
                fewer
            },
            Box::new(Mux::runtime_id),
        ),
    ];
    for (expected, restoring, scope) in refusals {
        let spawned = SpawnedTerminals::spawn(
            scope(&restored),
            &capture,
            |_| idle(),
            || false,
        )
        .unwrap();
        let pids = spawned_pids(&spawned);
        let error = restored.restore(&restoring, spawned).unwrap_err();
        assert!(format!("{error:?}").starts_with(expected), "{error:?}");
        assert_reaped(&pids);
        assert_eq!(restored.sessions(), []);
        assert_eq!(restored.terminal_count(), 0);
        assert_eq!(restored.hierarchy_seq(), seq);
    }

    // A runtime that already holds structure refuses too.
    restored.create_session(None).unwrap();
    let before = restored.structure();
    let spawned = spawn_all(&restored, &capture);
    let pids = spawned_pids(&spawned);
    assert!(matches!(
        restored.restore(&capture, spawned),
        Err(MuxError::RestoreNotFresh)
    ));
    assert_reaped(&pids);
    assert_eq!(restored.structure(), before);
    restored.shutdown().unwrap();
    fixture.mux.shutdown().unwrap();
}

#[test]
fn a_restore_needs_a_fresh_runtime_of_the_captured_socket() {
    let mut fixture = fixture();
    let capture = fixture.mux.capture_runtime();

    // Emptied after allocating, so restored identities could repeat ones
    // already handed out.
    let mut used = Mux::default();
    let session = used.create_session(None).unwrap();
    used.close_session(session).unwrap();
    assert_eq!(used.sessions(), []);
    let seq = used.hierarchy_seq();
    let spawned = spawn_all(&used, &capture);
    let pids = spawned_pids(&spawned);
    let error = used.restore(&capture, spawned).unwrap_err();
    assert!(matches!(error, MuxError::RestoreNotFresh), "{error:?}");
    assert_reaped(&pids);
    assert_eq!(used.sessions(), []);
    assert_eq!(used.hierarchy_seq(), seq);

    // A fresh runtime of another socket.
    let mut other = Mux::new("other").unwrap();
    let spawned = spawn_all(&other, &capture);
    let pids = spawned_pids(&spawned);
    let error = other.restore(&capture, spawned).unwrap_err();
    assert!(matches!(error, MuxError::RestoreForeignSocket), "{error:?}");
    assert_reaped(&pids);
    assert_eq!(other.sessions(), []);
    assert_eq!(other.terminal_count(), 0);
    fixture.mux.shutdown().unwrap();
}

#[test]
fn restored_children_stop_on_shutdown_and_a_cancelled_spawn_reaps_its_own() {
    let mut fixture = fixture();
    let capture = fixture.mux.capture_runtime();
    let mut restored = Mux::default();
    let spawned = spawn_all(&restored, &capture);
    let pids = spawned_pids(&spawned);
    assert_eq!(pids.len(), 6);
    restored.restore(&capture, spawned).unwrap();
    restored.shutdown().unwrap();
    assert_reaped(&pids);

    // Cancelling after the first spawn stops it before returning. The
    // child records its PID, which the cancellation waits for.
    let directory = std::env::temp_dir().join(format!(
        "huterm-restore-cancel-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let pid_file = directory.join("pid");
    let script =
        format!("printf '%s' \"$$\" > '{}'; read value", pid_file.display());
    let mut checks = 0;
    let result = SpawnedTerminals::spawn(
        restored.runtime_id(),
        &capture,
        |_| command(&script),
        || {
            checks += 1;
            if checks < 2 {
                return false;
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while std::fs::read_to_string(&pid_file)
                .map_or(true, |pid| pid.is_empty())
            {
                assert!(Instant::now() < deadline, "no PID was written");
                std::thread::sleep(Duration::from_millis(10));
            }
            true
        },
    );
    assert!(matches!(result, Err(MuxError::RestoreCancelled)));
    assert_eq!(checks, 2);
    let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
    assert_reaped(&[nix::unistd::Pid::from_raw(pid)]);
    let _ = std::fs::remove_dir_all(directory);
    fixture.mux.shutdown().unwrap();
}
