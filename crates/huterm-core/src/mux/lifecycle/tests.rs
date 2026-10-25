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
fn ready(client: &crate::test_support::TestClient, text: &str) {
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
fn shell_foreground(client: &crate::test_support::TestClient) {
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
        .open_test_tab(source_workspace, &command("printf READY; read value"))
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
    // A tab moving to another session also changes the window's scope.
    let source = mux.create_workspace(second, None).unwrap();
    let elsewhere = mux.create_session(None).unwrap();
    let destination = mux.create_workspace(elsewhere, None).unwrap();
    let opened = mux
        .open_test_tab(source, &command("printf READY; read value"))
        .unwrap();
    ready(&opened.client, "READY");
    let ticket = mux
        .prepare_close(CloseRequest::Window(attachment))
        .unwrap()
        .check_jobs();
    mux.move_tab(source, opened.tab.id, destination, None)
        .unwrap();
    assert!(matches!(
        mux.commit_close(&ticket, &ticket.recheck(), true),
        Err(MuxError::StaleClose)
    ));
    assert_eq!(mux.attachment_session(attachment).unwrap(), second);
    assert_eq!(
        mux.select_tab(opened.tab.id).unwrap().workspace,
        Some(destination)
    );
    mux.shutdown().unwrap();
}
fn pid_of(client: &crate::test_support::TestClient) -> nix::unistd::Pid {
    let pid = client.job_context().unwrap().shell.unwrap();
    nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap())
}
/// Sends a line to a child that answers each line by printing the next
/// step's marker, and waits for `marker`.
fn answers(client: &crate::test_support::TestClient, marker: &str) {
    client
        .send_input(TerminalInput::Text("next\n".into()))
        .expect("the child no longer accepts input");
    ready(client, marker);
}
#[test]
fn retarget_keeps_the_old_sessions_child_and_closing_the_new_one_spares_it() {
    let steps = "printf READY; read value; printf ALIVE; read value; \
                 printf AGAIN; read value";
    let mut mux = Mux::default();
    let first = mux.create_session(None).unwrap();
    let first_workspace = mux.create_workspace(first, None).unwrap();
    let kept = mux.open_test_tab(first_workspace, &command(steps)).unwrap();
    let second = mux.create_session(None).unwrap();
    let second_workspace = mux.create_workspace(second, None).unwrap();
    let closed = mux
        .open_test_tab(second_workspace, &command(steps))
        .unwrap();
    ready(&kept.client, "READY");
    ready(&closed.client, "READY");
    let attachment = mux.attach_session(first).unwrap();
    mux.retarget_attachment(attachment, second).unwrap();
    answers(&kept.client, "ALIVE");
    let closed_pid = pid_of(&closed.client);
    mux.close_session(second).unwrap();
    assert_eq!(
        nix::sys::signal::kill(closed_pid, None),
        Err(nix::errno::Errno::ESRCH),
        "the closed session's child survived"
    );
    assert!(mux.attachment_session(attachment).is_err());
    answers(&kept.client, "AGAIN");
    assert!(nix::sys::signal::kill(pid_of(&kept.client), None).is_ok());
    mux.shutdown().unwrap();
}
#[test]
fn emptied_window_closes_stop_no_terminal_and_never_need_consent() {
    let steps = "printf READY; read value; printf ALIVE; read value";
    let emptied = |mux: &Mux, attachment, workspace| {
        mux.prepare_close(CloseRequest::EmptiedWindow {
            attachment,
            workspace,
        })
    };
    let commit = |mux: &mut Mux, ticket: CloseTicket| {
        let assessment = ticket.check_jobs();
        assert!(!assessment.needs_confirmation(), "{:?}", assessment.jobs());
        mux.commit_close(&assessment, &assessment.recheck(), false)
    };
    let mut mux = Mux::default();

    // A workspace that still holds tabs refuses and changes nothing.
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let held = mux.open_test_tab(workspace, &command(steps)).unwrap();
    ready(&held.client, "READY");
    let attachment = mux.attach_session(session).unwrap();
    let before = mux.capture_hierarchy();
    assert!(matches!(
        emptied(&mux, attachment, workspace),
        Err(MuxError::ViewNotEmpty(refused)) if refused == workspace
    ));
    assert_eq!(mux.capture_hierarchy(), before);

    // The tab moves to a sibling workspace: the sole attachment detaches
    // and the moved child keeps running in the unviewed session.
    let sibling = mux.create_workspace(session, None).unwrap();
    mux.move_tab(workspace, held.tab.id, sibling, None).unwrap();
    let ticket = emptied(&mux, attachment, workspace).unwrap();
    assert_eq!(ticket.effect(), CloseEffect::Detach(attachment));
    commit(&mut mux, ticket).unwrap();
    assert!(mux.attachment_session(attachment).is_err());
    assert!(mux.session(session).is_some());
    answers(&held.client, "ALIVE");

    // Another attachment holds the session: detach.
    let shared = mux.create_session(None).unwrap();
    let shared_workspace = mux.create_workspace(shared, None).unwrap();
    let first = mux.attach_session(shared).unwrap();
    let second = mux.attach_session(shared).unwrap();
    let ticket = emptied(&mux, first, shared_workspace).unwrap();
    assert_eq!(ticket.effect(), CloseEffect::Detach(first));
    commit(&mut mux, ticket).unwrap();
    assert_eq!(mux.attachment_session(second).unwrap(), shared);

    // The last attachment of an empty session deletes it, and the
    // invalidated attachment is then unknown.
    let ticket = emptied(&mux, second, shared_workspace).unwrap();
    assert_eq!(ticket.effect(), CloseEffect::Session(shared));
    commit(&mut mux, ticket).unwrap();
    assert!(mux.session(shared).is_none());
    assert!(matches!(
        emptied(&mux, second, shared_workspace),
        Err(MuxError::UnknownAttachment(unknown)) if unknown == second
    ));

    // A tab opened between assessment and commit makes the commit stale,
    // and the next assessment refuses.
    let arriving = mux.create_session(None).unwrap();
    let arriving_workspace = mux.create_workspace(arriving, None).unwrap();
    let attachment = mux.attach_session(arriving).unwrap();
    let ticket = emptied(&mux, attachment, arriving_workspace).unwrap();
    assert_eq!(ticket.effect(), CloseEffect::Session(arriving));
    let opened = mux
        .open_test_tab(arriving_workspace, &command(steps))
        .unwrap();
    ready(&opened.client, "READY");
    assert!(matches!(
        commit(&mut mux, ticket),
        Err(MuxError::StaleClose)
    ));
    assert!(matches!(
        emptied(&mux, attachment, arriving_workspace),
        Err(MuxError::ViewNotEmpty(_))
    ));
    answers(&opened.client, "ALIVE");
    assert_eq!(mux.terminal_count(), 2);

    // A workspace of another runtime is foreign.
    let mut foreign = Mux::default();
    let foreign_session = foreign.create_session(None).unwrap();
    let foreign_workspace =
        foreign.create_workspace(foreign_session, None).unwrap();
    assert!(matches!(
        emptied(&mux, attachment, foreign_workspace),
        Err(MuxError::ForeignRuntime(runtime))
            if runtime == foreign_workspace.runtime()
    ));
    mux.shutdown().unwrap();
}
#[test]
fn detach_keeps_real_child_alive_and_last_window_close_reaps_it() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let opened = mux
        .open_test_tab(
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
    assert_eq!(mux.capture_hierarchy().attachments, []);
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
    let opened = mux.open_test_tab(workspace, &command("printf IDLE; read value; set -m; sleep 30 & printf BUSY; read value")).unwrap();
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
        .open_test_tab(
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
        .open_test_tab(
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
    let exited = mux
        .open_test_tab(workspace, &command("printf DONE"))
        .unwrap();
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
    // A live runtime that does not answer is unknown, unlike a stopped one.
    let (entered, paused) = std::sync::mpsc::channel();
    let (resume, release) = std::sync::mpsc::channel::<()>();
    exited
        .client
        .control
        .send_control(crate::terminal::RuntimeControl::Pause {
            entered,
            release,
        })
        .unwrap();
    paused.recv_timeout(Duration::from_secs(3)).unwrap();
    let assessment = mux
        .prepare_close(CloseRequest::Application)
        .unwrap()
        .check_jobs();
    assert_eq!(assessment.jobs(), &[JobState::Unknown]);
    assert!(matches!(
        mux.commit_close(&assessment, &assessment.recheck(), false),
        Err(MuxError::ConfirmationRequired)
    ));
    drop(resume);
    mux.commit_close(&assessment, &assessment.recheck(), true)
        .unwrap();
}
/// Panics `client`'s runtime and waits until it stopped.
fn panic_runtime(client: &crate::test_support::TestClient) {
    client
        .control
        .send_control(crate::terminal::RuntimeControl::Panic)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !client.control.registry().is_closed() {
        assert!(Instant::now() < deadline, "the runtime never stopped");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn a_panicked_tab_closes_without_consent_or_failure_and_spares_its_sibling() {
    let steps = "printf READY; read value; printf ALIVE; read value";
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let panicked = mux.open_test_tab(workspace, &command(steps)).unwrap();
    let sibling = mux.open_test_tab(workspace, &command(steps)).unwrap();
    ready(&panicked.client, "READY");
    ready(&sibling.client, "READY");
    let pid = pid_of(&panicked.client);
    panic_runtime(&panicked.client);
    let assessment = mux
        .prepare_close(CloseRequest::Tab {
            workspace,
            tab: panicked.tab.id,
        })
        .unwrap()
        .check_jobs();
    assert_eq!(assessment.jobs(), &[JobState::Idle]);
    mux.commit_close(&assessment, &assessment.recheck(), false)
        .expect("closing a stopped terminal reported a failure");
    assert_eq!(tab_ids(&mux, workspace), vec![sibling.tab.id]);
    answers(&sibling.client, "ALIVE");
    // The panicked runtime's own teardown reaps its shell.
    assert_eq!(
        nix::sys::signal::kill(pid, None),
        Err(nix::errno::Errno::ESRCH),
        "the panicked tab's shell survived"
    );
    mux.shutdown().unwrap();
}
#[test]
fn window_session_and_quit_closes_need_no_consent_for_a_panicked_member() {
    fn panicked_session(
        mux: &mut Mux,
    ) -> (SessionId, Vec<crate::mux::TestTab>) {
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let tabs = open_idle_tabs(mux, workspace, 2);
        panic_runtime(&tabs[0].client);
        (session, tabs)
    }
    let mut mux = Mux::default();
    // The last window's close terminates the session.
    let (closed, _tabs) = panicked_session(&mut mux);
    let attachment = mux.attach_session(closed).unwrap();
    let assessment = mux
        .prepare_close(CloseRequest::Window(attachment))
        .unwrap()
        .check_jobs();
    assert_eq!(assessment.jobs(), &[JobState::Idle, JobState::Idle]);
    mux.commit_close(&assessment, &assessment.recheck(), false)
        .expect("a window close with a stopped terminal reported a failure");
    assert!(mux.session(closed).is_none());
    // Quit needs no consent for it, and its shutdown reports no failure.
    let (_kept, _tabs) = panicked_session(&mut mux);
    let assessment = mux
        .prepare_close(CloseRequest::Application)
        .unwrap()
        .check_jobs();
    assert!(!assessment.needs_confirmation(), "{:?}", assessment.jobs());
    mux.shutdown()
        .expect("shutdown with a stopped terminal reported a failure");
    assert_eq!(mux.terminal_count(), 0);
}

#[test]
fn a_stopped_terminal_keeps_job_evidence_aligned_with_a_busy_sibling() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let stopped = open_idle_tabs(&mut mux, workspace, 1).remove(0);
    let busy = mux
        .open_test_tab(
            workspace,
            &command("set -m; sleep 30 & printf BUSY; read value"),
        )
        .unwrap();
    ready(&busy.client, "BUSY");
    panic_runtime(&stopped.client);
    let assessment = mux
        .prepare_close(CloseRequest::Session(session))
        .unwrap()
        .check_jobs();
    let jobs: Vec<_> = assessment.terminal_jobs().collect();
    // The stopped terminal skips inspection, but its sibling's evidence
    // still lines up with the sibling.
    assert_eq!(jobs[0], (stopped.tab.terminal_id, &JobState::Idle));
    assert!(
        matches!(jobs[1], (id, JobState::Running(_)) if id == busy.tab.terminal_id),
        "{jobs:?}"
    );
    assert!(assessment.needs_confirmation());
    mux.commit_close(&assessment, &assessment.recheck(), true)
        .expect("a session close with a stopped terminal reported a failure");
    assert_eq!(mux.terminal_count(), 0);
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
    let opened = mux.open_test_tab(workspace, &command).unwrap();
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
    assert_eq!(
        opened.client.poll().exited,
        Some(huterm_protocol::ExitStatus {
            code: Some(1),
            success: false
        }),
        "signaled root exit must be reported"
    );
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
    // Status is state, so a second exit cannot be published; reaping must
    // leave the published exit as it was.
    assert_eq!(
        opened.client.status().lifecycle,
        huterm_protocol::TerminalLifecycle::Exited(
            huterm_protocol::ExitStatus {
                code: Some(1),
                success: false
            }
        ),
        "reaping changed the published exit"
    );
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
    let opened = mux.open_test_tab(workspace, &command("sleep 30 & first=$!; printf FIRST; read step; kill $first; wait $first 2>/dev/null; sleep 30 & printf SECOND; read step")).unwrap();
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
    assert_eq!(mux.sessions(), []);
}

/// Opens `count` idle tabs and waits for each shell to reach `read`.
fn open_idle_tabs(
    mux: &mut Mux,
    workspace: WorkspaceId,
    count: usize,
) -> Vec<crate::mux::TestTab> {
    let tabs: Vec<_> = (0..count)
        .map(|_| {
            mux.open_test_tab(workspace, &command("printf READY; read value"))
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
