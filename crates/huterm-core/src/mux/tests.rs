use super::*;
use huterm_protocol::{
    CellSize, GridSize, TerminalEvent, TerminalInput, TerminalPresentation,
};
use std::time::{Duration, Instant};

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
        .open_tab(first_workspace, &command("read value"))
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
    let opened = mux.open_tab(first_workspace, &command("printf READY; read value; printf 'MOVED_%s' \"$value\"; read value")).unwrap();
    let anchor = mux
        .open_tab(second_workspace, &command("printf SIBLING; read value"))
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
    opened
        .client
        .send_input(TerminalInput::Text("alive\n".into()))
        .unwrap();
    wait_for_text(&mux.attach(opened.tab.terminal_id).unwrap(), "MOVED_alive");
    wait_for_text(&anchor.client, "SIBLING");
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
        .open_tab(first_workspace, &command("read value"))
        .unwrap()
        .tab;
    let other_tab = mux
        .open_tab(second_workspace, &command("read value"))
        .unwrap()
        .tab;
    let mut foreign = Mux::default();
    let foreign_session = foreign.create_session(None).unwrap();
    foreign.create_session(None).unwrap();
    let foreign_workspace =
        foreign.create_workspace(foreign_session, None).unwrap();
    foreign.create_workspace(foreign_session, None).unwrap();
    let foreign_tab = foreign
        .open_tab(foreign_workspace, &command("read value"))
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
        assert!(mux.open_tab(invalid, &command("read value")).is_err());
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
        .open_tab(first_workspace, &command("printf READY; read value"))
        .unwrap();
    let second = mux
        .open_tab(second_workspace, &command("printf READY; read value"))
        .unwrap();
    let sibling = mux.open_tab(third_workspace, &command("printf READY; read value; printf 'SIBLING_%s' \"$value\"; read value")).unwrap();
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
        mux.open_tab(workspace, &command("read value")),
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
    let first = mux.open_tab(workspace, &command("printf FIRST; read value; printf 'GOT_%s' \"$value\"; read value")).unwrap();
    let second = mux
        .open_tab(workspace, &command("printf SECOND; read value"))
        .unwrap();
    let third = mux
        .open_tab(workspace, &command("printf THIRD; read value"))
        .unwrap();
    let foreign = mux
        .open_tab(sibling, &command("printf FOREIGN; read value"))
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
fn wait_for_text(client: &RuntimeClient, needle: &str) {
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
        .open_tab(workspace, &command("printf READY; read value"))
        .unwrap();
    let mut invalid = command("");
    invalid.program = "/huterm-nonexistent-shell".into();
    assert!(mux.open_tab(workspace, &invalid).is_err());
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
    let first = mux.open_tab(first_workspace, &command(script)).unwrap();
    let second = mux.open_tab(first_workspace, &command(script)).unwrap();
    let third = mux.open_tab(second_workspace, &command(script)).unwrap();
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
        .open_tab(
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
#[expect(
    clippy::too_many_lines,
    reason = "exercise one recipient across all membership-preserving and invalidating transitions"
)]
fn host_effect_authority_tracks_attachment_and_terminal_membership() {
    use crate::host_effects::HostEffectAdmission;

    let mut mux = Mux::default();
    let first_session = mux.create_session(None).unwrap();
    let second_session = mux.create_session(None).unwrap();
    let first_workspace = mux.create_workspace(first_session, None).unwrap();
    let same_session_workspace =
        mux.create_workspace(first_session, None).unwrap();
    let second_workspace = mux.create_workspace(second_session, None).unwrap();
    let opened = mux
        .open_tab(first_workspace, &command("read value"))
        .unwrap();
    let terminal_id = opened.tab.terminal_id;
    let first_attachment = mux.attach_session(first_session).unwrap();
    let second_attachment = mux.attach_session(second_session).unwrap();
    let process = DesktopHostEffectClient::new();

    assert!(matches!(
        mux.register_host_effect_recipient(
            second_attachment,
            terminal_id,
            &process,
            HostEffectRecipientOptions::local_desktop(true),
        ),
        Err(MuxError::TerminalNotInAttachment { .. })
    ));
    let first_recipient = mux
        .register_host_effect_recipient(
            first_attachment,
            terminal_id,
            &process,
            HostEffectRecipientOptions::local_desktop(true),
        )
        .unwrap();
    let sink = opened.client.host_effect_sink();
    assert_eq!(
        sink.admit_borrowed("same session"),
        HostEffectAdmission::Accepted
    );
    let same_session = first_recipient.try_next().unwrap();
    mux.move_tab(first_workspace, opened.tab.id, same_session_workspace, None)
        .unwrap();
    assert!(first_recipient.is_current(&same_session));

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

    let second_recipient = mux
        .register_host_effect_recipient(
            second_attachment,
            terminal_id,
            &process,
            HostEffectRecipientOptions::local_desktop(true),
        )
        .unwrap();
    assert_eq!(
        sink.admit_borrowed("workspace move"),
        HostEffectAdmission::Accepted
    );
    let workspace_move = second_recipient.try_next().unwrap();
    mux.move_workspace(second_session, second_workspace, first_session, None)
        .unwrap();
    assert!(!second_recipient.is_current(&workspace_move));

    mux.retarget_attachment(second_attachment, first_session)
        .unwrap();
    let retargeted = mux
        .register_host_effect_recipient(
            second_attachment,
            terminal_id,
            &process,
            HostEffectRecipientOptions::local_desktop(true),
        )
        .unwrap();
    assert_eq!(
        sink.admit_borrowed("same target"),
        HostEffectAdmission::Accepted
    );
    let same_target = retargeted.try_next().unwrap();
    mux.retarget_attachment(second_attachment, first_session)
        .unwrap();
    assert!(retargeted.is_current(&same_target));

    mux.detach_session(second_attachment).unwrap();
    assert!(!retargeted.is_current(&same_target));
    let final_attachment = mux.attach_session(first_session).unwrap();
    let final_recipient = mux
        .register_host_effect_recipient(
            final_attachment,
            terminal_id,
            &process,
            HostEffectRecipientOptions::local_desktop(true),
        )
        .unwrap();
    assert_eq!(sink.admit_borrowed("close"), HostEffectAdmission::Accepted);
    let closing = final_recipient.try_next().unwrap();
    mux.close_session(first_session).unwrap();
    assert!(!final_recipient.is_current(&closing));
}

