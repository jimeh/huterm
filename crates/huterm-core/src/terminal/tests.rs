use std::cell::RefCell;
use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use super::*;

#[test]
fn character_queue_bytes_include_utf8_payload_and_meta_prefix() {
    for (text, meta, expected) in
        [("λ", true, 3), ("λ", false, 2), ("", true, 0)]
    {
        assert_eq!(
            input_bytes(&TerminalInput::Character {
                text: text.into(),
                meta
            }),
            expected
        );
    }
}

#[test]
fn ghostty_meta_characters_preserve_exact_bytes_and_input_order() {
    meta_roundtrip();
}

fn meta_roundtrip() {
    use std::fmt::Write as _;
    let expected = "\x1br\x1bR\x1b3\x1b<\x1b\x12\x1b \x1bλ®paste".as_bytes();
    let script = format!(
        "stty raw -echo; printf READY; bytes=$(dd bs=1 count={} 2>/dev/null | od -An -tx1 | tr -d ' \\n'); printf 'HEX:%s:DONE' \"$bytes\"",
        expected.len(),
    );
    let command = command(&script);
    let runtime =
        TerminalRuntime::spawn(TerminalId::new(94), &command).unwrap();
    let client = runtime.client();
    wait_for_text(&client, "READY");
    for text in ["r", "R", "3", "<", "\x12", " ", "λ"] {
        client
            .send_input(TerminalInput::Character {
                text: text.into(),
                meta: true,
            })
            .unwrap();
    }
    client.send_input(TerminalInput::Text("®".into())).unwrap();
    client
        .send_input(TerminalInput::Paste("paste".into()))
        .unwrap();
    let snapshot = wait_for_text(&client, ":DONE");
    let text: String =
        snapshot.cells().map(|cell| cell.text.as_str()).collect();
    let mut hex = String::new();
    for byte in expected {
        write!(hex, "{byte:02x}").unwrap();
    }
    assert!(text.contains(&format!("HEX:{hex}:DONE")), "{text:?}");
    assert_eq!(wait_for_exit(&client).code, Some(0));
    runtime.shutdown().unwrap();
}

#[test]
fn concurrent_clean_exits_reap_without_close_assessments() {
    let runtimes: Vec<_> = (100..104)
        .map(|id| {
            let command = command("printf READY; read line; exit 0");
            TerminalRuntime::spawn(TerminalId::new(id), &command).unwrap()
        })
        .collect();
    let clients: Vec<_> =
        runtimes.iter().map(TerminalRuntime::client).collect();
    let roots: Vec<_> = clients
        .iter()
        .map(|client| {
            wait_for_text(client, "READY");
            nix::unistd::Pid::from_raw(
                i32::try_from(client.job_context().unwrap().shell.unwrap())
                    .unwrap(),
            )
        })
        .collect();
    for client in &clients {
        client.send_input(TerminalInput::Text("\n".into())).unwrap();
    }
    for client in &clients {
        assert!(wait_for_exit(client).success);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while roots
        .iter()
        .any(|root| nix::sys::signal::kill(*root, None).is_ok())
    {
        assert!(
            Instant::now() < deadline,
            "concurrent idle roots were not reaped"
        );
        thread::sleep(Duration::from_millis(5));
    }
    for runtime in runtimes {
        runtime.shutdown().unwrap();
    }
}

#[test]
fn directory_metadata_replaces_state_orders_revisions_and_suppresses_duplicates()
 {
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(92),
        &command("printf '\\033]7;file://localhost/tmp/one\\007\\033]7;file://localhost/tmp/one\\007\\033]7;file://localhost/tmp/two\\007READY'; read line"),
    )
    .unwrap();
    let client = runtime.client();
    wait_for_text(&client, "READY");
    let mut changes = Vec::new();
    while let Some(event) = client.try_recv_event().unwrap() {
        if let TerminalEvent::MetadataChanged {
            revision, metadata, ..
        } = event
        {
            changes.push((
                revision,
                metadata
                    .directory()
                    .map(|directory| directory.path().to_owned()),
            ));
        }
    }
    // READY is parsed after all OSCs. Only the latest full replacement is
    // pending, and the duplicate OSC did not advance its revision.
    assert_eq!(changes, [(2, Some("/tmp/two".to_owned()))]);
    runtime.shutdown().unwrap();
}

/// Waits until the published name matches. `current` carries the last
/// published name between calls, so waiting for `None` needs a clear.
fn wait_for_foreground(
    client: &RuntimeClient,
    current: &mut Option<String>,
    expected: Option<&str>,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        while let Some(event) = client.try_recv_event().unwrap() {
            if let TerminalEvent::MetadataChanged { metadata, .. } = event {
                *current = metadata.foreground_process().map(str::to_owned);
            }
        }
        if current.as_deref() == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "foreground process was {current:?}, expected {expected:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

/// Waits until the published directory matches. `current` carries the
/// last published directory between calls.
fn wait_for_directory(
    client: &RuntimeClient,
    current: &mut Option<huterm_protocol::TerminalDirectory>,
    expected: impl Fn(&huterm_protocol::TerminalDirectory) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        while let Some(event) = client.try_recv_event().unwrap() {
            if let TerminalEvent::MetadataChanged { metadata, .. } = event {
                *current = metadata.directory().cloned();
            }
        }
        if current.as_ref().is_some_and(&expected) {
            return;
        }
        assert!(Instant::now() < deadline, "directory was {current:?}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn same_path(
    expected: &std::path::Path,
) -> impl Fn(&huterm_protocol::TerminalDirectory) -> bool {
    let expected = expected.canonicalize().unwrap();
    move |directory| {
        directory.is_local()
            && std::path::Path::new(directory.path())
                .canonicalize()
                .is_ok_and(|path| path == expected)
    }
}

#[test]
fn process_directories_follow_cd_and_running_jobs_without_osc7() {
    let job_directory = std::env::temp_dir()
        .join(format!("huterm-job-cwd-{}", std::process::id()));
    std::fs::create_dir_all(&job_directory).unwrap();
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(98),
        &command(&format!(
            "set -m; printf READY; read line; cd /; printf MOVED; read line; (cd '{}' && exec head -n 1 >/dev/null); printf DONE; read line",
            job_directory.display()
        )),
    )
    .unwrap();
    let client = runtime.client();
    let mut current = None;
    wait_for_text(&client, "READY");
    client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    wait_for_directory(
        &client,
        &mut current,
        same_path(std::path::Path::new("/")),
    );
    client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    wait_for_directory(&client, &mut current, same_path(&job_directory));
    client
        .send_input(TerminalInput::Text("stop\n".into()))
        .unwrap();
    wait_for_text(&client, "DONE");
    wait_for_directory(
        &client,
        &mut current,
        same_path(std::path::Path::new("/")),
    );
    runtime.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(job_directory);
}

#[test]
fn osc7_reports_apply_only_while_their_reporter_holds_the_foreground() {
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(99),
        &command(
            &format!(
                "set -m; printf '\\033]7;file://localhost/reported\\007READY'; read line; {}; printf DONE; read line",
                foreground_reporter("printf '\\033]7;file://remote.example/srv\\007'")
            ),
        ),
    )
    .unwrap();
    let client = runtime.client();
    let mut current = None;
    wait_for_text(&client, "READY");
    // The shell's report wins over its actual directory at the prompt.
    wait_for_directory(&client, &mut current, |directory| {
        directory.path() == "/reported"
    });
    client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    wait_for_directory(&client, &mut current, |directory| {
        directory.path() == "/srv" && !directory.is_local()
    });
    client
        .send_input(TerminalInput::Text("stop\n".into()))
        .unwrap();
    wait_for_text(&client, "DONE");
    // The job's report expired with it; the shell's process directory
    // applies until the shell reports again.
    wait_for_directory(
        &client,
        &mut current,
        same_path(&std::env::current_dir().unwrap()),
    );
    runtime.shutdown().unwrap();
}

