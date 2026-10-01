use super::*;
use crate::test_support::TestClient;
use crate::{
    DesktopHostEffectClient, HostEffectRecipientOptions,
    HostEffectViewerOptions,
};
use huterm_protocol::{
    CellSize, GridSize, TerminalInput, TerminalLifecycle, TerminalPresentation,
    ViewerCapabilities,
};
use std::time::{Duration, Instant};

/// Subscribes a clipboard-capable desktop viewer.
fn subscribe_clipboard(
    mux: &Mux,
    attachment: AttachmentId,
    terminal: TerminalId,
    process: &DesktopHostEffectClient,
    allowed: bool,
) -> Result<TerminalViewer, MuxError> {
    mux.subscribe_terminal(
        attachment,
        terminal,
        ViewerOptions {
            host_effects: Some(HostEffectViewerOptions {
                process: process.clone(),
                options: HostEffectRecipientOptions::local_desktop(allowed),
            }),
            ..ViewerOptions::new(ViewerCapabilities::ALL)
        },
    )
}

/// Subscribes a viewer that controls size and presentation from the
/// start: it reports `command`'s geometry and is focused.
fn subscribe_controller(
    mux: &Mux,
    attachment: AttachmentId,
    terminal: TerminalId,
    command: &TerminalCommand,
    presentation: Option<TerminalPresentation>,
) -> Result<TerminalViewer, MuxError> {
    mux.subscribe_terminal(
        attachment,
        terminal,
        ViewerOptions {
            focused: true,
            geometry: Some((command.grid_size, command.cell_size)),
            presentation,
            ..ViewerOptions::new(ViewerCapabilities::ALL)
        },
    )
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "exercise generated names, overrides, and reset on one shared ownership tree"
)]
fn names_reset_to_stable_ordinals_and_current_title_with_duplicate_id_targeting()
 {
    let mut mux = Mux::default();
    assert_eq!(mux.socket_name(), DEFAULT_SOCKET_NAME);
    let first_session = mux.create_session(None).unwrap();
    let second_session = mux.create_session(Some("duplicate")).unwrap();
    let first_workspace = mux.create_workspace(first_session, None).unwrap();
    let second_workspace = mux
        .create_workspace(second_session, Some("duplicate"))
        .unwrap();
    let tab = mux
        .open_test_tab(first_workspace, &command("read value"))
        .unwrap()
        .tab;
    assert_eq!(
        mux.session(first_session).unwrap().display_name(),
        "Session 1"
    );
    assert_eq!(
        mux.workspace(first_workspace).unwrap().display_name(),
        "Workspace 1"
    );
    mux.rename_session(first_session, Some("duplicate"))
        .unwrap();
    mux.rename_workspace(first_workspace, Some("duplicate"))
        .unwrap();
    mux.rename_tab(tab.id, Some("custom")).unwrap();
    assert_eq!(
        mux.tab(tab.id).unwrap().display_name("first title"),
        "custom"
    );
    assert_eq!(
        mux.tab(tab.id).unwrap().display_name("changed title"),
        "custom"
    );
    mux.rename_tab(tab.id, None).unwrap();
    assert_eq!(
        mux.tab(tab.id).unwrap().display_name("changed title"),
        "changed title"
    );
    assert_eq!(mux.tab(tab.id).unwrap().display_name(" \t"), "sh");
    mux.rename_session(first_session, None).unwrap();
    mux.rename_workspace(first_workspace, None).unwrap();
    assert_eq!(
        mux.session(first_session).unwrap().display_name(),
        "Session 1"
    );
    assert_eq!(
        mux.session(second_session).unwrap().display_name(),
        "duplicate"
    );
    assert_eq!(
        mux.workspace(first_workspace).unwrap().display_name(),
        "Workspace 1"
    );
    assert_eq!(
        mux.workspace(second_workspace).unwrap().display_name(),
        "duplicate"
    );
    assert_eq!(
        mux.select_tab(tab.id).unwrap(),
        SelectionTarget {
            session: first_session,
            workspace: Some(first_workspace),
            tab: Some(tab.id)
        }
    );
    assert_eq!(
        mux.sessions().iter().map(|s| s.id).collect::<Vec<_>>(),
        [first_session, second_session]
    );
    let original = mux.sessions().to_vec();
    assert!(matches!(
        mux.create_session(Some(" \t")),
        Err(MuxError::InvalidName)
    ));
    assert!(matches!(
        mux.create_workspace(first_session, Some("")),
        Err(MuxError::InvalidName)
    ));
    assert!(matches!(
        mux.rename_session(first_session, Some("")),
        Err(MuxError::InvalidName)
    ));
    assert!(matches!(
        mux.rename_workspace(first_workspace, Some("")),
        Err(MuxError::InvalidName)
    ));
    assert!(matches!(
        mux.rename_tab(tab.id, Some("")),
        Err(MuxError::InvalidName)
    ));
    assert_eq!(mux.sessions(), original);
    mux.close_session(first_session).unwrap();
    mux.rename_session(second_session, None).unwrap();
    mux.rename_workspace(second_workspace, None).unwrap();
    assert_eq!(
        mux.session(second_session).unwrap().display_name(),
        "Session 2"
    );
    assert_eq!(
        mux.workspace(second_workspace).unwrap().display_name(),
        "Workspace 2"
    );
}

