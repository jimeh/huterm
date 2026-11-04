//! Command records as viewers see them: published with status, per output
//! chunk, and settled only once output can no longer end them.

use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use huterm_protocol::{
    CellSize, CommandState, GridSize, TerminalCommand, TerminalId,
    TerminalLifecycle, TerminalStatus,
};

use super::{RuntimeControl, TerminalRuntime};
use crate::test_support::TestClient;

/// A scratch directory holding FIFOs the script waits on.
struct Gates(PathBuf);

impl Gates {
    fn new(names: &[&str]) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "huterm-records-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        for name in names {
            nix::unistd::mkfifo(
                &directory.join(name),
                nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
            )
            .unwrap();
        }
        Self(directory)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// Releases the reader blocked on `name`, waiting until it opens.
    fn release(&self, name: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(nix::fcntl::OFlag::O_NONBLOCK.bits())
                .open(self.path(name))
            {
                Ok(mut fifo) => return fifo.write_all(b"go\n").unwrap(),
                Err(error) => assert!(
                    Instant::now() < deadline,
                    "nothing waits on {name}: {error}"
                ),
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Gates {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn command(script: &str, gates: &Path) -> TerminalCommand {
    TerminalCommand {
        program: PathBuf::from("/bin/sh"),
        arguments: vec!["-c".into(), script.into()],
        working_directory: std::env::current_dir().unwrap(),
        // The script reads only this; nothing inherited is printed.
        environment: vec![(
            "GATES".into(),
            gates.to_string_lossy().into_owned(),
        )],
        grid_size: GridSize::clamped(40, 8),
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

/// Polls the published status until `accept` holds.
fn wait_for_status(
    client: &TestClient,
    what: &str,
    accept: impl Fn(&TerminalStatus) -> bool,
) -> Arc<TerminalStatus> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = client.status();
        if accept(&status) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "{what}: last status had lifecycle {:?} and records {:?}",
            status.lifecycle,
            states(&status)
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn states(status: &TerminalStatus) -> Vec<(u64, CommandState)> {
    status
        .commands
        .iter()
        .map(|record| (record.sequence, record.state))
        .collect()
}

fn wait_for_states(
    client: &TestClient,
    expected: &[(u64, CommandState)],
) -> Arc<TerminalStatus> {
    wait_for_status(client, &format!("records {expected:?}"), |status| {
        states(status) == expected
    })
}

/// Injects output as if the reader had read it from the PTY.
fn output(client: &TestClient, bytes: &str) {
    client
        .control
        .output
        .send(bytes.as_bytes().to_vec())
        .unwrap();
}

const fn finished(exit_code: i32) -> CommandState {
    CommandState::Finished {
        exit_code: Some(exit_code),
    }
}

/// Holds the runtime inside its control drain until the returned sender
/// is used, so everything sent meanwhile is queued when it resumes.
fn pause(client: &TestClient) -> std::sync::mpsc::Sender<()> {
    let (entered, paused) = std::sync::mpsc::channel();
    let (resume, release) = std::sync::mpsc::channel();
    client
        .control
        .controls
        .send(RuntimeControl::Pause { entered, release })
        .unwrap();
    paused.recv_timeout(Duration::from_secs(5)).unwrap();
    resume
}

fn control(client: &TestClient, control: RuntimeControl) {
    client.control.controls.send(control).unwrap();
}

#[test]
fn markers_queued_behind_the_exit_and_end_of_output_still_apply() {
    let gates = Gates::new(&["root"]);
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(140),
        &command(r#"printf READY; read -r x < "$GATES/root""#, &gates.0),
    )
    .unwrap();
    let client = TestClient::new(&runtime);
    wait_for_ready(&client);
    output(&client, "\x1b]133;A\x07$ \x1b]133;C\x07");
    wait_for_states(&client, &[(1, CommandState::Running)]);
    let root = client.job_context().unwrap().shell.unwrap();

    let resume = pause(&client);
    gates.release("root");
    let deadline = Instant::now() + Duration::from_secs(5);
    while huterm_procinfo::process(root).is_some_and(|root| !root.zombie) {
        assert!(Instant::now() < deadline, "the root did not exit");
        thread::sleep(Duration::from_millis(5));
    }
    // Output the reader queued before the end of output: a D, a C and D,
    // and a C whose D never comes.
    output(&client, "\x1b]133;D;0\x07");
    output(&client, "\x1b]133;C\x07late\r\n\x1b]133;D;3\x07");
    output(&client, "\x1b]133;C\x07");
    // Controls run ahead of queued output. A writer report makes the
    // runtime observe the exit before it parses that output, and the end
    // of output follows; the real reader's end may come anywhere around
    // them.
    control(&client, RuntimeControl::WriterFailed("probe".into()));
    control(&client, RuntimeControl::PtyEof);
    resume.send(()).unwrap();
    let ended = wait_for_states(
        &client,
        &[
            (1, finished(0)),
            (2, finished(3)),
            (3, CommandState::Abandoned),
        ],
    );
    assert!(matches!(ended.lifecycle, TerminalLifecycle::Exited(_)));
    assert_eq!(
        ended.failures,
        [],
        "a writer report after exit is no failure"
    );
    assert!(ended.commands[2].duration.is_some());
    runtime.shutdown().unwrap();
}

/// A reader failure stops the runtime, so no later output can end a running
/// record, even when the root's exit is still unobserved at that point.
#[test]
fn a_reader_failure_settles_a_running_record_when_the_runtime_stops() {
    let gates = Gates::new(&["root"]);
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(143),
        &command(r#"printf READY; read -r x < "$GATES/root""#, &gates.0),
    )
    .unwrap();
    let client = TestClient::new(&runtime);
    wait_for_ready(&client);
    output(&client, "\x1b]133;A\x07$ \x1b]133;C\x07");
    wait_for_states(&client, &[(1, CommandState::Running)]);
    let root = client.job_context().unwrap().shell.unwrap();

    let resume = pause(&client);
    gates.release("root");
    let deadline = Instant::now() + Duration::from_secs(5);
    while huterm_procinfo::process(root).is_some_and(|root| !root.zombie) {
        assert!(Instant::now() < deadline, "the root did not exit");
        thread::sleep(Duration::from_millis(5));
    }
    // The failure is the first thing the runtime handles after the exit,
    // so it stops before another turn observes that exit.
    control(&client, RuntimeControl::WorkerFailed("probe".into()));
    resume.send(()).unwrap();
    // The runtime publishes the failure once, as it stops.
    let stopped = wait_for_status(&client, "the reader failure", |status| {
        status
            .failures
            .iter()
            .any(|failure| failure.message == "probe")
    });
    assert_eq!(states(&stopped), [(1, CommandState::Abandoned)]);
    assert!(stopped.commands[0].duration.is_some());
    runtime.shutdown().unwrap();
}

/// On Linux a descendant can hold the terminal open after the root exits;
/// a macOS session leader's exit hangs the terminal up for every process.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "a session leader's exit hangs the terminal up on macOS"
)]
fn a_running_record_stays_running_while_a_descendant_holds_the_terminal() {
    let gates = Gates::new(&["root", "holder"]);
    let script = r#"trap '' HUP
(trap '' HUP; read -r x < "$GATES/holder") &
printf READY
read -r x < "$GATES/root""#;
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(142),
        &command(script, &gates.0),
    )
    .unwrap();
    let client = TestClient::new(&runtime);
    wait_for_ready(&client);
    output(&client, "\x1b]133;A\x07$ \x1b]133;C\x07");
    wait_for_states(&client, &[(1, CommandState::Running)]);
    gates.release("root");
    let exited = wait_for_status(&client, "root exit", |status| {
        matches!(status.lifecycle, TerminalLifecycle::Exited(_))
    });
    assert_eq!(states(&exited), [(1, CommandState::Running)]);
    // Output read after the observed exit applies normally.
    output(&client, "\x1b]133;D;0\x07\x1b]133;C\x07");
    let running = wait_for_states(
        &client,
        &[(1, finished(0)), (2, CommandState::Running)],
    );
    assert!(!client.job_context().unwrap().pty_eof);
    assert_eq!(client.status().revision, running.revision);
    gates.release("holder");
    wait_for_states(&client, &[(1, finished(0)), (2, CommandState::Abandoned)]);
    runtime.shutdown().unwrap();
}

/// Waits for the script's READY and returns the snapshot showing it.
fn wait_for_ready(
    client: &TestClient,
) -> Arc<huterm_protocol::TerminalSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = client.read_snapshot().unwrap();
        let text: String =
            snapshot.cells().map(|cell| cell.text.as_str()).collect();
        if text.contains("READY") {
            return snapshot;
        }
        assert!(Instant::now() < deadline, "the script never started");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_hidden_viewer_sees_a_start_and_its_end_as_separate_status_changes() {
    let gates = Gates::new(&["root"]);
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(141),
        &command(r#"printf READY; read -r x < "$GATES/root""#, &gates.0),
    )
    .unwrap();
    let client = TestClient::new(&runtime);
    let before = wait_for_ready(&client);
    // Visible output leaves a snapshot due, after which output wakes the
    // viewer no more: it stands for a hidden view.
    output(&client, "x");
    let deadline = Instant::now() + Duration::from_secs(5);
    while client.poll().invalidated.is_none() {
        assert!(Instant::now() < deadline, "no output publication");
        thread::sleep(Duration::from_millis(5));
    }
    while client.viewer.wake_pending() {}

    // Markers change no cells, yet each reaches the viewer as status.
    let mut seen = Vec::new();
    for (marker, expected) in [
        ("\x1b]133;C\x07", CommandState::Running),
        ("\x1b]133;D;1\x07", finished(1)),
    ] {
        output(&client, marker);
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            while !client.viewer.wake_pending() {
                assert!(Instant::now() < deadline, "no wake for {marker:?}");
                thread::sleep(Duration::from_millis(5));
            }
            if let Some(status) = client.poll().status
                && states(&status) == [(1, expected)]
            {
                break status;
            }
        };
        seen.push(status.revision);
    }
    assert!(seen[0] < seen[1], "one publication each: {seen:?}");
    // The rows the markers came with are unchanged: only "x" was printed.
    let after = client.read_snapshot().unwrap();
    let text = |snapshot: &huterm_protocol::TerminalSnapshot| {
        snapshot
            .cells()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
            .trim_end()
            .to_owned()
    };
    assert_eq!(text(&after), format!("{}x", text(&before)));
    gates.release("root");
    runtime.shutdown().unwrap();
}