#[test]
fn job_titles_apply_only_while_the_job_holds_the_foreground() {
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(100),
        &command(
            &format!(
                "set -m; printf '\\033]2;shell title\\007READY'; read line; {}; printf DONE; read line",
                foreground_reporter("printf '\\033]2;job title\\007'")
            ),
        ),
    )
    .unwrap();
    let client = runtime.client();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut title: Option<String> = None;
    let mut wait_for = |expected: Option<&str>| loop {
        while let Some(event) = client.try_recv_event().unwrap() {
            if let TerminalEvent::MetadataChanged { metadata, .. } = event {
                title = metadata.foreground_title().map(str::to_owned);
            }
        }
        if title.as_deref() == expected {
            return;
        }
        assert!(Instant::now() < deadline, "title was {title:?}");
        thread::sleep(Duration::from_millis(10));
    };
    wait_for(Some("shell title"));
    client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    wait_for(Some("job title"));
    client
        .send_input(TerminalInput::Text("stop\n".into()))
        .unwrap();
    // The job's title leaves with it, even though the terminal title
    // still reads "job title".
    wait_for(None);
    runtime.shutdown().unwrap();
}

#[test]
fn foreground_jobs_are_named_while_running_and_cleared_on_return() {
    // Job control gives the quiet `head` its own foreground group. It
    // exits normally after one line; dash ends a script on SIGINT.
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(96),
        &command(
            "set -m; printf READY; read line; head -n 1 >/dev/null; printf DONE; read line",
        ),
    )
    .unwrap();
    let client = runtime.client();
    let mut current = None;
    wait_for_text(&client, "READY");
    client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    wait_for_foreground(&client, &mut current, Some("head"));
    client
        .send_input(TerminalInput::Text("stop\n".into()))
        .unwrap();
    wait_for_text(&client, "DONE");
    wait_for_foreground(&client, &mut current, None);
    runtime.shutdown().unwrap();
}

#[test]
fn a_root_shell_replaced_by_exec_is_named_after_silent_input() {
    // Without echo, only the input trigger can notice the exec.
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(97),
        &command("stty -echo; printf READY; read line; exec sleep 30"),
    )
    .unwrap();
    let client = runtime.client();
    wait_for_text(&client, "READY");
    // Let the probe armed by READY's output run, so only input remains.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (reply, armed) = mpsc::channel();
        client
            .controls
            .send(RuntimeControl::ProbeArmed(reply))
            .unwrap();
        if !armed.recv_timeout(Duration::from_secs(2)).unwrap() {
            break;
        }
        assert!(Instant::now() < deadline, "probe stayed armed");
        thread::sleep(Duration::from_millis(10));
    }
    client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    wait_for_foreground(&client, &mut None, Some("sleep"));
    runtime.shutdown().unwrap();
}

#[test]
fn directory_metadata_survives_exit_in_its_replacement_event() {
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(95),
        &command("printf '\\033]7;file://localhost/tmp/final\\007'; exit"),
    )
    .unwrap();
    let client = runtime.client();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut metadata = None;
    let mut exited = false;
    while Instant::now() < deadline && (!exited || metadata.is_none()) {
        match client.try_recv_event().unwrap() {
            Some(TerminalEvent::MetadataChanged {
                metadata: replacement,
                ..
            }) => metadata = Some(replacement),
            Some(TerminalEvent::Exited { .. }) => exited = true,
            _ => thread::yield_now(),
        }
    }
    assert!(exited);
    assert_eq!(
        metadata
            .as_ref()
            .and_then(TerminalMetadata::directory)
            .map(huterm_protocol::TerminalDirectory::path),
        Some("/tmp/final")
    );
    runtime.shutdown().unwrap();
}

#[test]
fn clean_exit_reaps_automatically_while_retained_history_stays_available() {
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(93),
        &command("printf FINAL; read line; exit 7"),
    )
    .unwrap();
    let client = runtime.client();
    wait_for_text(&client, "FINAL");
    let root = client.job_context().unwrap().shell.unwrap();
    let root = nix::unistd::Pid::from_raw(i32::try_from(root).unwrap());
    client.send_input(TerminalInput::Text("\n".into())).unwrap();
    assert_eq!(
        wait_for_exit(&client),
        ExitStatus {
            code: Some(7),
            success: false
        }
    );
    // Root exit reaps immediately, without an assessment or close request.
    let deadline = Instant::now() + Duration::from_secs(5);
    while nix::sys::signal::kill(root, None).is_ok() {
        assert!(
            Instant::now() < deadline,
            "idle history retained a zombie root"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let context = client.job_context().unwrap();
    assert_eq!(
        context
            .lifecycle
            .assess(|| panic!("exited history inspected live PID")),
        crate::jobs::JobState::Idle
    );
    wait_for_text(&client, "FINAL");
    runtime.shutdown().unwrap();
    assert_eq!(
        context.lifecycle.assess(|| panic!()),
        crate::jobs::JobState::Unknown
    );
}

#[test]
fn lookup_only_failure_preserves_snapshot_runtime_pty_and_exited_history() {
    {
        let command = command(
            "stty -echo; printf 'https://x.test READY'; read line; printf ' ACK:%s' \"$line\"",
        );
        let runtime =
            TerminalRuntime::spawn(TerminalId::new(99), &command).unwrap();
        let client = runtime.client();
        wait_for_text(&client, "READY");
        let (reply, receiver) = async_channel::bounded(1);
        client
            .controls
            .send(RuntimeControl::Snapshot {
                scroll: None,
                point: Some(huterm_protocol::MousePosition::default()),
                fail_lookup: true,
                reply,
            })
            .unwrap();
        let failed = SnapshotRequest { receiver }.recv_blocking().unwrap();
        assert_eq!(failed.link, Some(huterm_protocol::LinkLookup::Unavailable));
        assert!(!client.closing.load(Ordering::Acquire));
        assert!(
            failed
                .snapshot
                .cells()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .contains("READY")
        );
        let valid = client
            .request_snapshot_with_link(
                None,
                Some(huterm_protocol::MousePosition::default()),
            )
            .unwrap()
            .recv_blocking()
            .unwrap();
        assert!(matches!(
            valid.link,
            Some(huterm_protocol::LinkLookup::Match(_))
        ));
        client
            .send_input(TerminalInput::Text("alive\n".into()))
            .unwrap();
        wait_for_text(&client, "ACK:alive");
        assert!(wait_for_exit(&client).success);
        let history = client
            .request_snapshot_with_link(
                None,
                Some(huterm_protocol::MousePosition::default()),
            )
            .unwrap()
            .recv_blocking()
            .unwrap();
        assert!(matches!(
            history.link,
            Some(huterm_protocol::LinkLookup::Match(_))
        ));
        while let Some(event) = client.try_recv_event().unwrap() {
            assert!(!matches!(event, TerminalEvent::Failed { .. }));
        }
        runtime.shutdown().unwrap();
    }
}

#[test]
fn snapshot_failure_reports_error_and_enters_runtime_cleanup() {
    let (reply, receiver) = async_channel::bounded(1);
    let (events, event_receiver, _activity) = EventPublisher::channel();
    let closing = AtomicBool::new(false);
    let id = TerminalId::new(96);
    complete_snapshot_request(
        Err(RuntimeError::Engine("snapshot allocation failed".into())),
        &reply,
        &events,
        id,
        &closing,
    );
    assert!(closing.load(Ordering::Acquire));
    assert!(
        matches!(receiver.try_recv(), Ok(Err(RuntimeError::Engine(message))) if message == "snapshot allocation failed")
    );
    assert!(
        matches!(event_receiver.try_recv(), Ok(TerminalEvent::Failed { terminal_id, message }) if terminal_id == id && message.contains("snapshot allocation failed"))
    );
}

#[test]
fn writer_failure_with_live_root_remains_observable_and_stops_runtime() {
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(92),
        &command("printf READY; read line"),
    )
    .unwrap();
    let client = runtime.client();
    wait_for_text(&client, "READY");
    client
        .controls
        .send(RuntimeControl::WriterFailed("test write failure".into()))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(TerminalEvent::Failed { message, .. }) =
            client.try_recv_event().unwrap()
        {
            assert_eq!(message, "test write failure");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "live-root write failure disappeared"
        );
        thread::sleep(Duration::from_millis(2));
    }
    runtime.shutdown().unwrap();
}