#[test]
fn cross_parent_moves_preserve_live_pty_identity_and_update_selection_ancestry()
{
    let mut mux = Mux::default();
    let first_session = mux.create_session(None).unwrap();
    let second_session = mux.create_session(None).unwrap();
    let first_workspace = mux.create_workspace(first_session, None).unwrap();
    let second_workspace = mux.create_workspace(first_session, None).unwrap();
    let third_workspace = mux.create_workspace(second_session, None).unwrap();
    let opened = mux.open_test_tab(first_workspace, &command("printf READY; read value; printf 'MOVED_%s' \"$value\"; read value")).unwrap();
    let anchor = mux
        .open_test_tab(second_workspace, &command("printf SIBLING; read value"))
        .unwrap();
    wait_for_text(&opened.client, "READY");
    mux.move_tab(
        first_workspace,
        opened.tab.id,
        second_workspace,
        Some(anchor.tab.id),
    )
    .unwrap();
    assert!(mux.workspace(first_workspace).unwrap().tabs.is_empty());
    assert_eq!(
        mux.workspace(second_workspace).unwrap().tabs,
        [opened.tab.clone(), anchor.tab.clone()]
    );
    mux.move_workspace(
        first_session,
        second_workspace,
        second_session,
        Some(third_workspace),
    )
    .unwrap();
    assert_eq!(
        mux.session(first_session).unwrap().workspaces,
        [first_workspace]
    );
    assert_eq!(
        mux.session(second_session).unwrap().workspaces,
        [second_workspace, third_workspace]
    );
    assert_eq!(
        mux.select_tab(opened.tab.id).unwrap(),
        SelectionTarget {
            session: second_session,
            workspace: Some(second_workspace),
            tab: Some(opened.tab.id)
        }
    );
    mux.move_workspace(second_session, second_workspace, second_session, None)
        .unwrap();
    assert_eq!(
        mux.session(second_session).unwrap().workspaces,
        [third_workspace, second_workspace]
    );
    mux.move_workspace(
        second_session,
        second_workspace,
        second_session,
        Some(second_workspace),
    )
    .unwrap();
    assert_eq!(
        mux.session(second_session).unwrap().workspaces,
        [third_workspace, second_workspace]
    );
    mux.move_workspace(
        second_session,
        second_workspace,
        second_session,
        Some(third_workspace),
    )
    .unwrap();
    assert_eq!(
        mux.workspace(second_workspace).unwrap().display_name(),
        "Workspace 2"
    );
    assert_eq!(mux.tab(opened.tab.id).unwrap(), &opened.tab);
    // Moving across sessions revoked the viewer the test opened with.
    assert!(opened.client.poll().revoked);
    let moved = TestClient::new(&mux.terminals[&opened.tab.terminal_id]);
    moved
        .send_input(TerminalInput::Text("alive\n".into()))
        .unwrap();
    wait_for_text(&moved, "MOVED_alive");
    wait_for_text(
        &TestClient::new(&mux.terminals[&anchor.tab.terminal_id]),
        "SIBLING",
    );
    mux.move_workspace(first_session, first_workspace, second_session, None)
        .unwrap();
    assert!(mux.session(first_session).unwrap().workspaces.is_empty());
    mux.close_session(first_session).unwrap();
    assert_eq!(mux.terminal_count(), 2);
    mux.shutdown().unwrap();
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "compare one ownership snapshot after every invalid source, destination, and anchor"
)]
fn stale_and_foreign_structural_targets_reject_atomically() {
    let mut mux = Mux::default();
    let first_session = mux.create_session(None).unwrap();
    let second_session = mux.create_session(None).unwrap();
    let first_workspace = mux.create_workspace(first_session, None).unwrap();
    let second_workspace = mux.create_workspace(second_session, None).unwrap();
    let tab = mux
        .open_test_tab(first_workspace, &command("read value"))
        .unwrap()
        .tab;
    let other_tab = mux
        .open_test_tab(second_workspace, &command("read value"))
        .unwrap()
        .tab;
    let mut foreign = Mux::default();
    let foreign_session = foreign.create_session(None).unwrap();
    foreign.create_session(None).unwrap();
    let foreign_workspace =
        foreign.create_workspace(foreign_session, None).unwrap();
    foreign.create_workspace(foreign_session, None).unwrap();
    let foreign_tab = foreign
        .open_test_tab(foreign_workspace, &command("read value"))
        .unwrap()
        .tab;
    assert_eq!(mux.socket_name(), foreign.socket_name());
    assert_eq!(first_session.get(), foreign_session.get());
    assert_eq!(first_workspace.get(), foreign_workspace.get());
    assert_eq!(tab.id.get(), foreign_tab.id.get());
    let stale_session = SessionId::in_runtime(mux.runtime_id(), u64::MAX);
    let stale_workspace = WorkspaceId::in_runtime(mux.runtime_id(), u64::MAX);
    let stale_tab = TabId::in_runtime(mux.runtime_id(), u64::MAX);
    let original_sessions = mux.sessions().to_vec();
    let original_workspaces = mux.workspaces.clone();
    for (source, target, destination, anchor) in [
        (first_workspace, tab.id, second_workspace, Some(stale_tab)),
        (first_workspace, tab.id, second_workspace, Some(tab.id)),
        (second_workspace, tab.id, first_workspace, None),
        (first_workspace, stale_tab, second_workspace, None),
        (stale_workspace, tab.id, second_workspace, None),
        (first_workspace, tab.id, stale_workspace, None),
        (foreign_workspace, tab.id, second_workspace, None),
        (first_workspace, foreign_tab.id, second_workspace, None),
        (first_workspace, tab.id, foreign_workspace, None),
        (
            first_workspace,
            tab.id,
            second_workspace,
            Some(foreign_tab.id),
        ),
    ] {
        assert!(mux.move_tab(source, target, destination, anchor).is_err());
        assert_eq!(mux.workspaces, original_workspaces);
    }
    for (source, target, destination, anchor) in [
        (
            first_session,
            first_workspace,
            second_session,
            Some(stale_workspace),
        ),
        (
            first_session,
            first_workspace,
            second_session,
            Some(first_workspace),
        ),
        (second_session, first_workspace, first_session, None),
        (first_session, stale_workspace, second_session, None),
        (stale_session, first_workspace, second_session, None),
        (first_session, first_workspace, stale_session, None),
        (foreign_session, first_workspace, second_session, None),
        (first_session, foreign_workspace, second_session, None),
        (first_session, first_workspace, foreign_session, None),
        (
            first_session,
            first_workspace,
            second_session,
            Some(foreign_workspace),
        ),
    ] {
        assert!(
            mux.move_workspace(source, target, destination, anchor)
                .is_err()
        );
        assert_eq!(mux.sessions(), original_sessions);
        assert_eq!(mux.workspaces, original_workspaces);
    }
    for invalid in [
        foreign_session,
        stale_session,
        SessionId::new(first_session.get()),
    ] {
        assert!(mux.create_workspace(invalid, None).is_err());
        assert!(mux.rename_session(invalid, Some("wrong")).is_err());
        assert!(mux.close_session(invalid).is_err());
        assert!(mux.select_session(invalid).is_err());
    }
    for invalid in [
        foreign_workspace,
        stale_workspace,
        WorkspaceId::new(first_workspace.get()),
    ] {
        assert!(mux.open_test_tab(invalid, &command("read value")).is_err());
        assert!(mux.rename_workspace(invalid, Some("wrong")).is_err());
        assert!(mux.close_workspace(invalid).is_err());
        assert!(mux.select_workspace(invalid).is_err());
    }
    for invalid in [foreign_tab.id, stale_tab, TabId::new(tab.id.get())] {
        assert!(mux.rename_tab(invalid, Some("wrong")).is_err());
        assert!(mux.close_tab(first_workspace, invalid).is_err());
        assert!(mux.select_tab(invalid).is_err());
    }
    assert!(mux.close_tab(first_workspace, other_tab.id).is_err());
    assert_eq!(mux.sessions(), original_sessions);
    assert_eq!(mux.workspaces, original_workspaces);
    assert_eq!(mux.terminal_count(), 2);
    mux.shutdown().unwrap();
    foreign.shutdown().unwrap();
}

