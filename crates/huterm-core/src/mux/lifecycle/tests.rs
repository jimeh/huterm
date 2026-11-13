use super::*;
use crate::test_support::wait_for_text;
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
        scrollback: huterm_protocol::ScrollbackLimit::DEFAULT,
        shell_integration: huterm_protocol::ShellIntegration::OFF,
        scrollback_compression: false,
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
    wait_for_text(&opened.client, "READY");
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
    wait_for_text(&opened.client, "READY");
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
    wait_for_text(client, marker);
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
    wait_for_text(&kept.client, "READY");
    wait_for_text(&closed.client, "READY");
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
    wait_for_text(&held.client, "READY");
    let attachment = mux.attach_session(session).unwrap();
    let before = mux.structure();
    assert!(matches!(
        emptied(&mux, attachment, workspace),
        Err(MuxError::ViewNotEmpty(refused)) if refused == workspace
    ));
    assert_eq!(mux.structure(), before);

    // The tab moves to a sibling workspace: the window leaves the empty
    // workspace, which closes, and keeps its attachment; the moved child
    // keeps running.
    let sibling = mux.create_workspace(session, None).unwrap();
    mux.move_tab(workspace, held.tab.id, sibling, None).unwrap();
    let ticket = emptied(&mux, attachment, workspace).unwrap();
    assert_eq!(
        ticket.effect(),
        CloseEffect::LeaveWorkspace {
            attachment,
            workspace
        }
    );
    commit(&mut mux, ticket).unwrap();
    assert_eq!(mux.attachment_session(attachment).unwrap(), session);
    assert!(mux.workspace(workspace).is_none());
    assert_eq!(mux.session(session).unwrap().workspaces, [sibling]);
    answers(&held.client, "ALIVE");
    mux.detach_session(attachment).unwrap();

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
    wait_for_text(&opened.client, "READY");
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
fn an_emptied_hidden_workspace_closes_only_when_empty_unpinned_and_not_alone() {
    let steps = "printf READY; read value";
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let shown = mux.create_workspace(session, None).unwrap();
    let hidden = mux.create_workspace(session, None).unwrap();
    let other = mux.create_session(None).unwrap();
    let elsewhere = mux.create_workspace(other, None).unwrap();
    mux.create_workspace(other, None).unwrap();

    // A workspace that holds a tab stays, and so does a pinned one.
    let held = mux.open_test_tab(hidden, &command(steps)).unwrap();
    wait_for_text(&held.client, "READY");
    assert!(!mux.close_emptied_workspace(session, hidden).unwrap());
    mux.close_tab(hidden, held.tab.id).unwrap();
    mux.set_workspace_pinned(hidden, true).unwrap();
    assert!(!mux.close_emptied_workspace(session, hidden).unwrap());
    assert!(mux.workspace(hidden).is_some());

    // Another session's workspace stays, however empty.
    assert!(!mux.close_emptied_workspace(session, elsewhere).unwrap());
    assert!(mux.workspace(elsewhere).is_some());

    // Empty and unpinned beside a sibling, it closes.
    mux.set_workspace_pinned(hidden, false).unwrap();
    assert!(mux.close_emptied_workspace(session, hidden).unwrap());
    assert!(mux.workspace(hidden).is_none());
    assert_eq!(mux.session(session).unwrap().workspaces, [shown]);

    // A session's only workspace stays, and a gone one changes nothing.
    assert!(!mux.close_emptied_workspace(session, shown).unwrap());
    assert!(!mux.close_emptied_workspace(session, hidden).unwrap());
    assert_eq!(mux.session(session).unwrap().workspaces, [shown]);
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
    wait_for_text(&opened.client, "READY");
    let pid = opened.client.job_context().unwrap().shell.unwrap();
    let attachment = mux.attach_session(session).unwrap();
    mux.detach_session(attachment).unwrap();
    opened
        .client
        .send_input(TerminalInput::Text("continue\n".into()))
        .unwrap();
    wait_for_text(&opened.client, "ALIVE");
    assert_eq!(mux.sessions().len(), 1);
    assert!(!mux.session_attached(session));
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
    wait_for_text(&opened.client, "IDLE");
    let idle = mux
        .prepare_close(CloseRequest::Application)
        .unwrap()
        .check_jobs();
    assert!(!idle.needs_confirmation(), "{:?}", idle.jobs());
    opened
        .client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    wait_for_text(&opened.client, "BUSY");
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
    // The job starts `sleep` in a group of its own and exits, leaving it
    // outside the shell's tree and the foreground group; only its
    // controlling terminal ties it to this tab.
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let opened = mux
        .open_test_tab(
            workspace,
            &command(&format!(
                "printf IDLE; read value; {}; printf BUSY; read value",
                crate::test_support::orphaned_sleep()
            )),
        )
        .unwrap();
    wait_for_text(&opened.client, "IDLE");
    opened
        .client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    wait_for_text(&opened.client, "BUSY");
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
            &command(&format!(
                "printf READY; read value; {}; printf DONE",
                crate::test_support::foreground_job(None)
            )),
        )
        .unwrap();
    wait_for_text(&opened.client, "READY");
    let shell_group =
        i32::try_from(opened.client.job_context().unwrap().shell.unwrap())
            .unwrap();
    opened
        .client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    wait_for_text(&opened.client, crate::test_support::HELD);
    let deadline = Instant::now() + Duration::from_secs(3);
    let foreground = loop {
        let assessment = mux
            .prepare_close(CloseRequest::Session(session))
            .unwrap()
            .check_jobs();
        // The job held the foreground before it printed; the runtime
        // observes it through its own probe.
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
    wait_for_text(&exited.client, "DONE");
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
    wait_for_text(&panicked.client, "READY");
    wait_for_text(&sibling.client, "READY");
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
    wait_for_text(&busy.client, "BUSY");
    panic_runtime(&stopped.client);
    let assessment = mux
        .prepare_close(CloseRequest::Session(session))
        .unwrap()
        .check_jobs();
    let jobs: Vec<_> = assessment.terminal_jobs().collect();
    // The stopped terminal skips inspection, but its sibling's evidence
    // still lines up with the sibling.
    assert_eq!(
        jobs[0],
        (stopped.tab.first_pane().terminal_id, &JobState::Idle)
    );
    assert!(
        matches!(jobs[1], (id, JobState::Running(_)) if id == busy.tab.first_pane().terminal_id),
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
    wait_for_text(&opened.client, "FIRST");
    let consent = mux
        .prepare_close(CloseRequest::Application)
        .unwrap()
        .check_jobs();
    assert!(consent.needs_confirmation());
    opened
        .client
        .send_input(TerminalInput::Text("next\n".into()))
        .unwrap();
    wait_for_text(&opened.client, "SECOND");
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
        |mux| captured = Some(mux.sessions().to_vec()),
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
    assert_eq!(captured.unwrap()[0].id, session);
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
        wait_for_text(&opened.client, "READY");
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
        vec![
            third.tab.first_pane().terminal_id,
            first.tab.first_pane().terminal_id
        ]
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

/// An idle tab split into a second pane running `script`, both ready.
fn split_tab(
    mux: &mut Mux,
    workspace: WorkspaceId,
    script: &str,
    marker: &str,
) -> (crate::mux::TestTab, crate::mux::TestPane) {
    let first = open_idle_tabs(mux, workspace, 1).remove(0);
    let second = mux
        .split_test_pane(
            workspace,
            first.tab.id,
            first.tab.first_pane().id,
            huterm_protocol::SplitDirection::Right,
            &command(script),
        )
        .unwrap();
    wait_for_text(&second.client, marker);
    (first, second)
}

#[test]
fn a_pane_ticket_covers_its_own_terminal_and_the_last_pane_closes_the_tab() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let (idle, busy) = split_tab(
        &mut mux,
        workspace,
        "set -m; sleep 30 & printf BUSY; read value",
        "BUSY",
    );
    let tab = idle.tab.id;
    let idle_pane = idle.tab.first_pane().id;
    // A busy pane's own ticket needs consent while its sibling stays.
    let busy_ticket = mux
        .prepare_close(CloseRequest::Pane {
            workspace,
            tab,
            pane: busy.pane.id,
        })
        .unwrap();
    assert!(matches!(busy_ticket.effect(), CloseEffect::Pane { .. }));
    let busy_assessment = busy_ticket.check_jobs();
    assert_eq!(busy_assessment.terminals(), [busy.pane.terminal_id]);
    assert!(busy_assessment.needs_confirmation());
    let ticket = mux
        .prepare_close(CloseRequest::Pane {
            workspace,
            tab,
            pane: idle_pane,
        })
        .unwrap();
    assert_eq!(
        ticket.effect(),
        CloseEffect::Pane {
            workspace,
            tab,
            pane: idle_pane,
        }
    );
    let assessment = ticket.check_jobs();
    assert_eq!(assessment.terminals(), [idle.tab.first_pane().terminal_id]);
    // The busy sibling's job does not make the idle pane's close prompt.
    assert!(!assessment.needs_confirmation(), "{:?}", assessment.jobs());
    mux.commit_close(&assessment, &assessment.recheck(), false)
        .unwrap();
    assert!(idle.client.read_snapshot().is_err());
    assert!(busy.client.read_snapshot().is_ok());
    assert_eq!(mux.tab(tab).unwrap().layout().pane_count(), 1);

    let ticket = mux
        .prepare_close(CloseRequest::Pane {
            workspace,
            tab,
            pane: busy.pane.id,
        })
        .unwrap();
    assert_eq!(ticket.effect(), CloseEffect::Tab { workspace, tab });
    let assessment = ticket.check_jobs();
    assert!(assessment.needs_confirmation());
    assert!(matches!(
        mux.commit_close(&assessment, &assessment.recheck(), false),
        Err(MuxError::ConfirmationRequired)
    ));
    mux.commit_close(&assessment, &assessment.recheck(), true)
        .unwrap();
    assert!(mux.tab(tab).is_none());
    assert_eq!(mux.terminal_count(), 0);
    assert!(matches!(
        mux.prepare_close(CloseRequest::Pane {
            workspace,
            tab,
            pane: busy.pane.id,
        }),
        Err(MuxError::UnknownTab(_))
    ));
}

#[test]
fn a_tab_close_covers_every_pane_and_prompts_for_a_job_in_any_of_them() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let (idle, busy) = split_tab(
        &mut mux,
        workspace,
        "set -m; sleep 30 & printf BUSY; read value",
        "BUSY",
    );
    let assessment = mux
        .prepare_close(CloseRequest::Tab {
            workspace,
            tab: idle.tab.id,
        })
        .unwrap()
        .check_jobs();
    assert_eq!(
        assessment.terminals(),
        [idle.tab.first_pane().terminal_id, busy.pane.terminal_id]
    );
    assert!(assessment.needs_confirmation());
    mux.commit_close(&assessment, &assessment.recheck(), true)
        .unwrap();
    assert_eq!(mux.terminal_count(), 0);
}