#[test]
fn ghostty_exited_runtime_discards_late_input_and_replies_but_keeps_history() {
    exited_history();
}

fn exited_history() {
    let command = command(
        "i=0; while [ $i -lt 40 ]; do printf 'history\\n'; i=$((i + 1)); done; printf '\\033[?1004hFINAL'; exit 0",
    );
    let runtime =
        TerminalRuntime::spawn(TerminalId::new(91), &command).unwrap();
    let client = runtime.client();
    wait_for_exit(&client);
    wait_for_text(&client, "FINAL");
    for input in [
        TerminalInput::Text("LATE".into()),
        TerminalInput::Paste("PASTE".into()),
        TerminalInput::Focus(true),
    ] {
        client.send_input(input).unwrap();
    }
    // Deliver an already-in-flight writer failure and final output that
    // asks for a terminal reply after the root has exited.
    client
        .controls
        .send(RuntimeControl::WriterFailed(
            "late revoked PTY write".into(),
        ))
        .unwrap();
    client.output.send(b"TAIL\x1b[6n".to_vec()).unwrap();
    let snapshot = wait_for_text(&client, "FINALTAIL");
    assert!(snapshot.history_size > 0);
    let history = client
        .request_scrolled_snapshot(ScrollCommand::Absolute(
            snapshot.history_size,
        ))
        .unwrap()
        .recv_blocking()
        .unwrap()
        .snapshot;
    assert!(history.viewport.bottom_offset > 0);
    assert!(
        history
            .cells()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .contains("history")
    );
    let selection = client
        .request_selection(
            snapshot.generation,
            BufferRange {
                start: huterm_protocol::BufferPoint {
                    rows_from_live_bottom: 0,
                    column: 0,
                },
                end: huterm_protocol::BufferPoint {
                    rows_from_live_bottom: 0,
                    column: 8,
                },
            },
        )
        .unwrap();
    assert_eq!(
        selection
            .receiver
            .recv_blocking()
            .unwrap()
            .unwrap()
            .as_deref(),
        Some("FINALTAIL")
    );
    client
        .resize(
            GridSize::clamped(90, 30),
            CellSize {
                width: 8,
                height: 16,
            },
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let snapshot = client.read_snapshot().unwrap();
        if snapshot.size == GridSize::clamped(90, 30) {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    while let Some(event) = client.try_recv_event().unwrap() {
        assert!(!matches!(event, TerminalEvent::Failed { .. }), "{event:?}");
    }
    assert_eq!(client.queued_input_bytes.load(Ordering::Acquire), 0);
    runtime.shutdown().unwrap();
}

#[test]
fn ghostty_runtime_round_trips_and_closes_a_live_child() {
    let command = command(
        "printf READY; read line; printf 'GHOSTTY:%s' \"$line\"; sleep 30",
    );
    let runtime =
        TerminalRuntime::spawn(TerminalId::new(92), &command).unwrap();
    let client = runtime.client();
    wait_for_text(&client, "READY");
    client
        .send_input(TerminalInput::Text("hello\n".into()))
        .unwrap();
    wait_for_text(&client, "GHOSTTY:hello");
    let started = Instant::now();
    runtime.shutdown().unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn ghostty_live_child_queries_seeded_presentation_before_first_snapshot() {
    let command = command(
        "stty raw -echo; printf '\\033]10;?\\033\\\\\\033[16t'; bytes=$(dd bs=1 count=34 2>/dev/null | od -An -tx1 | tr -d ' \\n'); printf 'QUERY:%s:DONE' \"$bytes\"",
    );
    let runtime =
        TerminalRuntime::spawn(TerminalId::new(95), &command).unwrap();
    let client = runtime.client();
    assert_eq!(wait_for_exit(&client).code, Some(0));
    let snapshot = wait_for_text(&client, ":DONE");
    let text: String =
        snapshot.cells().map(|cell| cell.text.as_str()).collect();
    assert!(
        text.contains(concat!(
            "QUERY:1b5d31303b7267623a653565352f653565352f653565351b5c",
            "1b5b363b31363b3874:DONE"
        )),
        "{text:?}"
    );
    runtime.shutdown().unwrap();
}

#[test]
fn rejected_engine_initialization_does_not_launch_the_child() {
    let marker = std::env::temp_dir()
        .join(format!("huterm-engine-startup-{}", std::process::id()));
    let mut command = command(&format!("touch {}; sleep 30", marker.display()));
    // Zero dimensions fail native initialization before the child starts.
    command.grid_size = GridSize {
        columns: 0,
        rows: 0,
    };
    let error =
        TerminalRuntime::spawn(TerminalId::new(93), &command).unwrap_err();
    assert!(matches!(error, RuntimeError::Engine(_)));
    assert!(!marker.exists());
}

#[test]
fn shared_scroll_snapshot_is_observed_by_a_second_client() {
    let runtime = TerminalRuntime::spawn(TerminalId::new(94), &command("i=0; while [ $i -lt 30 ]; do printf 'ROW-%s\\n' $i; i=$((i+1)); done; printf READY; read line")).unwrap();
    let client = runtime.client();
    let sibling = runtime.client();
    wait_for_text(&client, "READY");
    let scrolled = client
        .request_scrolled_snapshot(ScrollCommand::Absolute(5))
        .unwrap()
        .recv_blocking()
        .unwrap()
        .snapshot;
    assert_eq!(scrolled.viewport.bottom_offset, 5);
    assert_eq!(sibling.read_snapshot().unwrap().viewport, scrolled.viewport);
    let latest = client
        .request_scrolled_snapshot(ScrollCommand::Live)
        .unwrap()
        .recv_blocking()
        .unwrap()
        .snapshot;
    assert_eq!(latest.viewport.bottom_offset, 0);
    runtime.shutdown().unwrap();
}

#[test]
fn ghostty_mouse_reports_preserve_keyboard_order_and_disable_silence() {
    mouse_reports_roundtrip();
}

fn mouse_reports_roundtrip() {
    use huterm_protocol::{
        Modifiers, MouseAction, MouseButton, MouseInput, MousePosition,
        MouseTracking,
    };
    let expected = b"\x1b[<0;2;3MK\x1b[<0;2;3m";
    let script = format!(
        "stty raw -echo; printf '\\033[?1000h\\033[?1006hREADY'; bytes=$(dd bs=1 count={} 2>/dev/null | od -An -tx1 | tr -d ' \\n'); printf 'HEX:%s:DONE\\033[?1000lDISABLED' \"$bytes\"; byte=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\n'); printf 'SILENT:%s:END' \"$byte\"",
        expected.len(),
    );
    let command = command(&script);
    let runtime =
        TerminalRuntime::spawn(TerminalId::new(91), &command).unwrap();
    let client = runtime.client();
    let ready = wait_for_text(&client, "READY");
    assert_eq!(ready.modes.mouse_tracking, MouseTracking::Buttons);
    let mouse = |action| {
        TerminalInput::Mouse(MouseInput {
            action,
            position: MousePosition { column: 1, row: 2 },
            modifiers: Modifiers::default(),
        })
    };
    client
        .send_input(mouse(MouseAction::Press(MouseButton::Left)))
        .unwrap();
    client.send_input(TerminalInput::Text("K".into())).unwrap();
    client
        .send_input(mouse(MouseAction::Release(MouseButton::Left)))
        .unwrap();
    let disabled = wait_for_text(&client, "DISABLED");
    assert_eq!(disabled.modes.mouse_tracking, MouseTracking::Disabled);
    let hex = "1b5b3c303b323b334d4b1b5b3c303b323b336d";
    let text: String =
        disabled.cells().map(|cell| cell.text.as_str()).collect();
    assert!(text.contains(&format!("HEX:{hex}:DONE")), "{text:?}");
    client
        .send_input(mouse(MouseAction::Press(MouseButton::Left)))
        .unwrap();
    client.send_input(TerminalInput::Text("Z".into())).unwrap();
    wait_for_text(&client, "SILENT:5a:END");
    assert_eq!(wait_for_exit(&client).code, Some(0));
    runtime.shutdown().unwrap();
}

#[test]
fn blocking_snapshot_receive_times_out_and_reports_disconnect() {
    let (_sender, receiver) = async_channel::bounded(1);
    let request = SnapshotRequest { receiver };
    assert!(matches!(
        request.recv_blocking_with_timeout(Duration::from_millis(1)),
        Err(RuntimeError::TimedOut)
    ));

    let (sender, receiver) = async_channel::bounded(1);
    drop(sender);
    let request = SnapshotRequest { receiver };
    assert!(matches!(
        request.recv_blocking_with_timeout(Duration::from_secs(1)),
        Err(RuntimeError::Stopped)
    ));
}

fn foreground_job(client: &RuntimeClient) -> bool {
    let mut future = std::pin::pin!(client.has_foreground_job());
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let std::task::Poll::Ready(result) =
            std::future::Future::poll(future.as_mut(), &mut context)
        {
            return result.unwrap();
        }
        assert!(Instant::now() < deadline, "foreground query timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn close_confirmation_distinguishes_idle_shell_foreground_job_and_exit() {
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(31),
        &command("printf IDLE; read value; set -m; sleep 30 & fg; printf DONE"),
    )
    .unwrap();
    let client = runtime.client();
    wait_for_text(&client, "IDLE");
    assert!(!foreground_job(&client), "shell waiting for input is idle");
    client
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !foreground_job(&client) {
        assert!(Instant::now() < deadline, "foreground job not detected");
        thread::sleep(Duration::from_millis(10));
    }
    client
        .send_input(TerminalInput::Text("\x03".into()))
        .unwrap();
    wait_for_exit(&client);
    assert!(
        !foreground_job(&client),
        "exited child must not need confirmation"
    );
    runtime.shutdown().unwrap();
}

/// A job that runs `report` once it holds the foreground, then waits for
/// one line. The shell may hand over the terminal after the job starts:
/// macOS `/bin/sh` (bash 3.2) does, so a report sent sooner is credited to
/// the shell's group. Under heavy load that shell can also leave the job
/// in the shell's group while the terminal names the job's PID as its
/// foreground group; the job then never reports and the test times out.
fn foreground_reporter(report: &str) -> String {
    format!(
        "sh -c \"until [ \\$(ps -o tpgid= -p \\$\\$) = \\$(ps -o pgid= -p \\$\\$) ]; do sleep 0.01; done; {report}; exec head -n 1 >/dev/null\""
    )
}

/// Returns scripted read results, then blocks.
struct ScriptedReader(VecDeque<io::Result<Vec<u8>>>);

impl Read for ScriptedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.0.pop_front() {
            // Like a PTY, a short buffer takes part of the pending
            // output and leaves the rest for the next read.
            Some(Ok(mut bytes)) => {
                let count = bytes.len().min(buffer.len());
                buffer[..count].copy_from_slice(&bytes[..count]);
                let rest = bytes.split_off(count);
                if !rest.is_empty() {
                    self.0.push_front(Ok(rest));
                }
                Ok(count)
            }
            Some(Err(error)) => Err(error),
            None => Err(io::ErrorKind::WouldBlock.into()),
        }
    }
}

fn read_batch(results: Vec<io::Result<Vec<u8>>>) -> (Vec<u8>, ReadyEnd, usize) {
    let mut reader = ScriptedReader(results.into());
    let mut buffer = [0_u8; 8192];
    let mut batch = b"first".to_vec();
    let end = read_ready(&mut reader, &mut buffer, &mut batch);
    let left = reader
        .0
        .iter()
        .map(|result| result.as_ref().map_or(0, Vec::len))
        .sum();
    (batch, end, left)
}

#[test]
fn ready_reads_join_one_batch_until_the_pty_would_block() {
    let (batch, end, left) =
        read_batch(vec![Ok(b" second".to_vec()), Ok(b" third".to_vec())]);
    assert_eq!(batch, b"first second third");
    assert!(matches!(end, ReadyEnd::Drained));
    assert_eq!(left, 0);
}

#[test]
fn ready_reads_stop_before_the_batch_limit_and_keep_later_output() {
    let chunk = vec![b'x'; 8192];
    let reads = READ_BATCH_BYTES / chunk.len() + 2;
    let (batch, end, left) =
        read_batch((0..reads).map(|_| Ok(chunk.clone())).collect());
    assert!(matches!(end, ReadyEnd::Full));
    assert_eq!(batch.len(), READ_BATCH_BYTES);
    assert_eq!(batch.len() + left, 5 + reads * chunk.len());
}

#[test]
fn short_ready_reads_fill_the_batch_to_its_limit() {
    // macOS PTY reads return at most 1 KiB.
    let chunk = vec![b'x'; 1024];
    let reads = READ_BATCH_BYTES / chunk.len() + 2;
    let (batch, end, left) =
        read_batch((0..reads).map(|_| Ok(chunk.clone())).collect());
    assert!(matches!(end, ReadyEnd::Full));
    assert_eq!(batch.len(), READ_BATCH_BYTES);
    assert_eq!(batch.len() + left, 5 + reads * chunk.len());
}

#[test]
fn end_of_file_and_read_failures_follow_the_output_read_before_them() {
    let (batch, end, _) =
        read_batch(vec![Ok(b" tail".to_vec()), Ok(Vec::new())]);
    assert_eq!(batch, b"first tail");
    assert!(matches!(end, ReadyEnd::Ended(RuntimeControl::PtyEof)));

    let (batch, end, _) =
        read_batch(vec![Ok(b" tail".to_vec()), Err(io::Error::other("gone"))]);
    assert_eq!(batch, b"first tail");
    assert!(matches!(
        end,
        ReadyEnd::Ended(RuntimeControl::WorkerFailed(message))
            if message.contains("gone")
    ));
}

/// Records each read's outcome and each readiness wait in order.
struct LoggedReader {
    script: ScriptedReader,
    log: Rc<RefCell<Vec<&'static str>>>,
}

impl Read for LoggedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let result = self.script.read(buffer);
        self.log.borrow_mut().push(match &result {
            Ok(0) => "eof",
            Ok(_) => "read",
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                "would-block"
            }
            Err(_) => "error",
        });
        result
    }
}