#[test]
fn session_close_stops_its_workers_and_preserves_sibling_input() {
    let mut mux = Mux::default();
    let first_session = mux.create_session(None).unwrap();
    let second_session = mux.create_session(None).unwrap();
    let first_workspace = mux.create_workspace(first_session, None).unwrap();
    let second_workspace = mux.create_workspace(first_session, None).unwrap();
    let third_workspace = mux.create_workspace(second_session, None).unwrap();
    let first = mux
        .open_test_tab(first_workspace, &command("printf READY; read value"))
        .unwrap();
    let second = mux
        .open_test_tab(second_workspace, &command("printf READY; read value"))
        .unwrap();
    let sibling = mux.open_test_tab(third_workspace, &command("printf READY; read value; printf 'SIBLING_%s' \"$value\"; read value")).unwrap();
    wait_for_text(&first.client, "READY");
    wait_for_text(&second.client, "READY");
    wait_for_text(&sibling.client, "READY");
    mux.close_session(first_session).unwrap();
    for client in [&first.client, &second.client] {
        assert!(matches!(client.read_snapshot(), Err(RuntimeError::Stopped)));
    }
    assert!(mux.session(first_session).is_none());
    assert!(mux.workspace(first_workspace).is_none());
    assert!(mux.workspace(second_workspace).is_none());
    assert_eq!(mux.sessions().len(), 1);
    assert_eq!(mux.terminal_count(), 1);
    sibling
        .client
        .send_input(TerminalInput::Text("alive\n".into()))
        .unwrap();
    wait_for_text(&sibling.client, "SIBLING_alive");
    mux.shutdown().unwrap();
}