#[test]
fn splits_make_pane_and_tab_closes_stale_but_resizes_and_zoom_do_not() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let (first, second) =
        split_tab(&mut mux, workspace, "printf READY; read value", "READY");
    let tab = first.tab.id;
    let first_pane = first.tab.first_pane().id;
    let tab_close = CloseRequest::Tab { workspace, tab };
    let assessment = mux.prepare_close(tab_close.clone()).unwrap().check_jobs();
    let huterm_protocol::PaneNode::Split(root) =
        mux.tab(tab).unwrap().layout().root().clone()
    else {
        unreachable!()
    };
    let children: Vec<_> =
        root.children.iter().map(|child| child.node.id()).collect();
    mux.resize_split(workspace, tab, root.id, &children, &[300_000, 700_000])
        .unwrap();
    mux.set_pane_zoom(workspace, tab, Some(second.pane.id))
        .unwrap();
    // Neither changes what the close covers, so its commit stays valid.
    mux.validate_close(&assessment, &assessment.recheck(), false)
        .unwrap();

    // A pane close assessed before another window splits that pane goes
    // stale, and its retry closes only the original pane.
    let pane_close = CloseRequest::Pane {
        workspace,
        tab,
        pane: first_pane,
    };
    let assessment =
        mux.prepare_close(pane_close.clone()).unwrap().check_jobs();
    let added = mux
        .split_test_pane(
            workspace,
            tab,
            first_pane,
            huterm_protocol::SplitDirection::Down,
            &command("printf ADDED; read value"),
        )
        .unwrap();
    wait_for_text(&added.client, "ADDED");
    assert!(matches!(
        mux.commit_close(&assessment, &assessment.recheck(), false),
        Err(MuxError::StaleClose)
    ));
    let assessment = mux.prepare_close(pane_close).unwrap().check_jobs();
    mux.commit_close(&assessment, &assessment.recheck(), false)
        .unwrap();
    assert!(first.client.read_snapshot().is_err());
    assert!(added.client.read_snapshot().is_ok());
    assert!(second.client.read_snapshot().is_ok());
    assert_eq!(mux.tab(tab).unwrap().layout().pane_count(), 2);

    // A tab close assessed before a split is stale too, and its retry
    // covers the new pane.
    let assessment = mux.prepare_close(tab_close.clone()).unwrap().check_jobs();
    let later = mux
        .split_pane(
            workspace,
            tab,
            second.pane.id,
            huterm_protocol::SplitDirection::Down,
            &command("read value"),
        )
        .unwrap();
    assert!(matches!(
        mux.commit_close(&assessment, &assessment.recheck(), false),
        Err(MuxError::StaleClose)
    ));
    let assessment = mux.prepare_close(tab_close).unwrap().check_jobs();
    assert!(assessment.terminals().contains(&later.pane.terminal_id));
    mux.commit_close(&assessment, &assessment.recheck(), false)
        .unwrap();
    assert_eq!(mux.terminal_count(), 0);
}