/// Runs the reader loop over `results` with a readiness wait that
/// returns `wait`, collecting forwarded output and controls. The script
/// blocks forever once exhausted, so the wait fails after a few calls:
/// a loop that stops handling end-of-file or errors then ends with a
/// budget failure instead of hanging the test run.
fn forward(
    results: Vec<io::Result<Vec<u8>>>,
    wait: fn() -> io::Result<()>,
) -> (Vec<&'static str>, Vec<Vec<u8>>, Vec<RuntimeControl>) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut reader = LoggedReader {
        script: ScriptedReader(results.into()),
        log: Rc::clone(&log),
    };
    let wake = Arc::new(crate::wake::Wake::default());
    let (output, outputs) = mpsc::sync_channel(OUTPUT_CAPACITY);
    let (controls, received) = mpsc::channel();
    let wait_log = Rc::clone(&log);
    let mut waits = 0;
    forward_output(
        &mut reader,
        || {
            wait_log.borrow_mut().push("wait");
            waits += 1;
            if waits > 4 {
                return Err(io::Error::other("wait budget exhausted"));
            }
            wait()
        },
        &crate::wake::SyncSender::new(output, Arc::clone(&wake)),
        &crate::wake::Sender::new(controls, wake),
        &AtomicBool::new(false),
    );
    let log = log.borrow().clone();
    let received: Vec<_> = received.try_iter().collect();
    assert!(
        !received.iter().any(|control| matches!(
            control,
            RuntimeControl::WorkerFailed(message)
                if message.contains("wait budget exhausted")
        )),
        "reader loop did not stop: {log:?}"
    );
    (log, outputs.try_iter().collect(), received)
}