#[test]
fn exhausted_creation_does_not_publish_partial_structure() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    mux.reserve_through(u64::MAX - 2);
    assert!(matches!(
        mux.open_test_tab(workspace, &command("read value")),
        Err(MuxError::IdExhausted)
    ));
    assert!(mux.workspace(workspace).unwrap().tabs.is_empty());
    assert_eq!(mux.terminal_count(), 0);
    assert!(matches!(
        mux.create_workspace(session, None),
        Err(MuxError::IdExhausted)
    ));
    assert!(matches!(
        mux.create_session(None),
        Err(MuxError::IdExhausted)
    ));
    assert_eq!(mux.sessions().len(), 1);
    assert_eq!(mux.session(session).unwrap().workspaces, [workspace]);
    mux.session_ordinal = u64::MAX;
    mux.workspace_ordinal = u64::MAX;
    mux.next_id = 5;
    assert!(matches!(
        mux.create_session(None),
        Err(MuxError::IdExhausted)
    ));
    assert!(matches!(
        mux.create_workspace(session, None),
        Err(MuxError::IdExhausted)
    ));
    assert_eq!(mux.next_id, 5);
}
#[test]
fn reorder_preserves_live_terminals_and_rejects_stale_or_foreign_anchors() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let sibling = mux.create_workspace(session, None).unwrap();
    let first = mux.open_test_tab(workspace, &command("printf FIRST; read value; printf 'GOT_%s' \"$value\"; read value")).unwrap();
    let second = mux
        .open_test_tab(workspace, &command("printf SECOND; read value"))
        .unwrap();
    let third = mux
        .open_test_tab(workspace, &command("printf THIRD; read value"))
        .unwrap();
    let foreign = mux
        .open_test_tab(sibling, &command("printf FOREIGN; read value"))
        .unwrap();
    wait_for_text(&first.client, "FIRST");
    let original = mux.workspace(workspace).unwrap().tabs.clone();
    mux.reorder_tab(workspace, third.tab.id, Some(first.tab.id))
        .unwrap();
    assert_eq!(
        mux.workspace(workspace).unwrap().tabs,
        [third.tab.clone(), first.tab.clone(), second.tab.clone()]
    );
    mux.reorder_tab(workspace, third.tab.id, None).unwrap();
    assert_eq!(mux.workspace(workspace).unwrap().tabs, original);
    for anchor in [Some(first.tab.id), Some(second.tab.id)] {
        mux.reorder_tab(workspace, first.tab.id, anchor).unwrap();
        assert_eq!(mux.workspace(workspace).unwrap().tabs, original);
    }
    for (tab, anchor) in [
        (
            first.tab.id,
            Some(TabId::in_runtime(mux.runtime_id(), u64::MAX)),
        ),
        (first.tab.id, Some(foreign.tab.id)),
        (foreign.tab.id, None),
    ] {
        assert!(matches!(
            mux.reorder_tab(workspace, tab, anchor),
            Err(MuxError::UnknownTab(_))
        ));
        assert_eq!(mux.workspace(workspace).unwrap().tabs, original);
    }
    assert!(matches!(
        mux.reorder_tab(
            WorkspaceId::in_runtime(mux.runtime_id(), u64::MAX),
            first.tab.id,
            None
        ),
        Err(MuxError::UnknownWorkspace(_))
    ));
    first
        .client
        .send_input(TerminalInput::Text("alive\n".into()))
        .unwrap();
    wait_for_text(&first.client, "GOT_alive");
    wait_for_text(&second.client, "SECOND");
    assert_eq!(mux.terminal_count(), 4);
    assert_eq!(mux.workspace(sibling).unwrap().tabs, [foreign.tab]);
    mux.shutdown().unwrap();
}

