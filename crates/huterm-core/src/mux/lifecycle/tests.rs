use super::*;
use huterm_protocol::{CellSize, GridSize, TerminalCommand, TerminalInput};

fn command(script: &str) -> TerminalCommand {
    TerminalCommand {
        program: "/bin/sh".into(),
        arguments: vec!["-c".into(), script.into()],
        working_directory: std::env::current_dir().unwrap(),
        environment: Vec::new(),
        grid_size: GridSize::clamped(80, 24),
        cell_size: CellSize {
            width: 8,
            height: 16,
        },
        presentation: huterm_protocol::TerminalPresentation::default(),
    }
}
fn ready(client: &RuntimeClient, text: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let snapshot = client.read_snapshot().unwrap();
        let output: String =
            snapshot.cells().map(|cell| cell.text.as_str()).collect();
        if output.contains(text) {
            return;
        }
        assert!(Instant::now() < deadline, "missing {text}: {output}");
        std::thread::sleep(Duration::from_millis(10));
    }
}
/// Waits until the root shell holds the terminal's foreground again.
/// Some shells, such as macOS `/bin/sh`, can print after a job ends
/// before they reclaim the terminal.
fn shell_foreground(client: &RuntimeClient) {
    let deadline = Instant::now() + Duration::from_secs(3);
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    loop {
        let mut query = std::pin::pin!(client.has_foreground_job());
        let busy = loop {
            if let std::task::Poll::Ready(busy) =
                std::future::Future::poll(query.as_mut(), &mut context)
            {
                break busy.unwrap();
            }
            assert!(Instant::now() < deadline, "foreground query timed out");
            std::thread::sleep(Duration::from_millis(10));
        };
        if !busy {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "shell never reclaimed the terminal"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn attachment_detach_and_retarget_preserve_sessions_and_validate_atomically() {
    let mut mux = Mux::default();
    let first = mux.create_session(None).unwrap();
    let second = mux.create_session(None).unwrap();
    let attachment = mux.attach_session(first).unwrap();
    assert!(
        mux.retarget_attachment(attachment, SessionId::new(second.get()))
            .is_err()
    );
    assert_eq!(mux.attachment_session(attachment).unwrap(), first);
    mux.retarget_attachment(attachment, second).unwrap();
    assert!(mux.session(first).is_some());
    mux.detach_session(attachment).unwrap();
    assert!(mux.session(second).is_some());
    assert!(mux.attachment_session(attachment).is_err());
    let foreign = Mux::default();
    assert!(foreign.attachment_session(attachment).is_err());
}
#[test]
fn concurrent_window_tickets_reassess_when_last_attachment_changes() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let first = mux.attach_session(session).unwrap();
    let second = mux.attach_session(session).unwrap();
    let a = mux
        .prepare_close(CloseRequest::Window(first))
        .unwrap()
        .check_jobs();
    let b = mux
        .prepare_close(CloseRequest::Window(second))
        .unwrap()
        .check_jobs();
    mux.commit_close(&a, &a.recheck(), false).unwrap();
    assert!(matches!(
        mux.commit_close(&b, &b.recheck(), false),
        Err(MuxError::StaleClose)
    ));
    assert_eq!(mux.attachment_session(second).unwrap(), session);
    let b = mux.prepare_close(CloseRequest::Window(second)).unwrap();
    assert_eq!(b.effect(), CloseEffect::Session(session));
    let b = b.check_jobs();
    mux.commit_close(&b, &b.recheck(), false).unwrap();
    assert!(mux.session(session).is_none());
    assert!(mux.attachment_session(second).is_err());
}
#[test]
fn rejected_wrong_workspace_close_preserves_an_unrelated_assessment() {
    let mut mux = Mux::default();
    let source = mux.create_session(None).unwrap();
    let source_workspace = mux.create_workspace(source, None).unwrap();
    let opened = mux
        .open_tab(source_workspace, &command("printf READY; read value"))
        .unwrap();
    ready(&opened.client, "READY");
    let target = mux.create_session(None).unwrap();
    let wrong_workspace = mux.create_workspace(target, None).unwrap();
    let attachment = mux.attach_session(target).unwrap();
    let assessment = mux
        .prepare_close(CloseRequest::Window(attachment))
        .unwrap()
        .check_jobs();
    assert!(
        matches!(mux.close_tab(wrong_workspace, opened.tab.id), Err(MuxError::UnknownTab(id)) if id == opened.tab.id)
    );
    mux.commit_close(&assessment, &assessment.recheck(), false)
        .expect("rejected close invalidated an unrelated assessment");
    assert!(mux.session(target).is_none());
    assert_eq!(
        mux.select_tab(opened.tab.id).unwrap().workspace,
        Some(source_workspace)
    );
    assert!(opened.client.read_snapshot().is_ok());
    mux.shutdown().unwrap();
}

#[test]
fn structural_changes_invalidate_consent_without_consuming_attachment() {
    let mut mux = Mux::default();
    let first = mux.create_session(None).unwrap();
    let second = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(first, None).unwrap();
    let attachment = mux.attach_session(first).unwrap();
    let ticket = mux
        .prepare_close(CloseRequest::Window(attachment))
        .unwrap()
        .check_jobs();
    mux.move_workspace(first, workspace, second, None).unwrap();
    assert!(matches!(
        mux.commit_close(&ticket, &ticket.recheck(), true),
        Err(MuxError::StaleClose)
    ));
    assert_eq!(mux.attachment_session(attachment).unwrap(), first);
    let ticket = mux
        .prepare_close(CloseRequest::Window(attachment))
        .unwrap()
        .check_jobs();
    mux.retarget_attachment(attachment, second).unwrap();
    assert!(matches!(
        mux.commit_close(&ticket, &ticket.recheck(), true),
        Err(MuxError::StaleClose)
    ));
    assert_eq!(mux.attachment_session(attachment).unwrap(), second);
}
#[test]
fn detach_keeps_real_child_alive_and_last_window_close_reaps_it() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let opened = mux
        .open_tab(
            workspace,
            &command("printf READY; read value; printf ALIVE; read value"),
        )
        .unwrap();
    ready(&opened.client, "READY");
    let pid = opened.client.job_context().unwrap().shell.unwrap();
    let attachment = mux.attach_session(session).unwrap();
    mux.detach_session(attachment).unwrap();
    opened
        .client
        .send_input(TerminalInput::Text("continue\n".into()))
        .unwrap();
    ready(&opened.client, "ALIVE");
    assert_eq!(mux.capture_hierarchy().sessions.len(), 1);
    assert!(mux.capture_hierarchy().attachments.is_empty());
    let attachment = mux.attach_session(session).unwrap();
    let assessment = mux
        .prepare_close(CloseRequest::Window(attachment))
        .unwrap()
        .check_jobs();
    assert!(!assessment.needs_confirmation(), "{:?}", assessment.jobs());
    mux.commit_close(&assessment, &assessment.recheck(), false)
        .unwrap();
    assert_eq!(mux.terminal_count(), 0);
    assert_eq!(
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
            None
        ),
        Err(nix::errno::Errno::ESRCH)
    );
    assert!(opened.client.read_snapshot().is_err());
}
#[test]
fn new_background_job_requires_renewed_consent_and_quit_includes_zero_views() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let opened = mux.open_tab(workspace, &command("printf IDLE; read value; set -m; sleep 30 & printf BUSY; read value")).unwrap();
    ready(&opened.client, "IDLE");
    let idle = mux
        .prepare_close(CloseRequest::Application)
        .unwrap()
        .check_jobs();
    assert!(!idle.needs_confirmation(), "{:?}", idle.jobs());
    opened
        .client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    ready(&opened.client, "BUSY");
    let busy = idle.recheck();
    assert!(busy.needs_confirmation(), "{:?}", busy.jobs());
    assert!(matches!(
        mux.commit_close(&idle, &busy, true),
        Err(MuxError::StaleClose)
    ));
    assert!(matches!(
        mux.commit_close(&busy, &busy.recheck(), false),
        Err(MuxError::ConfirmationRequired)
    ));
    mux.commit_close(&busy, &busy.recheck(), true).unwrap();
    assert_eq!(mux.terminal_count(), 0);
    for state in busy.jobs() {
        if let JobState::Running(jobs) = state {
            for job in jobs {
                assert_eq!(
                    nix::sys::signal::kill(
                        nix::unistd::Pid::from_raw(
                            i32::try_from(job.pid).unwrap()
                        ),
                        None
                    ),
                    Err(nix::errno::Errno::ESRCH),
                    "background job {} survived",
                    job.pid
                );
            }
        }
    }
}
#[test]
fn orphans_holding_the_terminal_need_consent_and_are_cleaned_up() {
    // Job control puts the inner shell in its own group. It exits,
    // leaving `sleep` outside the shell's tree and the foreground group;
    // only its controlling terminal ties it to this tab.
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let opened = mux
        .open_tab(
            workspace,
            &command("set -m; printf IDLE; read value; sh -c 'sleep 30 &'; printf BUSY; read value"),
        )
        .unwrap();
    ready(&opened.client, "IDLE");
    opened
        .client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    ready(&opened.client, "BUSY");
    shell_foreground(&opened.client);
    let busy = mux
        .prepare_close(CloseRequest::Application)
        .unwrap()
        .check_jobs();
    let orphan = busy
        .jobs()
        .iter()
        .find_map(|state| match state {
            JobState::Running(jobs) => {
                jobs.iter().find(|job| job.identity.ends_with(" sleep"))
            }
            _ => None,
        })
        .cloned()
        .unwrap_or_else(|| panic!("orphan missing: {:?}", busy.jobs()));
    assert!(!orphan.foreground);
    assert_eq!(orphan.command, "sleep");
    assert_eq!(orphan.command_line.as_deref(), Some("sleep 30"));
    mux.commit_close(&busy, &busy.recheck(), true).unwrap();
    assert_eq!(
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(orphan.pid).unwrap()),
            None
        ),
        Err(nix::errno::Errno::ESRCH),
        "orphan {} survived close",
        orphan.pid
    );
}
#[test]
fn foreground_exit_and_unavailable_runtime_are_assessed_conservatively() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let opened = mux
        .open_tab(
            workspace,
            &command("printf READY; read value; set -m; sleep 30 & fg"),
        )
        .unwrap();
    ready(&opened.client, "READY");
    let shell_group =
        i32::try_from(opened.client.job_context().unwrap().shell.unwrap())
            .unwrap();
    opened
        .client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let foreground = loop {
        let assessment = mux
            .prepare_close(CloseRequest::Session(session))
            .unwrap()
            .check_jobs();
        // A just-forked child can still be in the shell's foreground group.
        // Wait until job control has moved it into its own foreground group.
        if assessment.jobs().iter().any(|state| {
            matches!(state, JobState::Running(jobs) if jobs.iter().any(|job| job.foreground && job.group != shell_group))
        }) {
            break assessment;
        }
        assert!(
            Instant::now() < deadline,
            "foreground job not observed: {:?}",
            assessment.jobs()
        );
    };
    let rechecked = foreground.recheck();
    let result = mux.commit_close(&foreground, &rechecked, true);
    assert!(
        result.is_ok(),
        "close failed: {result:?}; assessed: {:?}; rechecked: {:?}",
        foreground.jobs(),
        rechecked.jobs()
    );
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let exited = mux.open_tab(workspace, &command("printf DONE")).unwrap();
    ready(&exited.client, "DONE");
    while !exited
        .client
        .job_context()
        .is_some_and(|context| context.exited && context.pty_eof)
    {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let assessment = mux
        .prepare_close(CloseRequest::Application)
        .unwrap()
        .check_jobs();
    assert!(!assessment.needs_confirmation(), "{:?}", assessment.jobs());
    exited.client.close().unwrap();
    while exited.client.job_context().is_some() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let assessment = mux
        .prepare_close(CloseRequest::Application)
        .unwrap()
        .check_jobs();
    assert_eq!(assessment.jobs(), &[JobState::Unknown]);
    assert!(matches!(
        mux.commit_close(&assessment, &assessment.recheck(), false),
        Err(MuxError::ConfirmationRequired)
    ));
    mux.commit_close(&assessment, &assessment.recheck(), true)
        .unwrap();
}