#[test]
fn presentation_seed_and_controller_follow_attachment_authority() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let mut command = command("read value");
    command.presentation.foreground = huterm_protocol::Rgb {
        red: 1,
        green: 2,
        blue: 3,
    };
    let opened = mux.open_tab(workspace, &command).unwrap();
    assert_eq!(
        opened.client.presentation(),
        Some((
            command.presentation.clone(),
            command.grid_size,
            command.cell_size,
        ))
    );

    let attachment = mux.attach_session(session).unwrap();
    let process = DesktopHostEffectClient::new();
    let clipboard = mux
        .register_host_effect_recipient(
            attachment,
            opened.tab.terminal_id,
            &process,
            HostEffectRecipientOptions::local_desktop(false),
        )
        .unwrap();
    let controller = mux
        .register_presentation_controller(attachment, opened.tab.terminal_id)
        .unwrap();
    let generation = opened.client.read_snapshot().unwrap().generation;
    while opened.client.try_recv_event().unwrap().is_some() {}
    let mut changed = command.presentation.clone();
    changed.background = huterm_protocol::Rgb {
        red: 4,
        green: 5,
        blue: 6,
    };
    controller.update(changed.clone()).unwrap();
    wait_for_presentation(&opened.client, &changed);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(TerminalEvent::Invalidated {
            generation: invalidated,
            ..
        }) = opened.client.try_recv_event().unwrap()
        {
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
        opened.client.host_effect_sink().admit_borrowed("denied"),
        crate::host_effects::HostEffectAdmission::Denied
    );
    assert!(clipboard.try_next().is_none());

    mux.detach_session(attachment).unwrap();
    assert!(matches!(
        controller.update(command.presentation.clone()),
        Err(RuntimeError::Stopped)
    ));
    assert_eq!(opened.client.presentation().unwrap().0, changed);
    mux.close_session(session).unwrap();
}