#[test]
fn reader_waits_after_a_drained_batch_and_keeps_output_before_end_of_file() {
    let (log, outputs, controls) = forward(
        vec![
            Ok(b"a".to_vec()),
            Ok(b"b".to_vec()),
            Err(io::ErrorKind::WouldBlock.into()),
            Ok(b"c".to_vec()),
            Ok(Vec::new()),
        ],
        || Ok(()),
    );
    // No read is retried between a would-block result and the wait.
    assert_eq!(log, ["read", "read", "would-block", "wait", "read", "eof"]);
    assert_eq!(outputs, [b"ab".to_vec(), b"c".to_vec()]);
    assert!(matches!(controls[..], [RuntimeControl::PtyEof]));
}

#[test]
fn reader_failures_keep_the_output_read_before_them() {
    let (_, outputs, controls) = forward(
        vec![Ok(b"a".to_vec()), Err(io::Error::other("gone"))],
        || Ok(()),
    );
    assert_eq!(outputs, [b"a".to_vec()]);
    assert!(matches!(
        &controls[..],
        [RuntimeControl::WorkerFailed(message)] if message.contains("gone")
    ));

    let (log, outputs, controls) = forward(
        vec![Ok(b"a".to_vec()), Err(io::ErrorKind::WouldBlock.into())],
        || Err(io::Error::other("closed")),
    );
    assert_eq!(log, ["read", "would-block", "wait"]);
    assert_eq!(outputs, [b"a".to_vec()]);
    assert!(matches!(
        &controls[..],
        [RuntimeControl::WorkerFailed(message)]
            if message.contains("readiness wait failed")
    ));
}

/// One 120 by 40 DOOM-fire style frame: truecolor foreground and
/// background SGR for every cell.
fn truecolor_frame() -> String {
    use std::fmt::Write as _;
    let mut frame = String::from("\x1b[H");
    for cell in 0..120 * 40 {
        let value = (cell * 13) % 256;
        let _ = write!(
            frame,
            "\x1b[38;2;{value};{};{};48;2;{};{};{}m\u{2580}",
            value / 2,
            value / 4,
            255 - value,
            value / 3,
            value / 5
        );
    }
    frame
}

/// Times a PTY flood of DOOM-fire style frames through the whole runtime:
/// reader, output channel, parser, and effects. A title after every
/// segment timestamps progress without snapshots, and the loop requests a
/// snapshot about 8 ms after the previous one completes, roughly like a
/// 120 Hz client, so output-queue depth shows up as segment-rate spread as
/// well as in the mean. Segment rates start at the first marker, which
/// excludes shell and `cat` startup.
#[test]
#[ignore = "release benchmark; run through mise run bench:engine"]
fn engine_benchmark_pty_throughput() {
    use std::fmt::Write as _;
    const SEGMENTS: u32 = 12;
    const SEGMENT_FRAMES: u32 = 50;
    let segment = truecolor_frame().repeat(SEGMENT_FRAMES as usize);
    let mut payload = String::new();
    for index in 0..SEGMENTS {
        payload.push_str(&segment);
        let _ = write!(payload, "\x1b]2;SEGMENT-{index}\x07");
    }
    let data = std::env::temp_dir()
        .join(format!("huterm-throughput-{}.bin", std::process::id()));
    std::fs::write(&data, &payload).unwrap();
    let mut elapsed = Vec::new();
    let mut segment_rates = Vec::new();
    for _ in 0..3 {
        let mut command = command(&format!(
            "stty -echo; printf READY; read go; cat '{}'; read done",
            data.display()
        ));
        command.grid_size = GridSize::clamped(120, 40);
        let runtime =
            TerminalRuntime::spawn(TerminalId::new(90), &command).unwrap();
        let client = runtime.client();
        wait_for_text(&client, "READY");
        client
            .send_input(TerminalInput::Text("go\n".into()))
            .unwrap();
        let started = Instant::now();
        let deadline = started + Duration::from_secs(60);
        let mut previous: Option<(u32, Instant)> = None;
        let mut snapshot_at = started;
        'done: loop {
            while let Some(event) = client.try_recv_event().unwrap() {
                let TerminalEvent::TitleChanged { title, .. } = event else {
                    continue;
                };
                let Some(index) = title
                    .strip_prefix("SEGMENT-")
                    .and_then(|index| index.parse::<u32>().ok())
                else {
                    continue;
                };
                let now = Instant::now();
                // Pending titles coalesce, so a late poll can skip a
                // marker; count every segment the gap covers.
                if let Some((before, at)) = previous {
                    segment_rates.push(
                        f64::from((index - before) * SEGMENT_FRAMES)
                            / (now - at).as_secs_f64(),
                    );
                }
                previous = Some((index, now));
                if index == SEGMENTS - 1 {
                    break 'done;
                }
            }
            assert!(Instant::now() < deadline, "throughput run timed out");
            if snapshot_at.elapsed() >= Duration::from_millis(8) {
                client.read_snapshot().unwrap();
                snapshot_at = Instant::now();
            }
            thread::sleep(Duration::from_millis(1));
        }
        elapsed.push(started.elapsed());
        runtime.shutdown().unwrap();
    }
    let _ = std::fs::remove_file(&data);
    elapsed.sort_unstable();
    assert!(
        !segment_rates.is_empty(),
        "no run observed two segment markers, so no rate was measured"
    );
    segment_rates.sort_by(f64::total_cmp);
    let count = f64::from(u32::try_from(segment_rates.len()).unwrap());
    let mean = segment_rates.iter().sum::<f64>() / count;
    let deviation = (segment_rates
        .iter()
        .map(|rate| (rate - mean).powi(2))
        .sum::<f64>()
        / count)
        .sqrt();
    let median = elapsed[elapsed.len() / 2];
    println!(
        "engine=ghostty fixture=pty-throughput bytes={} frames={} elapsed_ms_p50={:.1} mb_per_second={:.1} segment_fps_mean={mean:.0} segment_fps_stddev={deviation:.0} segment_fps_p10={:.0} segment_fps_p90={:.0} segment_fps_min={:.0}",
        payload.len(),
        SEGMENTS * SEGMENT_FRAMES,
        median.as_secs_f64() * 1e3,
        f64::from(u32::try_from(payload.len()).unwrap())
            / median.as_secs_f64()
            / 1e6,
        segment_rates[segment_rates.len() / 10],
        segment_rates[segment_rates.len() * 9 / 10],
        segment_rates[0],
    );
}