pub(super) fn command(script: &str) -> TerminalCommand {
    TerminalCommand {
        program: "/bin/sh".into(),
        arguments: vec!["-c".into(), script.into()],
        working_directory: std::env::current_dir().unwrap(),
        environment: Vec::new(),
        grid_size: GridSize::clamped(40, 8),
        cell_size: CellSize {
            width: 8,
            height: 16,
        },
        presentation: huterm_protocol::TerminalPresentation::default(),
    }
}
fn wait_for_text(client: &crate::test_support::TestClient, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let snapshot = client.read_snapshot().unwrap();
        let text: String =
            snapshot.cells().map(|cell| cell.text.as_str()).collect();
        if text.contains(needle) {
            return;
        }
        assert!(Instant::now() < deadline, "missing {needle}: {text}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn allocation_preserves_reserved_identity_and_rejects_exhaustion() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    mux.reserve_through(90);
    assert_eq!(
        mux.create_workspace(session, None).unwrap(),
        WorkspaceId::in_runtime(mux.runtime_id(), 91)
    );
    mux.reserve_through(1);
    assert_eq!(
        mux.create_workspace(session, None).unwrap(),
        WorkspaceId::in_runtime(mux.runtime_id(), 92)
    );
    mux.reserve_through(u64::MAX);
    assert!(matches!(
        mux.create_workspace(session, None),
        Err(MuxError::IdExhausted)
    ));
}

#[test]
fn failed_spawn_does_not_publish_a_tab_or_replace_siblings() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let first = mux
        .open_test_tab(workspace, &command("printf READY; read value"))
        .unwrap();
    let mut invalid = command("");
    invalid.program = "/huterm-nonexistent-shell".into();
    assert!(mux.open_test_tab(workspace, &invalid).is_err());
    assert_eq!(mux.workspace(workspace).unwrap().tabs, vec![first.tab]);
    assert_eq!(mux.terminal_count(), 1);
    wait_for_text(&first.client, "READY");
    mux.shutdown().unwrap();
}

#[test]
fn closing_tabs_preserves_order_and_other_workspace_processes() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let first_workspace = mux.create_workspace(session, None).unwrap();
    let second_workspace = mux.create_workspace(session, None).unwrap();
    let script =
        "printf READY; read value; printf 'GOT:%s' \"$value\"; read value";
    let first = mux
        .open_test_tab(first_workspace, &command(script))
        .unwrap();
    let second = mux
        .open_test_tab(first_workspace, &command(script))
        .unwrap();
    let third = mux
        .open_test_tab(second_workspace, &command(script))
        .unwrap();
    assert_ne!(first.tab.terminal_id, third.tab.terminal_id);
    mux.close_tab(first_workspace, first.tab.id).unwrap();
    assert!(matches!(
        first.client.read_snapshot(),
        Err(RuntimeError::Stopped)
    ));
    assert_eq!(
        mux.workspace(first_workspace).unwrap().tabs,
        vec![second.tab]
    );
    mux.close_workspace(first_workspace).unwrap();
    assert!(matches!(
        second.client.read_snapshot(),
        Err(RuntimeError::Stopped)
    ));
    wait_for_text(&third.client, "READY");
    third
        .client
        .send_input(TerminalInput::Text("alive\n".into()))
        .unwrap();
    wait_for_text(&third.client, "GOT:alive");
    assert!(mux.workspace(first_workspace).is_none());
    assert_eq!(mux.terminal_count(), 1);
    assert!(matches!(
        mux.close_tab(second_workspace, first.tab.id),
        Err(MuxError::UnknownTab(_))
    ));
    mux.shutdown().unwrap();
}