#[test]
fn arrangements_leave_pane_and_tab_close_assessments_valid() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let (first, second) =
        split_tab(&mut mux, workspace, "printf READY; read value", "READY");
    let tab = first.tab.id;
    let pane_close = CloseRequest::Pane {
        workspace,
        tab,
        pane: first.tab.first_pane().id,
    };
    let tab_close = CloseRequest::Tab { workspace, tab };
    let pane_assessment = mux.prepare_close(pane_close).unwrap().check_jobs();
    let tab_assessment = mux.prepare_close(tab_close).unwrap().check_jobs();
    // Another window rearranges the tab into rows, which takes a new split
    // identity.
    let current = mux.tab(tab).unwrap().layout().clone();
    let panes: Vec<_> = current.panes().iter().map(|pane| pane.id).collect();
    let rows = huterm_protocol::NamedLayout::built_in("rows")
        .unwrap()
        .template
        .expand(&panes)
        .unwrap();
    mux.arrange_panes(workspace, tab, &current, &rows).unwrap();
    assert!(!mux.tab(tab).unwrap().layout().same_shape(&current));
    // That changes neither close's scope.
    mux.validate_close(&pane_assessment, &pane_assessment.recheck(), false)
        .unwrap();
    mux.commit_close(&tab_assessment, &tab_assessment.recheck(), false)
        .unwrap();
    assert!(second.client.read_snapshot().is_err());
    assert_eq!(mux.terminal_count(), 0);
}