#[test]
fn replacement_presentation_controller_rejects_superseded_updates() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let opened = mux.open_tab(workspace, &command("read value")).unwrap();
    let attachment = mux.attach_session(session).unwrap();
    let stale = mux
        .register_presentation_controller(attachment, opened.tab.terminal_id)
        .unwrap();
    let current = mux
        .register_presentation_controller(attachment, opened.tab.terminal_id)
        .unwrap();
    let changed = huterm_protocol::TerminalPresentation {
        cursor: huterm_protocol::Rgb {
            red: 7,
            green: 8,
            blue: 9,
        },
        ..Default::default()
    };
    assert!(matches!(
        stale.update(changed.clone()),
        Err(RuntimeError::Stopped)
    ));
    current.update(changed.clone()).unwrap();
    wait_for_presentation(&opened.client, &changed);
    mux.close_session(session).unwrap();
    assert!(matches!(
        current.update(changed),
        Err(RuntimeError::Stopped)
    ));
}

#[test]
fn presentation_controller_validates_membership_and_structural_changes() {
    let mut mux = Mux::default();
    let source = mux.create_session(None).unwrap();
    let destination = mux.create_session(None).unwrap();
    let source_workspace = mux.create_workspace(source, None).unwrap();
    let destination_workspace =
        mux.create_workspace(destination, None).unwrap();
    let opened = mux
        .open_tab(source_workspace, &command("read value"))
        .unwrap();
    let source_attachment = mux.attach_session(source).unwrap();
    let destination_attachment = mux.attach_session(destination).unwrap();
    assert!(matches!(
        mux.register_presentation_controller(
            destination_attachment,
            opened.tab.terminal_id
        ),
        Err(MuxError::TerminalNotInAttachment { .. })
    ));
    let mut foreign_mux = Mux::default();
    let foreign_session = foreign_mux.create_session(None).unwrap();
    let foreign_attachment =
        foreign_mux.attach_session(foreign_session).unwrap();
    assert!(matches!(
        mux.register_presentation_controller(
            foreign_attachment,
            opened.tab.terminal_id
        ),
        Err(MuxError::ForeignRuntime(_))
    ));

    let retargeted = mux
        .register_presentation_controller(
            source_attachment,
            opened.tab.terminal_id,
        )
        .unwrap();
    mux.retarget_attachment(source_attachment, destination)
        .unwrap();
    assert!(matches!(
        retargeted.update(TerminalPresentation::default()),
        Err(RuntimeError::Stopped)
    ));

    let replacement_attachment = mux.attach_session(source).unwrap();
    let moved = mux
        .register_presentation_controller(
            replacement_attachment,
            opened.tab.terminal_id,
        )
        .unwrap();
    mux.move_tab(source_workspace, opened.tab.id, destination_workspace, None)
        .unwrap();
    assert!(matches!(
        moved.update(TerminalPresentation::default()),
        Err(RuntimeError::Stopped)
    ));

    let closing = mux
        .register_presentation_controller(
            destination_attachment,
            opened.tab.terminal_id,
        )
        .unwrap();
    mux.close_tab(destination_workspace, opened.tab.id).unwrap();
    assert!(matches!(
        closing.update(TerminalPresentation::default()),
        Err(RuntimeError::Stopped)
    ));
    mux.shutdown().unwrap();
}

fn wait_for_presentation(
    client: &RuntimeClient,
    expected: &huterm_protocol::TerminalPresentation,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if client
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
        .open_tab(
            workspace,
            &command(
                "read value; printf '\x1b]0;finished\x07BACKGROUND'; exit 7",
            ),
        )
        .unwrap();
    let terminal_id = opened.tab.terminal_id;
    drop(opened.client);
    let client = mux.attach(terminal_id).unwrap();
    client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut title = false;
    let mut exited = false;
    // No snapshot requests while the terminal is hidden.
    while !title || !exited {
        while let Some(event) = client.try_recv_event().unwrap() {
            match event {
                TerminalEvent::TitleChanged { title: value, .. } => {
                    title |= value == "finished";
                }
                TerminalEvent::Exited { status, .. } => {
                    assert_eq!(status.code, Some(7));
                    exited = true;
                }
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "missing title or exit");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(mux.workspace(workspace).unwrap().tabs.len(), 1);
    wait_for_text(&client, "BACKGROUND");
    mux.shutdown().unwrap();
}