#[cfg(unix)]
#[test]
fn deleting_a_session_reaps_its_child_process() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let opened = mux
        .open_test_tab(
            workspace,
            &command("printf 'PID:%s:READY' \"$$\"; read value"),
        )
        .unwrap();
    wait_for_text(&opened.client, "READY");
    let snapshot = opened.client.read_snapshot().unwrap();
    let text: String =
        snapshot.cells().map(|cell| cell.text.as_str()).collect();
    let pid = text
        .split("PID:")
        .nth(1)
        .unwrap()
        .split(':')
        .next()
        .unwrap()
        .parse::<i32>()
        .unwrap();
    mux.close_session(session).unwrap();
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    );
}

#[test]
fn viewer_authority_tracks_attachment_and_terminal_membership() {
    use crate::host_effects::HostEffectAdmission;

    let mut mux = Mux::default();
    let first_session = mux.create_session(None).unwrap();
    let second_session = mux.create_session(None).unwrap();
    let first_workspace = mux.create_workspace(first_session, None).unwrap();
    let same_session_workspace =
        mux.create_workspace(first_session, None).unwrap();
    let second_workspace = mux.create_workspace(second_session, None).unwrap();
    let opened = mux
        .open_test_tab(first_workspace, &command("read value"))
        .unwrap();
    let terminal_id = opened.tab.terminal_id;
    let first_attachment = mux.attach_session(first_session).unwrap();
    let second_attachment = mux.attach_session(second_session).unwrap();
    let process = DesktopHostEffectClient::new();
    let subscribe = |mux: &Mux, attachment| {
        subscribe_clipboard(mux, attachment, terminal_id, &process, true)
    };

    assert!(matches!(
        subscribe(&mux, second_attachment),
        Err(MuxError::TerminalNotInAttachment { .. })
    ));
    let first = subscribe(&mux, first_attachment).unwrap();
    let first_recipient = first.host_effects().unwrap();
    let sink = opened.client.control.host_effect_sink();
    assert_eq!(
        sink.admit_borrowed("same session"),
        HostEffectAdmission::Accepted
    );
    let same_session = first_recipient.try_next().unwrap();
    mux.move_tab(first_workspace, opened.tab.id, same_session_workspace, None)
        .unwrap();
    assert!(first_recipient.is_current(&same_session));
    assert!(first.read_snapshot().is_ok());

    assert_eq!(
        sink.admit_borrowed("cross session"),
        HostEffectAdmission::Accepted
    );
    mux.move_tab(
        same_session_workspace,
        opened.tab.id,
        second_workspace,
        None,
    )
    .unwrap();
    assert!(first_recipient.try_next().is_none());
    assert!(!first_recipient.is_current(&same_session));
    assert!(first.poll().revoked);
    assert!(matches!(
        first.send_input(TerminalInput::Text("x".into())),
        Err(RuntimeError::Revoked)
    ));
    assert!(matches!(first.read_snapshot(), Err(RuntimeError::Revoked)));
    assert!(
        opened.client.read_snapshot().is_err(),
        "the test viewer has no attachment but moved with all others"
    );

    let second = subscribe(&mux, second_attachment).unwrap();
    assert_eq!(
        sink.admit_borrowed("workspace move"),
        HostEffectAdmission::Accepted
    );
    let workspace_move = second.host_effects().unwrap().try_next().unwrap();
    mux.move_workspace(second_session, second_workspace, first_session, None)
        .unwrap();
    assert!(!second.host_effects().unwrap().is_current(&workspace_move));
    assert!(second.poll().revoked);

    mux.retarget_attachment(second_attachment, first_session)
        .unwrap();
    let retargeted = subscribe(&mux, second_attachment).unwrap();
    assert_eq!(
        sink.admit_borrowed("same target"),
        HostEffectAdmission::Accepted
    );
    let same_target = retargeted.host_effects().unwrap().try_next().unwrap();
    mux.retarget_attachment(second_attachment, first_session)
        .unwrap();
    assert!(retargeted.host_effects().unwrap().is_current(&same_target));
    assert!(retargeted.read_snapshot().is_ok());

    mux.detach_session(second_attachment).unwrap();
    assert!(!retargeted.host_effects().unwrap().is_current(&same_target));
    assert!(matches!(
        retargeted.read_snapshot(),
        Err(RuntimeError::Revoked)
    ));
    let final_attachment = mux.attach_session(first_session).unwrap();
    let last = subscribe(&mux, final_attachment).unwrap();
    assert_eq!(sink.admit_borrowed("close"), HostEffectAdmission::Accepted);
    let closing = last.host_effects().unwrap().try_next().unwrap();
    mux.close_session(first_session).unwrap();
    assert!(!last.host_effects().unwrap().is_current(&closing));
    assert!(matches!(
        last.read_snapshot(),
        Err(RuntimeError::Revoked | RuntimeError::Stopped)
    ));
}