fn command(script: &str) -> TerminalCommand {
    TerminalCommand {
        program: PathBuf::from("/bin/sh"),
        arguments: vec!["-c".into(), script.into()],
        working_directory: std::env::current_dir()
            .expect("current directory should exist"),
        environment: Vec::new(),
        grid_size: GridSize::clamped(40, 8),
        cell_size: CellSize {
            width: 8,
            height: 16,
        },
        presentation: TerminalPresentation::default(),
    }
}

fn wait_for_text(client: &RuntimeClient, needle: &str) -> TerminalSnapshot {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let snapshot = client.read_snapshot().expect("snapshot should work");
        let text: String =
            snapshot.cells().map(|cell| cell.text.as_str()).collect();
        if text.contains(needle) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "terminal output did not contain {needle:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_exit(client: &RuntimeClient) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        while let Some(event) =
            client.try_recv_event().expect("event receiver should work")
        {
            if let TerminalEvent::Exited { status, .. } = event {
                return status;
            }
        }
        assert!(
            Instant::now() < deadline,
            "terminal child did not report exit"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn published_events_coalesce_into_one_pending_activity_signal() {
    let (events, receiver, activity) = EventPublisher::channel();
    let id = TerminalId::new(97);
    events.send(TerminalEvent::Ready(id)).unwrap();
    events.send(TerminalEvent::Bell(id)).unwrap();

    assert!(activity.try_recv().is_ok());
    assert!(activity.try_recv().is_err(), "signals must coalesce");
    assert!(receiver.try_recv().is_ok());
    assert!(receiver.try_recv().is_ok());
    assert!(receiver.try_recv().is_err());

    events.send(TerminalEvent::Bell(id)).unwrap();
    assert!(activity.try_recv().is_ok(), "a drained signal re-arms");
}

#[test]
fn runtime_signals_activity_for_its_published_events() {
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(98),
        &command("printf READY; IFS= read -r line"),
    )
    .expect("runtime should spawn");
    let client = runtime.client();
    let deadline = Instant::now() + Duration::from_secs(3);
    while client.activity.try_recv().is_err() {
        assert!(
            Instant::now() < deadline,
            "runtime published no activity signal"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        client.try_recv_event().unwrap().is_some(),
        "a signal means an event is waiting"
    );
}

#[test]
fn pty_runtime_should_round_trip_input_and_observe_exit() {
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(11),
        &command("printf READY; IFS= read -r line; printf ':%s' \"$line\""),
    )
    .expect("runtime should start");
    let client = runtime.client();
    wait_for_text(&client, "READY");

    client
        .send_input(TerminalInput::Text("hello\n".into()))
        .expect("input should send");
    let snapshot = wait_for_text(&client, ":hello");

    assert!(snapshot.generation >= 2);
    assert_eq!(
        wait_for_exit(&client),
        ExitStatus {
            code: Some(0),
            success: true
        }
    );
    let final_snapshot = client
        .read_snapshot()
        .expect("final snapshot should remain available after exit");
    assert!(final_snapshot.generation >= snapshot.generation);
    runtime.shutdown().expect("runtime should stop cleanly");
    loop {
        match client.try_recv_event() {
            Ok(Some(event)) => assert!(
                !matches!(event, TerminalEvent::Failed { .. }),
                "normal child exit should not report a runtime failure"
            ),
            Ok(None) | Err(RuntimeError::Stopped) => break,
            Err(error) => panic!("event receiver failed: {error}"),
        }
    }
}

#[test]
fn pty_runtime_applies_explicit_terminal_environment() {
    let mut command = command(
        "printf '%s|%s|%s|%s' \"$TERM\" \"$COLORTERM\" \"$TERM_PROGRAM\" \"$TERMINFO_DIRS\"",
    );
    command.environment = vec![
        ("TERM".into(), "xterm-huterm".into()),
        ("TERMINFO_DIRS".into(), "/private/terminfo:".into()),
    ];
    let runtime = TerminalRuntime::spawn(TerminalId::new(90), &command)
        .expect("runtime should start");
    wait_for_text(
        &runtime.client(),
        "xterm-huterm|truecolor|Huterm|/private/terminfo:",
    );
    runtime.shutdown().unwrap();
}

#[cfg(unix)]
#[test]
fn pty_runtime_should_process_bursty_output_without_poll_delay() {
    let started = Instant::now();
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(17),
        &command("dd if=/dev/zero bs=1048576 count=1 2>/dev/null; printf DONE"),
    )
    .expect("runtime should start");
    wait_for_text(&runtime.client(), "DONE");
    let elapsed = started.elapsed();
    runtime.shutdown().expect("runtime should stop cleanly");

    assert!(
        elapsed < Duration::from_secs(1),
        "bursty PTY output took {elapsed:?} to process"
    );
}

#[test]
fn pty_runtime_should_resize_grid_and_pty() {
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(12),
        &command("printf READY; sleep 2"),
    )
    .expect("runtime should start");
    let client = runtime.client();
    wait_for_text(&client, "READY");

    client
        .resize(
            GridSize::clamped(52, 13),
            CellSize {
                width: 9,
                height: 17,
            },
        )
        .expect("resize should send");
    let deadline = Instant::now() + Duration::from_secs(2);
    let snapshot = loop {
        let snapshot = client.read_snapshot().expect("snapshot should work");
        if snapshot.size == GridSize::clamped(52, 13) {
            break snapshot;
        }
        assert!(Instant::now() < deadline, "terminal grid did not resize");
    };

    assert_eq!(snapshot.cells().count(), 52 * 13);
    runtime.shutdown().expect("runtime should stop cleanly");
}

#[test]
fn pty_runtime_should_fail_cleanly_for_missing_program() {
    let mut missing = command("");
    missing.program = PathBuf::from("/definitely/missing/huterm-test-program");

    let error = TerminalRuntime::spawn(TerminalId::new(13), &missing)
        .expect_err("missing executable should fail");

    assert!(matches!(error, RuntimeError::Spawn(_)));
}