/// Assesses `request` and commits it as an idle close needs no consent.
fn commit_idle(mux: &mut Mux, request: CloseRequest) -> Result<(), MuxError> {
    let assessment = mux.prepare_close(request)?.check_jobs();
    assert!(!assessment.needs_confirmation(), "{:?}", assessment.jobs());
    mux.commit_close(&assessment, &assessment.recheck(), false)
}

#[test]
fn a_workspace_close_covers_its_terminals_and_spares_its_siblings() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let kept = mux.create_workspace(session, None).unwrap();
    let closing = mux.create_workspace(session, None).unwrap();
    let survivor = open_idle_tabs(&mut mux, kept, 1).remove(0);
    let doomed = open_idle_tabs(&mut mux, closing, 2);
    let attachment = mux.attach_session(session).unwrap();

    let ticket = mux.prepare_close(CloseRequest::Workspace(closing)).unwrap();
    assert_eq!(ticket.effect(), CloseEffect::Workspace(closing));
    let assessment = ticket.check_jobs();
    assert_eq!(
        assessment.terminals(),
        doomed
            .iter()
            .map(|tab| tab.tab.first_pane().terminal_id)
            .collect::<Vec<_>>()
    );
    let pid = pid_of(&doomed[0].client);

    // A tab opened in the workspace after assessment widens its scope.
    let late = open_idle_tabs(&mut mux, closing, 1).remove(0);
    assert!(matches!(
        mux.commit_close(&assessment, &assessment.recheck(), false),
        Err(MuxError::StaleClose)
    ));
    assert_eq!(mux.workspace(closing).unwrap().tabs.len(), 3);
    let late_pid = pid_of(&late.client);

    commit_idle(&mut mux, CloseRequest::Workspace(closing)).unwrap();
    assert!(mux.workspace(closing).is_none());
    assert_eq!(mux.session(session).unwrap().workspaces, [kept]);
    assert_eq!(mux.attachment_session(attachment).unwrap(), session);
    assert!(nix::sys::signal::kill(pid, None).is_err());
    assert!(nix::sys::signal::kill(late_pid, None).is_err());
    // Only the sibling's shell is left, and it keeps running.
    assert_eq!(mux.terminal_count(), 1);
    assert!(nix::sys::signal::kill(pid_of(&survivor.client), None).is_ok());
    mux.shutdown().unwrap();
}

#[test]
fn the_last_workspace_of_a_session_is_refused_as_a_workspace_close() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let only = mux.create_workspace(session, None).unwrap();
    let before = mux.structure();
    assert!(matches!(
        mux.prepare_close(CloseRequest::Workspace(only)),
        Err(MuxError::LastWorkspace(refused)) if refused == only
    ));
    assert_eq!(mux.structure(), before);
    // A workspace that already went is unknown, not the last.
    let other = mux.create_workspace(session, None).unwrap();
    mux.close_workspace(other).unwrap();
    assert!(matches!(
        mux.prepare_close(CloseRequest::Workspace(other)),
        Err(MuxError::UnknownWorkspace(_))
    ));
}