#[test]
fn presentation_seed_and_controlling_viewer_follow_attachment_authority() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let mut command = command("read value");
    command.presentation.foreground = huterm_protocol::Rgb {
        red: 1,
        green: 2,
        blue: 3,
    };
    let opened = mux.open_test_tab(workspace, &command).unwrap();
    assert_eq!(
        opened.client.control.presentation(),
        Some((
            command.presentation.clone(),
            command.grid_size,
            command.cell_size,
        ))
    );

    let attachment = mux.attach_session(session).unwrap();
    let process = DesktopHostEffectClient::new();
    let clipboard = subscribe_clipboard(
        &mux,
        attachment,
        opened.tab.terminal_id,
        &process,
        false,
    )
    .unwrap();
    let controller = subscribe_controller(
        &mux,
        attachment,
        opened.tab.terminal_id,
        &command,
        None,
    )
    .unwrap();
    let generation = controller.read_snapshot().unwrap().generation;
    let mut changed = command.presentation.clone();
    changed.background = huterm_protocol::Rgb {
        red: 4,
        green: 5,
        blue: 6,
    };
    controller.update_presentation(changed.clone()).unwrap();
    wait_for_presentation(&opened.client, &changed);
    // Presentation invalidates snapshots without advancing the content
    // generation.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(invalidated) = controller.poll().invalidated {
            assert_eq!(invalidated, generation);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "presentation update did not invalidate snapshots"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        opened
            .client
            .control
            .host_effect_sink()
            .admit_borrowed("denied"),
        crate::host_effects::HostEffectAdmission::Denied
    );
    assert!(clipboard.host_effects().unwrap().try_next().is_none());

    mux.detach_session(attachment).unwrap();
    assert!(matches!(
        controller.update_presentation(command.presentation.clone()),
        Err(RuntimeError::Revoked)
    ));
    assert_eq!(opened.client.control.presentation().unwrap().0, changed);
    mux.close_session(session).unwrap();
}

#[test]
fn presentation_follows_the_controlling_viewer_without_failing_others() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let command = command("read value");
    let opened = mux.open_test_tab(workspace, &command).unwrap();
    let attachment = mux.attach_session(session).unwrap();
    let theme = |red| huterm_protocol::TerminalPresentation {
        cursor: huterm_protocol::Rgb {
            red,
            green: 8,
            blue: 9,
        },
        ..Default::default()
    };
    let first = subscribe_controller(
        &mux,
        attachment,
        opened.tab.terminal_id,
        &command,
        Some(theme(1)),
    )
    .unwrap();
    wait_for_presentation(&opened.client, &theme(1));
    // Focus moves to the second viewer's window.
    first.set_focus(false).unwrap();
    let second = subscribe_controller(
        &mux,
        attachment,
        opened.tab.terminal_id,
        &command,
        Some(theme(2)),
    )
    .unwrap();
    wait_for_presentation(&opened.client, &theme(2));
    // A viewer that does not control still submits without failing.
    first.update_presentation(theme(3)).unwrap();
    second.read_snapshot().unwrap();
    assert_eq!(opened.client.control.presentation().unwrap().0, theme(2));
    // Focus hands control, and the presentation it submitted, back.
    second.set_focus(false).unwrap();
    first.set_focus(true).unwrap();
    wait_for_presentation(&opened.client, &theme(3));
    mux.close_session(session).unwrap();
    assert!(matches!(
        second.update_presentation(theme(4)),
        Err(RuntimeError::Revoked | RuntimeError::Stopped)
    ));
}

