//! Spawns whose PTY creation never returns, as macOS's `openpty` can under
//! PTY exhaustion. A private gate parks its holder inside the critical
//! section, so these tests never touch the process-wide gate other tests
//! use, and never exhaust the host's PTYs.

use std::path::PathBuf;

use super::*;
use crate::pty::{PtyGate, STALLED, TIMED_OUT};
use crate::test_support::{Pause, WAIT, wait_until};

/// A startup deadline that expires while the runtime is parked.
const SHORT: Duration = Duration::from_millis(50);

fn command() -> TerminalCommand {
    TerminalCommand {
        program: PathBuf::from("/bin/sh"),
        arguments: vec!["-c".into(), "exec sleep 30".into()],
        working_directory: std::env::current_dir()
            .expect("current directory should exist"),
        environment: Vec::new(),
        grid_size: GridSize::clamped(40, 8),
        cell_size: CellSize {
            width: 8,
            height: 16,
        },
        presentation: TerminalPresentation::default(),
        scrollback: huterm_protocol::ScrollbackLimit::DEFAULT,
        shell_integration: huterm_protocol::ShellIntegration::OFF,
        scrollback_compression: false,
    }
}

fn spawn(
    id: u64,
    gate: &Arc<PtyGate>,
    deadline: Duration,
    abandoned: &Sender<JoinHandle<Result<(), RuntimeError>>>,
) -> Result<TerminalRuntime, RuntimeError> {
    let terminal_id = TerminalId::new(id);
    TerminalRuntime::spawn_with(
        terminal_id.runtime(),
        terminal_id,
        &command(),
        &Startup {
            deadline,
            gate: Arc::clone(gate),
            abandoned: Some(abandoned.clone()),
        },
    )
}

fn pty_message(result: Result<TerminalRuntime, RuntimeError>) -> String {
    match result {
        Err(RuntimeError::Pty(message)) => message,
        other => panic!("expected a PTY error, got {other:?}"),
    }
}

/// Waits for an abandoned runtime's thread to end and returns its result.
fn finished(
    join: JoinHandle<Result<(), RuntimeError>>,
) -> Result<(), RuntimeError> {
    wait_until("the abandoned runtime to finish", || {
        if join.is_finished() {
            Ok(())
        } else {
            Err("running")
        }
    });
    join.join().expect("the abandoned runtime should not panic")
}

#[test]
fn a_spawn_stuck_creating_its_pty_times_out_and_stops_once_it_returns() {
    let pause = Arc::new(Pause::default());
    let _release = pause.releasing();
    let gate = Arc::new(PtyGate::paused(WAIT, Arc::clone(&pause)));
    let (abandoned, abandoned_runtimes) = mpsc::channel();

    let result = spawn(201, &gate, SHORT, &abandoned);

    assert_eq!(pty_message(result), TIMED_OUT);
    pause.wait_blocked(1);
    let join = abandoned_runtimes
        .try_recv()
        .expect("a timed-out spawn should hand over its runtime thread");
    assert!(!join.is_finished(), "the runtime should still be parked");

    pause.release();
    // The late return sees the close before it starts a child, and the
    // thread ends with the error nobody receives.
    let late = finished(join);
    assert!(
        matches!(&late, Err(RuntimeError::Pty(message)) if message == TIMED_OUT),
        "the abandoned runtime should stop before its child: {late:?}"
    );

    // The abandoned runtime left the gate free, and an ordinary spawn
    // through it starts and stops.
    let runtime = spawn(202, &gate, STARTUP_DEADLINE, &abandoned)
        .expect("a spawn after the stuck one returned should start");
    runtime.shutdown().unwrap();
    assert!(abandoned_runtimes.try_recv().is_err());
}

#[test]
fn a_later_spawn_fails_fast_while_an_earlier_one_is_stuck() {
    let pause = Arc::new(Pause::default());
    let _release = pause.releasing();
    // A zero allowance treats any holder as stalled, so the refusal does
    // not depend on how long the first spawn has held the gate.
    let gate = Arc::new(PtyGate::paused(Duration::ZERO, Arc::clone(&pause)));
    let (abandoned, abandoned_runtimes) = mpsc::channel();
    assert_eq!(pty_message(spawn(203, &gate, SHORT, &abandoned)), TIMED_OUT);
    pause.wait_blocked(1);
    let stuck = abandoned_runtimes.try_recv().unwrap();

    // The production deadline would let this spawn queue for ten seconds;
    // the gate's message shows the gate refused it instead.
    let result = spawn(204, &gate, STARTUP_DEADLINE, &abandoned);

    assert_eq!(pty_message(result), STALLED);
    assert!(
        abandoned_runtimes.try_recv().is_err(),
        "a refused spawn joins its runtime instead of abandoning it"
    );
    pause.release();
    assert!(finished(stuck).is_err());
}