#[test]
fn an_emptied_window_whose_sibling_holds_tabs_leaves_and_closes_its_workspace()
{
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let emptied = mux.create_workspace(session, None).unwrap();
    let populated = mux.create_workspace(session, None).unwrap();
    let sibling = open_idle_tabs(&mut mux, populated, 1).remove(0);
    let attachment = mux.attach_session(session).unwrap();
    let request = CloseRequest::EmptiedWindow {
        attachment,
        workspace: emptied,
    };
    let assessment = mux.prepare_close(request.clone()).unwrap().check_jobs();
    assert_eq!(
        assessment.effect(),
        &CloseEffect::LeaveWorkspace {
            attachment,
            workspace: emptied
        }
    );
    assert_eq!(assessment.terminals(), Vec::<TerminalId>::new());
    assert!(!assessment.needs_confirmation());
    commit_idle(&mut mux, request).unwrap();
    assert!(mux.workspace(emptied).is_none());
    assert_eq!(mux.session(session).unwrap().workspaces, [populated]);
    assert_eq!(mux.attachment_session(attachment).unwrap(), session);
    assert_eq!(mux.terminal_count(), 1);
    assert!(nix::sys::signal::kill(pid_of(&sibling.client), None).is_ok());
    mux.shutdown().unwrap();
}

#[test]
fn an_emptied_window_whose_workspace_left_its_session_leaves_it_alone() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let kept = mux.create_workspace(session, None).unwrap();
    let elsewhere = mux.create_session(None).unwrap();
    let attachment = mux.attach_session(session).unwrap();
    let emptied = |workspace| CloseRequest::EmptiedWindow {
        attachment,
        workspace,
    };
    let leaves = |workspace| CloseEffect::LeaveWorkspace {
        attachment,
        workspace,
    };

    // Closed elsewhere: the session keeps an empty workspace, so the
    // window leaves rather than closing its session.
    let closed = mux.create_workspace(session, None).unwrap();
    mux.close_workspace(closed).unwrap();
    let ticket = mux.prepare_close(emptied(closed)).unwrap();
    assert_eq!(ticket.effect(), leaves(closed));
    commit_idle(&mut mux, emptied(closed)).unwrap();
    assert_eq!(mux.session(session).unwrap().workspaces, [kept]);

    // Moved to another session, with a tab: left alone there.
    let moved = mux.create_workspace(session, None).unwrap();
    let tab = open_idle_tabs(&mut mux, moved, 1).remove(0);
    mux.move_workspace(session, moved, elsewhere, None).unwrap();
    assert_eq!(
        mux.prepare_close(emptied(moved)).unwrap().effect(),
        leaves(moved)
    );
    commit_idle(&mut mux, emptied(moved)).unwrap();
    assert_eq!(mux.workspace(moved).unwrap().session_id, elsewhere);
    assert!(nix::sys::signal::kill(pid_of(&tab.client), None).is_ok());

    // Moved away empty: kept in its new session.
    let empty = mux.create_workspace(session, None).unwrap();
    mux.move_workspace(session, empty, elsewhere, None).unwrap();
    commit_idle(&mut mux, emptied(empty)).unwrap();
    assert_eq!(mux.session(elsewhere).unwrap().workspaces, [moved, empty]);
    assert_eq!(mux.attachment_session(attachment).unwrap(), session);

    // Once the session keeps no workspace, today's rule applies.
    mux.close_workspace(kept).unwrap();
    assert_eq!(
        mux.prepare_close(emptied(kept)).unwrap().effect(),
        CloseEffect::Session(session)
    );
    mux.shutdown().unwrap();
}

/// How an emptied window's workspace lost its last tab.
#[derive(Clone, Copy, Debug)]
enum Emptying {
    TabClosed,
    LonePaneClosed,
    TabMovedAway,
}

/// Empties `workspace`, whose only tab is `tab`, as `emptying` names,
/// moving the tab to `destination` when it moves away.
fn empty_workspace(
    mux: &mut Mux,
    emptying: Emptying,
    workspace: WorkspaceId,
    tab: &crate::mux::TestTab,
    destination: WorkspaceId,
) {
    match emptying {
        Emptying::TabClosed => commit_idle(
            mux,
            CloseRequest::Tab {
                workspace,
                tab: tab.tab.id,
            },
        ),
        Emptying::LonePaneClosed => commit_idle(
            mux,
            CloseRequest::Pane {
                workspace,
                tab: tab.tab.id,
                pane: tab.tab.first_pane().id,
            },
        ),
        Emptying::TabMovedAway => {
            mux.move_tab(workspace, tab.tab.id, destination, None)
        }
    }
    .unwrap();
}