#[test]
fn draining_dirty_notification_does_not_rearm_until_snapshot() {
    fn title(client: &RuntimeClient, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if matches!(client.try_recv_event().unwrap(), Some(TerminalEvent::TitleChanged { title, .. }) if title == expected)
            {
                return;
            }
            assert!(Instant::now() < deadline, "missing title {expected}");
            thread::yield_now();
        }
    }
    fn invalidation(client: &RuntimeClient) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if matches!(
                client.try_recv_event().unwrap(),
                Some(TerminalEvent::Invalidated { .. })
            ) {
                return;
            }
            assert!(Instant::now() < deadline, "missing invalidation");
            thread::yield_now();
        }
    }
    let runtime = TerminalRuntime::spawn(TerminalId::new(99), &command(
        "stty -echo; printf READY; while IFS= read -r line; do printf '%s\\033]2;%s\\007' \"$line\" \"$line\"; done"
    )).unwrap();
    let client = runtime.client();
    wait_for_text(&client, "READY");
    while client.try_recv_event().unwrap().is_some() {}
    client
        .send_input(TerminalInput::Text("one\n".into()))
        .unwrap();
    invalidation(&client);
    client
        .send_input(TerminalInput::Text("two\n".into()))
        .unwrap();
    title(&client, "two");
    // This ordered control completes after the output that published the title.
    client.job_context().unwrap();
    while let Some(event) = client.try_recv_event().unwrap() {
        assert!(!matches!(event, TerminalEvent::Invalidated { .. }));
    }
    client.read_snapshot().unwrap();
    client
        .send_input(TerminalInput::Text("three\n".into()))
        .unwrap();
    invalidation(&client);
    runtime.shutdown().unwrap();
}

#[test]
fn invalidation_should_coalesce_until_client_consumes_it() {
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(14),
        &command("printf 'one\\ntwo\\nthree\\n'; sleep 1"),
    )
    .expect("runtime should start");
    let client = runtime.client();
    wait_for_text(&client, "three");
    let mut invalidations = 0;
    while let Some(event) = client.try_recv_event().unwrap_or(None) {
        if matches!(event, TerminalEvent::Invalidated { .. }) {
            invalidations += 1;
        }
    }

    assert_eq!(invalidations, 1);
    runtime.shutdown().expect("runtime should stop cleanly");
}

#[test]
fn shutdown_should_escalate_for_a_hup_resistant_child() {
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(15),
        &command("trap '' HUP TERM; printf READY; while :; do sleep 30; done"),
    )
    .expect("runtime should start");
    wait_for_text(&runtime.client(), "READY");

    let started = Instant::now();
    runtime.shutdown().expect("runtime should stop cleanly");

    assert!(
        started.elapsed() < Duration::from_secs(3),
        "shutdown should not wait for the live child"
    );
}

#[test]
fn saturated_input_should_not_block_client_or_priority_close() {
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(16),
        &command("trap '' HUP TERM; printf READY; while :; do sleep 30; done"),
    )
    .expect("runtime should start");
    let client = runtime.client();
    wait_for_text(&client, "READY");

    assert!(matches!(
        client.send_input(TerminalInput::Text(
            "x".repeat(INPUT_BYTE_CAPACITY + 1)
        )),
        Err(RuntimeError::Busy)
    ));

    let enqueue_started = Instant::now();
    let mut accepted = 0;
    for _ in 0..2_048 {
        match client.send_input(TerminalInput::Text("x".repeat(1_024))) {
            Ok(()) => accepted += 1,
            Err(RuntimeError::Busy) => break,
            Err(error) => {
                panic!("runtime stopped while filling queue: {error}")
            }
        }
    }
    assert!(accepted > 0, "runtime should accept initial input");
    assert!(accepted < 2_048, "bounded ingress should report saturation");
    assert!(
        enqueue_started.elapsed() < Duration::from_secs(1),
        "client enqueue should not inherit bounded queue backpressure"
    );

    let shutdown_started = Instant::now();
    runtime.shutdown().expect("runtime should stop cleanly");
    assert!(
        shutdown_started.elapsed() < Duration::from_secs(3),
        "priority close should bypass saturated data traffic"
    );
}

#[test]
fn client_input_is_admitted_and_handled_ahead_of_queued_output() {
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(17),
        &command(
            "stty -echo; printf READY; IFS= read -r line; printf 'GOT:%s|' \"$line\" | tr '\\033' E",
        ),
    )
    .expect("runtime should start");
    let client = runtime.client();
    wait_for_text(&client, "READY");

    let (entered, paused) = mpsc::channel();
    let (resume, release) = mpsc::channel();
    client
        .controls
        .send(RuntimeControl::Pause { entered, release })
        .unwrap();
    paused
        .recv_timeout(Duration::from_secs(2))
        .expect("runtime should pause");
    // A cursor-position query heads the backlog. Its reply reaches the
    // child's line before the input only if queued output is parsed first.
    let mut backlog = 0;
    while client
        .output
        .try_send(if backlog == 0 { &b"\x1b[6n"[..] } else { b"." }.to_vec())
        .is_ok()
    {
        backlog += 1;
    }
    assert!(backlog > 0, "the paused runtime should queue output");

    let admitted = client.send_input(TerminalInput::Text("hello\n".into()));
    resume.send(()).unwrap();
    admitted.expect("a full output queue should not refuse client input");
    wait_for_text(&client, "GOT:hello|");
    runtime.shutdown().expect("runtime should stop cleanly");
}

#[test]
fn queued_client_messages_alternate_with_output_until_both_disconnect() {
    let (messages, client) = mpsc::sync_channel(4);
    let (output, pty) = mpsc::sync_channel(4);
    for _ in 0..2 {
        messages
            .send(RuntimeMessage::Resize {
                grid: GridSize::clamped(1, 1),
                cell: CellSize {
                    width: 1,
                    height: 1,
                },
            })
            .unwrap();
    }
    for byte in *b"abc" {
        output.send(vec![byte]).unwrap();
    }
    let mut output_turn = false;
    let mut order = String::new();
    while let Ok(next) = next_message(&client, &pty, &mut output_turn) {
        order.push(match next {
            NextMessage::Client(_) => 'C',
            NextMessage::Output(bytes) => char::from(bytes[0]),
        });
    }
    assert_eq!(order, "CaCbc");

    drop(output);
    assert_eq!(
        next_message(&client, &pty, &mut output_turn).err(),
        Some(TryRecvError::Empty),
        "clients can still send after the reader stops"
    );
    drop(messages);
    assert_eq!(
        next_message(&client, &pty, &mut output_turn).err(),
        Some(TryRecvError::Disconnected)
    );
}