#[test]
fn ghostty_root_exit_completes_the_terminal_even_with_a_surviving_slave_holder()
{
    exited_holder();
}

fn exited_holder() {
    let fixture = super::holder_fixture::HolderFixture::new();
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let command = fixture.command();
    let opened = mux.open_tab(workspace, &command).unwrap();
    let helper = fixture.wait_ready();
    let root = opened.client.job_context().unwrap().shell.unwrap();
    let root = nix::unistd::Pid::from_raw(i32::try_from(root).unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !opened.client.job_context().unwrap().exited {
        assert!(
            Instant::now() < deadline,
            "root did not exit; helper={helper}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut exit_events = 0;
    while let Some(event) = opened.client.try_recv_event().unwrap() {
        if let huterm_protocol::TerminalEvent::Exited { status, .. } = event {
            exit_events += 1;
            assert_eq!(
                status,
                huterm_protocol::ExitStatus {
                    code: Some(1),
                    success: false
                }
            );
        }
    }
    assert_eq!(exit_events, 1, "signaled root exit must be reported once");
    assert!(
        nix::sys::signal::kill(root, None).is_err(),
        "root must be reaped when its exit is published"
    );
    assert!(!fixture.directory.join("done").exists());
    assert!(nix::sys::signal::kill(helper, None).is_ok());
    let assessment = mux
        .prepare_close(CloseRequest::Application)
        .unwrap()
        .check_jobs();
    assert!(nix::sys::signal::kill(helper, None).is_ok());
    assert!(
        !assessment.needs_confirmation(),
        "live helper={helper}, context={:?}, jobs={:?}",
        opened.client.job_context(),
        assessment.jobs()
    );
    let current = assessment.recheck();
    assert_eq!(current.jobs(), &[JobState::Idle]);
    while let Some(event) = opened.client.try_recv_event().unwrap() {
        assert!(
            !matches!(event, huterm_protocol::TerminalEvent::Exited { .. }),
            "reaping emitted a second exit event"
        );
    }
    assert!(opened.client.read_snapshot().is_ok());
    mux.commit_close(&current, &current.recheck(), false)
        .unwrap();
    assert!(
        nix::sys::signal::kill(helper, None).is_ok(),
        "closing completed history must not signal old process groups"
    );
    fixture.release().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !fixture.directory.join("done").exists() {
        assert!(
            Instant::now() < deadline,
            "helper did not finish after release"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn consent_survives_child_replacement_inside_the_same_live_job_group() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let opened = mux.open_tab(workspace, &command("sleep 30 & first=$!; printf FIRST; read step; kill $first; wait $first 2>/dev/null; sleep 30 & printf SECOND; read step")).unwrap();
    ready(&opened.client, "FIRST");
    let consent = mux
        .prepare_close(CloseRequest::Application)
        .unwrap()
        .check_jobs();
    assert!(consent.needs_confirmation());
    opened
        .client
        .send_input(TerminalInput::Text("next\n".into()))
        .unwrap();
    ready(&opened.client, "SECOND");
    let current = consent.recheck();
    assert_ne!(
        consent.jobs(),
        current.jobs(),
        "fixture did not replace the child"
    );
    mux.commit_close(&consent, &current, true).unwrap();
    assert_eq!(mux.terminal_count(), 0);
    for state in current.jobs() {
        if let JobState::Running(jobs) = state {
            for job in jobs {
                assert_eq!(
                    nix::sys::signal::kill(
                        nix::unistd::Pid::from_raw(
                            i32::try_from(job.pid).unwrap()
                        ),
                        None
                    ),
                    Err(nix::errno::Errno::ESRCH)
                );
            }
        }
    }
}

#[test]
fn accepted_capture_precedes_teardown_without_a_second_freshness_check() {
    use std::cell::Cell;

    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let assessment = mux
        .prepare_close(CloseRequest::Application)
        .unwrap()
        .check_jobs();
    let mut captured = None;
    let clock_reads = Cell::new(0);
    mux.commit_close_with_clock(
        &assessment,
        &assessment,
        false,
        |mux| captured = Some(mux.capture_hierarchy()),
        || {
            let read = clock_reads.get();
            clock_reads.set(read + 1);
            assessment.checked_at
                + if read == 0 {
                    CLOSE_ASSESSMENT_MAX_AGE
                } else {
                    CLOSE_ASSESSMENT_MAX_AGE + Duration::from_millis(1)
                }
        },
    )
    .unwrap();
    assert_eq!(clock_reads.get(), 1, "teardown rechecked freshness");
    assert_eq!(captured.unwrap().sessions[0].id, session);
    assert!(mux.sessions().is_empty());
}

/// Opens `count` idle tabs and waits for each shell to reach `read`.
fn open_idle_tabs(
    mux: &mut Mux,
    workspace: WorkspaceId,
    count: usize,
) -> Vec<crate::OpenedTab> {
    let tabs: Vec<_> = (0..count)
        .map(|_| {
            mux.open_tab(workspace, &command("printf READY; read value"))
                .unwrap()
        })
        .collect();
    for opened in &tabs {
        ready(&opened.client, "READY");
    }
    tabs
}
fn tab_ids(mux: &Mux, workspace: WorkspaceId) -> Vec<TabId> {
    mux.workspace(workspace)
        .unwrap()
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect()
}

#[test]
fn tabs_ticket_covers_the_distinct_requested_terminals_and_commit_keeps_siblings()
 {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let [first, second, third] =
        <[_; 3]>::try_from(open_idle_tabs(&mut mux, workspace, 3))
            .ok()
            .unwrap();
    let ticket = mux
        .prepare_close(CloseRequest::Tabs {
            workspace,
            tabs: vec![third.tab.id, first.tab.id, third.tab.id],
        })
        .unwrap();
    assert_eq!(
        ticket.effect(),
        CloseEffect::Tabs {
            workspace,
            tabs: vec![third.tab.id, first.tab.id],
        }
    );
    let assessment = ticket.check_jobs();
    assert_eq!(
        assessment.terminals(),
        vec![third.tab.terminal_id, first.tab.terminal_id]
    );
    assert_eq!(assessment.jobs().len(), 2);
    assert!(
        assessment
            .terminal_jobs()
            .map(|(terminal, _)| terminal)
            .eq(assessment.terminals()),
        "paired evidence must follow the terminal order"
    );
    assert!(!assessment.needs_confirmation(), "{:?}", assessment.jobs());
    mux.commit_close(&assessment, &assessment.recheck(), false)
        .unwrap();
    assert_eq!(tab_ids(&mux, workspace), vec![second.tab.id]);
    assert_eq!(mux.terminal_count(), 1);
    assert!(second.client.read_snapshot().is_ok());
    assert!(first.client.read_snapshot().is_err());
    assert!(third.client.read_snapshot().is_err());
    mux.shutdown().unwrap();
}

#[test]
fn tabs_close_rejects_invalid_requests_and_is_stale_after_structural_change() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let other = mux.create_workspace(session, None).unwrap();
    let [first, second] =
        <[_; 2]>::try_from(open_idle_tabs(&mut mux, workspace, 2))
            .ok()
            .unwrap();
    assert!(matches!(
        mux.prepare_close(CloseRequest::Tabs {
            workspace,
            tabs: Vec::new()
        }),
        Err(MuxError::EmptyClose)
    ));
    let assessment = mux
        .prepare_close(CloseRequest::Tabs {
            workspace,
            tabs: vec![first.tab.id, second.tab.id],
        })
        .unwrap()
        .check_jobs();
    mux.move_tab(workspace, first.tab.id, other, None).unwrap();
    assert!(matches!(
        mux.commit_close(&assessment, &assessment.recheck(), true),
        Err(MuxError::StaleClose)
    ));
    assert_eq!(tab_ids(&mux, workspace), vec![second.tab.id]);
    assert_eq!(tab_ids(&mux, other), vec![first.tab.id]);
    assert_eq!(mux.terminal_count(), 2);
    assert!(first.client.read_snapshot().is_ok());
    assert!(second.client.read_snapshot().is_ok());
    assert!(matches!(
        mux.prepare_close(CloseRequest::Tabs {
            workspace,
            tabs: vec![second.tab.id, first.tab.id],
        }),
        Err(MuxError::UnknownTab(id)) if id == first.tab.id
    ));
    assert_eq!(mux.terminal_count(), 2);
    mux.shutdown().unwrap();
}

#[test]
fn explicit_session_termination_invalidates_every_view() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let first = mux.attach_session(session).unwrap();
    let second = mux.attach_session(session).unwrap();
    mux.close_session(session).unwrap();
    assert!(mux.attachment_session(first).is_err());
    assert!(mux.attachment_session(second).is_err());
}