#[test]
fn an_emptied_window_never_ends_a_session_that_holds_another_workspace() {
    for emptying in [
        Emptying::TabClosed,
        Emptying::LonePaneClosed,
        Emptying::TabMovedAway,
    ] {
        for (with_sibling, shared) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let case = format!(
                "{emptying:?}, sibling {with_sibling}, shared {shared}"
            );
            let mut mux = Mux::default();
            let session = mux.create_session(None).unwrap();
            let shown = mux.create_workspace(session, None).unwrap();
            let sibling = with_sibling
                .then(|| mux.create_workspace(session, None).unwrap());
            let elsewhere = mux.create_session(None).unwrap();
            let destination = mux.create_workspace(elsewhere, None).unwrap();
            let tab = open_idle_tabs(&mut mux, shown, 1).remove(0);
            let attachment = mux.attach_session(session).unwrap();
            let other = shared.then(|| mux.attach_session(session).unwrap());
            empty_workspace(&mut mux, emptying, shown, &tab, destination);
            let emptied = |attachment| CloseRequest::EmptiedWindow {
                attachment,
                workspace: shown,
            };
            let expected = match (sibling, other) {
                (Some(_), _) => CloseEffect::LeaveWorkspace {
                    attachment,
                    workspace: shown,
                },
                (None, Some(_)) => CloseEffect::Detach(attachment),
                (None, None) => CloseEffect::Session(session),
            };
            assert_eq!(
                mux.prepare_close(emptied(attachment)).unwrap().effect(),
                expected,
                "{case}"
            );
            commit_idle(&mut mux, emptied(attachment)).unwrap();
            match (sibling, other) {
                (Some(sibling), other) => {
                    // The empty sibling survives, and the session with it.
                    assert_eq!(
                        mux.session(session).unwrap().workspaces,
                        [sibling],
                        "{case}"
                    );
                    assert_eq!(
                        mux.attachment_session(attachment).unwrap(),
                        session,
                        "{case}"
                    );
                    // A second window on the closed workspace leaves too.
                    if let Some(other) = other {
                        assert_eq!(
                            mux.prepare_close(emptied(other)).unwrap().effect(),
                            CloseEffect::LeaveWorkspace {
                                attachment: other,
                                workspace: shown
                            },
                            "{case}"
                        );
                        commit_idle(&mut mux, emptied(other)).unwrap();
                        assert_eq!(
                            mux.session(session).unwrap().workspaces,
                            [sibling],
                            "{case}"
                        );
                    }
                }
                (None, Some(other)) => {
                    assert_eq!(
                        mux.session(session).unwrap().workspaces,
                        [shown],
                        "{case}"
                    );
                    assert!(mux.attachment_session(attachment).is_err());
                    assert_eq!(
                        mux.attachment_session(other).unwrap(),
                        session,
                        "{case}"
                    );
                }
                (None, None) => {
                    assert!(mux.session(session).is_none(), "{case}");
                }
            }
            mux.shutdown().unwrap();
        }
    }
}

#[test]
fn held_emptied_tickets_go_stale_when_a_sibling_appears_or_goes() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let shown = mux.create_workspace(session, None).unwrap();
    let elsewhere = mux.create_session(None).unwrap();
    let attachment = mux.attach_session(session).unwrap();
    let request = CloseRequest::EmptiedWindow {
        attachment,
        workspace: shown,
    };
    let leaves = CloseEffect::LeaveWorkspace {
        attachment,
        workspace: shown,
    };
    let stale = |mux: &mut Mux, held: &CloseAssessment| {
        assert!(matches!(
            mux.validate_close_scope(held),
            Err(MuxError::StaleClose)
        ));
        assert!(matches!(
            mux.commit_close(held, &held.recheck(), false),
            Err(MuxError::StaleClose)
        ));
    };

    // A new empty sibling turns a held session close into leaving.
    let held = mux.prepare_close(request.clone()).unwrap().check_jobs();
    assert_eq!(held.effect(), &CloseEffect::Session(session));
    assert!(mux.validate_close_scope(&held).is_ok());
    let sibling = mux.create_workspace(session, None).unwrap();
    stale(&mut mux, &held);
    let held = mux.prepare_close(request.clone()).unwrap().check_jobs();
    assert_eq!(held.effect(), &leaves);

    // The sibling gaining and then losing its tab keeps it leaving.
    let tab = open_idle_tabs(&mut mux, sibling, 1).remove(0);
    stale(&mut mux, &held);
    let held = mux.prepare_close(request.clone()).unwrap().check_jobs();
    assert_eq!(held.effect(), &leaves);
    mux.close_tab(sibling, tab.tab.id).unwrap();
    stale(&mut mux, &held);
    let held = mux.prepare_close(request.clone()).unwrap().check_jobs();
    assert_eq!(held.effect(), &leaves);

    // The only sibling closing turns it back.
    mux.close_workspace(sibling).unwrap();
    stale(&mut mux, &held);
    assert_eq!(mux.workspace(shown).unwrap().tabs.len(), 0);
    assert_eq!(
        mux.prepare_close(request.clone()).unwrap().effect(),
        CloseEffect::Session(session)
    );

    // So does the only sibling moving to another session.
    let moved = mux.create_workspace(session, None).unwrap();
    let held = mux.prepare_close(request.clone()).unwrap().check_jobs();
    assert_eq!(held.effect(), &leaves);
    mux.move_workspace(session, moved, elsewhere, None).unwrap();
    stale(&mut mux, &held);
    assert_eq!(
        mux.prepare_close(request).unwrap().effect(),
        CloseEffect::Session(session)
    );
    // A foreign runtime's ticket is never fresh here.
    let mut foreign = Mux::default();
    let other = foreign.create_session(None).unwrap();
    let ticket = foreign
        .prepare_close(CloseRequest::Session(other))
        .unwrap()
        .check_jobs();
    assert!(matches!(
        mux.validate_close_scope(&ticket),
        Err(MuxError::StaleClose)
    ));
}