#[test]
fn input_beyond_writer_capacity_arrives_once_the_child_reads() {
    // More writes than the PTY buffer and writer queue hold, but few
    // enough that the rest fits the runtime's ingress. The client finishes
    // sending before the child reads, leaving the writer's dequeue as the
    // runtime's only wake.
    const INPUTS: usize = 100;
    const INPUT_BYTES: usize = 4_000;
    let marker = std::env::temp_dir()
        .join(format!("huterm-writer-capacity-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let script = format!(
        "stty raw -echo; printf READY; while [ ! -e {marker} ]; do sleep 0.05; done; head -c {total} >/dev/null && printf DONE; sleep 30",
        marker = marker.display(),
        total = INPUTS * INPUT_BYTES,
    );
    let runtime =
        TerminalRuntime::spawn(TerminalId::new(21), &command(&script))
            .expect("runtime should start");
    let client = runtime.client();
    wait_for_text(&client, "READY");
    while client.try_recv_event().unwrap().is_some() {}
    let deadline = Instant::now() + Duration::from_secs(5);
    for _ in 0..INPUTS {
        let mut input = TerminalInput::Text("x".repeat(INPUT_BYTES));
        // Sending can outpace the runtime. Refused sends wake it, which
        // is harmless only until the child is released.
        while let Err(refused) = client.offer_input(input) {
            assert!(
                matches!(refused.error, RuntimeError::Busy),
                "runtime stopped: {}",
                refused.error
            );
            assert!(Instant::now() < deadline, "runtime ingress stalled");
            input = refused.input;
            thread::yield_now();
        }
    }
    std::fs::write(&marker, b"").expect("marker should be writable");
    // Snapshot requests wake the runtime; events do not. DONE is the only
    // output after READY, and it needs every write delivered.
    while !matches!(
        client.try_recv_event().unwrap(),
        Some(TerminalEvent::Invalidated { .. })
    ) {
        assert!(
            Instant::now() < deadline,
            "runtime did not resume writing after the child read"
        );
        thread::sleep(Duration::from_millis(10));
    }
    wait_for_text(&client, "DONE");
    runtime.shutdown().expect("runtime should stop cleanly");
    std::fs::remove_file(&marker).expect("marker should be removable");
}

#[test]
fn pending_writer_spill_should_drain_fifo_until_full() {
    let (sender, receiver) = async_channel::bounded(2);
    let mut pending =
        VecDeque::from([b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]);

    assert_eq!(
        flush_pending_write(&sender, &mut pending),
        WriterQueueState::Full
    );
    assert_eq!(pending, VecDeque::from([b"three".to_vec()]));
    let receive = || match receiver.try_recv().expect("write should exist") {
        WriterMessage::Write(bytes) => bytes,
    };
    assert_eq!((receive(), receive()), (b"one".to_vec(), b"two".to_vec()));

    assert_eq!(
        flush_pending_write(&sender, &mut pending),
        WriterQueueState::Drained
    );
    assert_eq!(receive(), b"three".to_vec());
}

#[test]
fn idle_writer_closes_without_an_input_message() {
    struct IdleWriter {
        started: Sender<()>,
        dropped: Sender<()>,
    }
    impl Write for IdleWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            panic!("idle writer must not receive input");
        }
        fn flush(&mut self) -> io::Result<()> {
            self.started.send(()).unwrap();
            Ok(())
        }
    }
    impl Drop for IdleWriter {
        fn drop(&mut self) {
            let _ = self.dropped.send(());
        }
    }
    let (dropped, observed) = mpsc::channel();
    let (started, ready) = mpsc::channel();
    let (sender, receiver) = async_channel::bounded(1);
    let (controls, _control_receiver) = mpsc::channel();
    let controls = crate::wake::Sender::new(
        controls,
        Arc::new(crate::wake::Wake::default()),
    );
    let input_closed = Arc::new(AtomicBool::new(false));
    let worker = spawn_writer(
        TerminalId::new(19),
        Box::new(IdleWriter { started, dropped }),
        WriterReadiness::Immediate,
        receiver,
        controls,
        Arc::new(WriterCapacity::default()),
        Arc::new(AtomicBool::new(false)),
        Arc::clone(&input_closed),
    )
    .unwrap();
    sender
        .send_blocking(WriterMessage::Write(Vec::new()))
        .unwrap();
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    input_closed.store(true, Ordering::Release);
    sender.close();
    observed
        .recv_timeout(Duration::from_secs(2))
        .expect("idle writer did not release its descriptor on input closure");
    worker.join().unwrap();
}

#[test]
fn writer_wakes_the_runtime_only_after_it_requests_capacity() {
    struct ReportingWriter(Sender<Vec<u8>>);
    impl Write for ReportingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.send(bytes.to_vec()).unwrap();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let (written, observed) = mpsc::channel();
    let (sender, receiver) = async_channel::bounded(1);
    let (controls, _control_receiver) = mpsc::channel();
    let wake = Arc::new(crate::wake::Wake::default());
    let controls = crate::wake::Sender::new(controls, Arc::clone(&wake));
    let capacity = Arc::new(WriterCapacity::default());
    let closing = Arc::new(AtomicBool::new(false));
    let join = spawn_writer(
        TerminalId::new(20),
        Box::new(ReportingWriter(written)),
        WriterReadiness::Immediate,
        receiver,
        controls,
        Arc::clone(&capacity),
        Arc::clone(&closing),
        Arc::new(AtomicBool::new(false)),
    )
    .expect("writer worker should start");
    // The writer dequeues each message before writing it, so an observed
    // write means its wake decision has been made.
    let write = |bytes: &[u8]| {
        sender
            .send_blocking(WriterMessage::Write(bytes.to_vec()))
            .expect("write should queue");
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(2)).unwrap(),
            bytes
        );
        wake.take_pending()
    };

    assert!(!write(b"a"), "an unrequested dequeue woke the runtime");
    capacity.request_wake();
    assert!(write(b"b"), "a requested dequeue did not wake the runtime");
    assert!(!write(b"c"), "a consumed request woke the runtime again");

    closing.store(true, Ordering::Release);
    drop(sender);
    join.join().expect("writer worker should stop");
}

#[derive(Debug)]
struct PartialWouldBlockWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
    block_next: bool,
}

impl Write for PartialWouldBlockWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.block_next {
            self.block_next = false;
            return Err(io::ErrorKind::WouldBlock.into());
        }
        self.block_next = true;
        let count = bytes.len().min(2);
        self.bytes
            .lock()
            .expect("captured bytes should not be poisoned")
            .extend_from_slice(&bytes[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn writer_should_resume_after_partial_would_block_without_repeating_bytes() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = PartialWouldBlockWriter {
        bytes: Arc::clone(&bytes),
        block_next: false,
    };
    let (sender, receiver) = async_channel::bounded(1);
    let (controls, _control_receiver) = mpsc::channel();
    let controls = crate::wake::Sender::new(
        controls,
        Arc::new(crate::wake::Wake::default()),
    );
    let closing = Arc::new(AtomicBool::new(false));
    let join = spawn_writer(
        TerminalId::new(18),
        Box::new(writer),
        WriterReadiness::Immediate,
        receiver,
        controls,
        Arc::new(WriterCapacity::default()),
        Arc::clone(&closing),
        Arc::new(AtomicBool::new(false)),
    )
    .expect("writer worker should start");
    sender
        .send_blocking(WriterMessage::Write(b"abcdef".to_vec()))
        .expect("write should queue");

    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if bytes
            .lock()
            .expect("captured bytes should not be poisoned")
            .len()
            == 6
        {
            break;
        }
        assert!(Instant::now() < deadline, "write did not complete");
        thread::sleep(Duration::from_millis(2));
    }
    closing.store(true, Ordering::Release);
    drop(sender);
    join.join().expect("writer worker should stop");

    assert_eq!(
        *bytes.lock().expect("captured bytes should not be poisoned"),
        b"abcdef"
    );
}

#[cfg(unix)]
#[test]
fn shutdown_should_terminate_a_distinct_foreground_process_group() {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let pid_path = std::env::temp_dir()
        .join(format!("huterm-foreground-job-{}.pid", std::process::id()));
    let _ = std::fs::remove_file(&pid_path);
    let script = format!(
        "trap '' HUP TERM; set -m; sleep 30 & child=$!; printf %s \"$child\" > {}; printf READY; fg",
        pid_path.display()
    );
    let runtime =
        TerminalRuntime::spawn(TerminalId::new(17), &command(&script))
            .expect("runtime should start");
    wait_for_text(&runtime.client(), "READY");
    let foreground_pid: i32 = std::fs::read_to_string(&pid_path)
        .expect("foreground PID should be written")
        .parse()
        .expect("foreground PID should be numeric");

    runtime.shutdown().expect("runtime should stop cleanly");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match kill(Pid::from_raw(foreground_pid), None) {
            Err(Errno::ESRCH) => break,
            Ok(()) | Err(_) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            result => panic!(
                "foreground process {foreground_pid} survived shutdown: {result:?}"
            ),
        }
    }
    std::fs::remove_file(pid_path).expect("PID file should be removable");
}