#[test]
fn subscription_validates_membership_and_structural_changes() {
    let mut mux = Mux::default();
    let source = mux.create_session(None).unwrap();
    let destination = mux.create_session(None).unwrap();
    let source_workspace = mux.create_workspace(source, None).unwrap();
    let destination_workspace =
        mux.create_workspace(destination, None).unwrap();
    let opened = mux
        .open_test_tab(source_workspace, &command("read value"))
        .unwrap();
    let source_attachment = mux.attach_session(source).unwrap();
    let destination_attachment = mux.attach_session(destination).unwrap();
    let subscribe = |mux: &Mux, attachment| {
        mux.subscribe_terminal(
            attachment,
            opened.tab.terminal_id,
            ViewerOptions::new(ViewerCapabilities::ALL),
        )
    };
    assert!(matches!(
        subscribe(&mux, destination_attachment),
        Err(MuxError::TerminalNotInAttachment { .. })
    ));
    let mut foreign_mux = Mux::default();
    let foreign_session = foreign_mux.create_session(None).unwrap();
    let foreign_attachment =
        foreign_mux.attach_session(foreign_session).unwrap();
    assert!(matches!(
        subscribe(&mux, foreign_attachment),
        Err(MuxError::ForeignRuntime(_))
    ));

    let retargeted = subscribe(&mux, source_attachment).unwrap();
    mux.retarget_attachment(source_attachment, destination)
        .unwrap();
    assert!(matches!(
        retargeted.update_presentation(TerminalPresentation::default()),
        Err(RuntimeError::Revoked)
    ));

    let replacement_attachment = mux.attach_session(source).unwrap();
    let moved = subscribe(&mux, replacement_attachment).unwrap();
    mux.move_tab(source_workspace, opened.tab.id, destination_workspace, None)
        .unwrap();
    assert!(matches!(
        moved.update_presentation(TerminalPresentation::default()),
        Err(RuntimeError::Revoked)
    ));

    let closing = subscribe(&mux, destination_attachment).unwrap();
    mux.close_tab(destination_workspace, opened.tab.id).unwrap();
    assert!(matches!(
        closing.read_snapshot(),
        Err(RuntimeError::Stopped)
    ));
    mux.shutdown().unwrap();
}

fn wait_for_presentation(
    client: &crate::test_support::TestClient,
    expected: &huterm_protocol::TerminalPresentation,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if client
            .control
            .presentation()
            .is_some_and(|(presentation, _, _)| presentation == *expected)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "terminal presentation did not update"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn detaching_preserves_terminal_and_unobserved_output_and_exit() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let opened = mux
        .open_test_tab(
            workspace,
            &command(
                "read value; printf '\x1b]0;finished\x07BACKGROUND'; exit 7",
            ),
        )
        .unwrap();
    let terminal_id = opened.tab.terminal_id;
    opened
        .client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    drop(opened.client);
    let deadline = Instant::now() + Duration::from_secs(3);
    // No viewer and no snapshot requests while the terminal is hidden;
    // a later viewer reads the title and exit from the status.
    let status = loop {
        let status = mux.terminals[&terminal_id].registry().status();
        if status.title == "finished"
            && status.lifecycle != TerminalLifecycle::Running
        {
            break status;
        }
        assert!(Instant::now() < deadline, "missing title or exit");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(matches!(
        status.lifecycle,
        TerminalLifecycle::Exited(exit) if exit.code == Some(7)
    ));
    assert_eq!(mux.workspace(workspace).unwrap().tabs.len(), 1);
    let late = mux.terminals[&terminal_id]
        .subscribe(ViewerOptions::new(ViewerCapabilities::ALL))
        .unwrap();
    let update = late.poll();
    assert!(update.exited.is_none(), "a late viewer reports no exit");
    assert!(
        update
            .status
            .is_some_and(|status| status.title == "finished")
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while !late
        .read_snapshot()
        .unwrap()
        .cells()
        .map(|cell| cell.text.as_str())
        .collect::<String>()
        .contains("BACKGROUND")
    {
        assert!(Instant::now() < deadline, "missing BACKGROUND");
        std::thread::sleep(Duration::from_millis(10));
    }
    mux.shutdown().unwrap();
}