#[test]
fn an_emptied_pinned_workspace_stays_even_as_its_sessions_only_one() {
    for emptying in [
        Emptying::TabClosed,
        Emptying::LonePaneClosed,
        Emptying::TabMovedAway,
    ] {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let pinned = mux.create_workspace(session, None).unwrap();
        let elsewhere = mux.create_session(None).unwrap();
        let destination = mux.create_workspace(elsewhere, None).unwrap();
        let tab = open_idle_tabs(&mut mux, pinned, 1).remove(0);
        mux.set_workspace_pinned(pinned, true).unwrap();
        let attachment = mux.attach_session(session).unwrap();
        empty_workspace(&mut mux, emptying, pinned, &tab, destination);
        // Each way out leaves the tab's directory behind for a new tab.
        assert!(
            mux.workspace(pinned).unwrap().directory().is_some(),
            "{emptying:?}"
        );
        let request = CloseRequest::EmptiedWindow {
            attachment,
            workspace: pinned,
        };
        // The session's only workspace: neither the session nor the
        // window's attachment ends.
        let before = mux.structure();
        assert!(
            matches!(
                mux.prepare_close(request.clone()),
                Err(MuxError::PinnedWorkspace(refused)) if refused == pinned
            ),
            "{emptying:?}"
        );
        assert_eq!(mux.structure(), before);
        // Beside a sibling it still stays rather than closing.
        let sibling = mux.create_workspace(session, None).unwrap();
        assert!(matches!(
            mux.prepare_close(request),
            Err(MuxError::PinnedWorkspace(_))
        ));
        // An explicit Close Workspace still closes it.
        commit_idle(&mut mux, CloseRequest::Workspace(pinned)).unwrap();
        assert_eq!(mux.session(session).unwrap().workspaces, [sibling]);
        assert_eq!(mux.attachment_session(attachment).unwrap(), session);
        mux.shutdown().unwrap();
    }
}

#[test]
fn pinning_or_unpinning_turns_a_held_emptied_close_stale() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let shown = mux.create_workspace(session, None).unwrap();
    let attachment = mux.attach_session(session).unwrap();
    let request = CloseRequest::EmptiedWindow {
        attachment,
        workspace: shown,
    };
    let stale = |mux: &mut Mux, held: &CloseAssessment| {
        assert!(matches!(
            mux.validate_close_scope(held),
            Err(MuxError::StaleClose)
        ));
        assert!(matches!(
            mux.commit_close(held, &held.recheck(), false),
            Err(MuxError::StaleClose)
        ));
    };
    // A session close held while the user pins the workspace never commits.
    let held = mux.prepare_close(request.clone()).unwrap().check_jobs();
    assert_eq!(held.effect(), &CloseEffect::Session(session));
    mux.set_workspace_pinned(shown, true).unwrap();
    stale(&mut mux, &held);
    assert!(matches!(
        mux.prepare_close(request.clone()),
        Err(MuxError::PinnedWorkspace(_))
    ));
    // So does a leave held beside a sibling: the pinned workspace stays.
    mux.set_workspace_pinned(shown, false).unwrap();
    mux.create_workspace(session, None).unwrap();
    let held = mux.prepare_close(request.clone()).unwrap().check_jobs();
    assert_eq!(
        held.effect(),
        &CloseEffect::LeaveWorkspace {
            attachment,
            workspace: shown,
        }
    );
    mux.set_workspace_pinned(shown, true).unwrap();
    stale(&mut mux, &held);
    assert!(mux.workspace(shown).is_some());
    // Setting the same pin again changes nothing a ticket checks.
    mux.set_workspace_pinned(shown, false).unwrap();
    let held = mux.prepare_close(request).unwrap().check_jobs();
    mux.set_workspace_pinned(shown, false).unwrap();
    assert!(mux.validate_close_scope(&held).is_ok());
    mux.shutdown().unwrap();
}

/// Answers each line it reads with `GOT_<line>.`, without echo, so only
/// input that reached the shell shows.
const LINES: &str =
    "stty -echo; printf READY; while read v; do printf 'GOT_%s.' \"$v\"; done";

/// Holds `client`'s runtime owner, so input it is sent stays queued, until
/// the returned sender is dropped or sent on.
fn hold_runtime(
    client: &crate::test_support::TestClient,
) -> std::sync::mpsc::Sender<()> {
    let (entered, paused) = std::sync::mpsc::channel();
    let (resume, release) = std::sync::mpsc::channel();
    client
        .control
        .send_control(crate::terminal::RuntimeControl::Pause {
            entered,
            release,
        })
        .unwrap();
    paused.recv_timeout(Duration::from_secs(3)).unwrap();
    resume
}

/// A full-capability viewer of `terminal` that belongs to `attachment`.
fn attached_viewer(
    mux: &Mux,
    attachment: AttachmentId,
    terminal: TerminalId,
) -> crate::TerminalViewer {
    mux.subscribe_terminal(
        attachment,
        terminal,
        crate::ViewerOptions::new(huterm_protocol::ViewerCapabilities::ALL),
    )
    .unwrap()
}

#[test]
fn detaching_keeps_input_a_dropped_viewer_sent_before_it() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let opened = mux.open_test_tab(workspace, &command(LINES)).unwrap();
    wait_for_text(&opened.client, "READY");
    let attachment = mux.attach_session(session).unwrap();
    let viewer =
        attached_viewer(&mux, attachment, opened.tab.first_pane().terminal_id);
    let resume = hold_runtime(&opened.client);
    viewer
        .send_input(TerminalInput::Text("kept\n".into()))
        .unwrap();
    drop(viewer);
    mux.detach_session(attachment).unwrap();
    resume.send(()).unwrap();
    wait_for_text(&opened.client, "GOT_kept.");
    mux.shutdown().unwrap();
}

#[test]
fn detaching_a_live_viewer_discards_its_queued_input() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let opened = mux.open_test_tab(workspace, &command(LINES)).unwrap();
    wait_for_text(&opened.client, "READY");
    let attachment = mux.attach_session(session).unwrap();
    let viewer =
        attached_viewer(&mux, attachment, opened.tab.first_pane().terminal_id);
    let resume = hold_runtime(&opened.client);
    viewer
        .send_input(TerminalInput::Text("lost\n".into()))
        .unwrap();
    mux.detach_session(attachment).unwrap();
    assert!(viewer.poll().revoked);
    assert!(matches!(
        viewer.send_input(TerminalInput::Text("late\n".into())),
        Err(crate::RuntimeError::Revoked)
    ));
    resume.send(()).unwrap();
    // The discarded line was queued first, so it would show before this.
    opened
        .client
        .send_input(TerminalInput::Text("after\n".into()))
        .unwrap();
    let screen = crate::test_support::snapshot_text(&wait_for_text(
        &opened.client,
        "GOT_after.",
    ));
    assert!(!screen.contains("GOT_lost."), "{screen}");
    mux.shutdown().unwrap();
}

#[test]
fn closing_a_window_after_its_viewer_dropped_keeps_the_viewer_s_input() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let opened = mux.open_test_tab(workspace, &command(LINES)).unwrap();
    wait_for_text(&opened.client, "READY");
    let closing = mux.attach_session(session).unwrap();
    // Another window still shows the session, so closing this one only
    // detaches it.
    mux.attach_session(session).unwrap();
    let viewer =
        attached_viewer(&mux, closing, opened.tab.first_pane().terminal_id);
    let resume = hold_runtime(&opened.client);
    viewer
        .send_input(TerminalInput::Text("kept\n".into()))
        .unwrap();
    drop(viewer);
    let assessment = mux
        .prepare_close(CloseRequest::Window(closing))
        .unwrap()
        .check_jobs();
    assert_eq!(assessment.effect(), &CloseEffect::Detach(closing));
    mux.commit_close(&assessment, &assessment.recheck(), false)
        .unwrap();
    resume.send(()).unwrap();
    wait_for_text(&opened.client, "GOT_kept.");
    mux.shutdown().unwrap();
}
