use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{
    self, Receiver, RecvTimeoutError, SyncSender, TryRecvError,
};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use huterm_protocol::{
    CellSize, ExitStatus, GridSize, TerminalCommand, TerminalEvent, TerminalId,
    TerminalInput, TerminalSnapshot, Viewport,
};
use portable_pty::Child;
use thiserror::Error;

use crate::engine::{EngineEffect, TerminalEngine};
use crate::input::encode_input;
use crate::pty::{self, PtyProcess};

const MESSAGE_CAPACITY: usize = 64;
const WRITER_CAPACITY: usize = 64;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// In-process client handle for one terminal runtime.
#[derive(Clone, Debug)]
pub struct RuntimeClient {
    terminal_id: TerminalId,
    messages: SyncSender<RuntimeMessage>,
    events: Arc<Mutex<Receiver<TerminalEvent>>>,
    invalidation_pending: Arc<AtomicBool>,
}

impl RuntimeClient {
    /// Returns the terminal addressed by this client.
    #[must_use]
    pub fn terminal_id(&self) -> TerminalId {
        self.terminal_id
    }

    /// Sends structured input using emulator modes owned by the runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal has stopped.
    pub fn send_input(&self, input: TerminalInput) -> Result<(), RuntimeError> {
        self.messages
            .send(RuntimeMessage::Input(input))
            .map_err(|_| RuntimeError::Stopped)
    }

    /// Resizes both the PTY and canonical emulator grid.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal has stopped.
    pub fn resize(
        &self,
        grid: GridSize,
        cell: CellSize,
    ) -> Result<(), RuntimeError> {
        self.messages
            .send(RuntimeMessage::Resize {
                grid: GridSize::clamped(grid.columns, grid.rows),
                cell,
            })
            .map_err(|_| RuntimeError::Stopped)
    }

    /// Reads an immutable snapshot for a client-owned viewport.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime is unavailable or does not answer.
    pub fn read_snapshot(
        &self,
        viewport: Viewport,
    ) -> Result<TerminalSnapshot, RuntimeError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.messages
            .send(RuntimeMessage::Snapshot {
                viewport,
                reply: sender,
            })
            .map_err(|_| RuntimeError::Stopped)?;
        receiver
            .recv_timeout(REQUEST_TIMEOUT)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => RuntimeError::TimedOut,
                RecvTimeoutError::Disconnected => RuntimeError::Stopped,
            })
    }

    /// Receives the next queued runtime event without blocking.
    ///
    /// # Errors
    ///
    /// Returns an error if another client poisoned the shared event receiver.
    pub fn try_recv_event(
        &self,
    ) -> Result<Option<TerminalEvent>, RuntimeError> {
        let event = match self
            .events
            .lock()
            .map_err(|_| RuntimeError::EventReceiverPoisoned)?
            .try_recv()
        {
            Ok(event) => Some(event),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                return Err(RuntimeError::Stopped);
            }
        };
        if matches!(event, Some(TerminalEvent::Invalidated { .. })) {
            self.invalidation_pending.store(false, Ordering::Release);
        }
        Ok(event)
    }

    /// Requests orderly terminal shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal has already stopped.
    pub fn close(&self) -> Result<(), RuntimeError> {
        self.messages
            .send(RuntimeMessage::Close)
            .map_err(|_| RuntimeError::Stopped)
    }
}

/// Owner of one terminal's runtime thread and cleanup path.
#[derive(Debug)]
pub struct TerminalRuntime {
    client: RuntimeClient,
    join: Option<JoinHandle<()>>,
}

impl TerminalRuntime {
    /// Starts a PTY, child process, emulator owner, and I/O workers.
    ///
    /// # Errors
    ///
    /// Returns a typed startup error if the PTY or child cannot be created.
    pub fn spawn(
        terminal_id: TerminalId,
        command: &TerminalCommand,
    ) -> Result<Self, RuntimeError> {
        let process = pty::spawn(command)?;
        let (message_sender, message_receiver) =
            mpsc::sync_channel(MESSAGE_CAPACITY);
        let (event_sender, event_receiver) = mpsc::channel();
        let invalidation_pending = Arc::new(AtomicBool::new(false));
        let runtime_pending = Arc::clone(&invalidation_pending);
        let initial_size = command.grid_size;
        let runtime_sender = message_sender.clone();
        let join = thread::Builder::new()
            .name(format!("huterm-runtime-{}", terminal_id.get()))
            .spawn(move || {
                run_terminal(
                    terminal_id,
                    initial_size,
                    process,
                    message_receiver,
                    runtime_sender,
                    event_sender,
                    runtime_pending,
                );
            })
            .map_err(|error| RuntimeError::Thread(error.to_string()))?;

        let client = RuntimeClient {
            terminal_id,
            messages: message_sender,
            events: Arc::new(Mutex::new(event_receiver)),
            invalidation_pending,
        };
        Ok(Self {
            client,
            join: Some(join),
        })
    }

    /// Returns a cloneable client handle.
    #[must_use]
    pub fn client(&self) -> RuntimeClient {
        self.client.clone()
    }

    /// Stops the child and joins the runtime owner.
    ///
    /// # Errors
    ///
    /// Returns an error if the runtime thread panicked.
    pub fn shutdown(mut self) -> Result<(), RuntimeError> {
        let _ = self.client.close();
        self.join_runtime()
    }

    fn join_runtime(&mut self) -> Result<(), RuntimeError> {
        let Some(join) = self.join.take() else {
            return Ok(());
        };
        join.join().map_err(|_| RuntimeError::ThreadPanic)
    }
}

impl Drop for TerminalRuntime {
    fn drop(&mut self) {
        let _ = self.client.close();
        let _ = self.join_runtime();
    }
}

/// Terminal runtime startup and command error.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// PTY creation or I/O setup failed.
    #[error("PTY error: {0}")]
    Pty(String),
    /// Child process startup failed.
    #[error("failed to spawn terminal child: {0}")]
    Spawn(String),
    /// A worker thread could not be created.
    #[error("failed to create terminal worker: {0}")]
    Thread(String),
    /// A runtime worker panicked.
    #[error("terminal runtime worker panicked")]
    ThreadPanic,
    /// An internally-owned PTY component was unexpectedly absent.
    #[error("terminal runtime invariant failed: {0}")]
    Invariant(&'static str),
    /// The terminal runtime stopped before the request completed.
    #[error("terminal runtime has stopped")]
    Stopped,
    /// A synchronous request exceeded the bounded response timeout.
    #[error("terminal runtime request timed out")]
    TimedOut,
    /// The event receiver mutex was poisoned.
    #[error("terminal event receiver was poisoned")]
    EventReceiverPoisoned,
}

#[derive(Debug)]
enum RuntimeMessage {
    PtyOutput(Vec<u8>),
    PtyReadFailed(String),
    PtyWriteFailed(String),
    ChildExited(portable_pty::ExitStatus),
    ChildWaitFailed(String),
    Input(TerminalInput),
    Resize {
        grid: GridSize,
        cell: CellSize,
    },
    Snapshot {
        viewport: Viewport,
        reply: SyncSender<TerminalSnapshot>,
    },
    Close,
}

#[derive(Debug)]
enum WriterMessage {
    Write(Vec<u8>),
    Close,
}

#[expect(
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    reason = "the runtime owner keeps startup, event handling, and shutdown in one auditable path"
)]
fn run_terminal(
    terminal_id: TerminalId,
    initial_size: GridSize,
    process: PtyProcess,
    messages: Receiver<RuntimeMessage>,
    message_sender: SyncSender<RuntimeMessage>,
    events: mpsc::Sender<TerminalEvent>,
    invalidation_pending: Arc<AtomicBool>,
) {
    let parts = match process.into_parts() {
        Ok(parts) => parts,
        Err(error) => {
            let _ = events.send(TerminalEvent::Failed {
                terminal_id,
                message: error.to_string(),
            });
            return;
        }
    };
    let crate::pty::PtyParts {
        master,
        reader,
        writer,
        mut child,
        mut killer,
    } = parts;
    let reader_join =
        match spawn_reader(terminal_id, reader, message_sender.clone()) {
            Ok(join) => join,
            Err(error) => {
                let _ = events.send(TerminalEvent::Failed {
                    terminal_id,
                    message: error.to_string(),
                });
                let _ = killer.kill();
                drop(master);
                let _ = child.wait();
                return;
            }
        };
    let (writer_sender, writer_receiver) = mpsc::sync_channel(WRITER_CAPACITY);
    let writer_join = match spawn_writer(
        terminal_id,
        writer,
        writer_receiver,
        message_sender.clone(),
    ) {
        Ok(join) => join,
        Err(error) => {
            let _ = events.send(TerminalEvent::Failed {
                terminal_id,
                message: error.to_string(),
            });
            let _ = killer.kill();
            drop(master);
            drop(messages);
            join_worker(reader_join);
            let _ = child.wait();
            return;
        }
    };
    let child_join =
        match spawn_child_waiter(terminal_id, child, message_sender) {
            Ok(join) => join,
            Err(error) => {
                let _ = events.send(TerminalEvent::Failed {
                    terminal_id,
                    message: error.to_string(),
                });
                let _ = killer.kill();
                drop(master);
                drop(messages);
                drop(writer_sender);
                join_worker(reader_join);
                join_worker(writer_join);
                return;
            }
        };
    let mut engine = TerminalEngine::new(terminal_id, initial_size);
    let _ = events.send(TerminalEvent::Ready(terminal_id));
    publish_invalidation(
        &events,
        &invalidation_pending,
        terminal_id,
        engine.generation(),
    );

    let mut child_exited = false;
    while let Ok(message) = messages.recv() {
        match message {
            RuntimeMessage::PtyOutput(bytes) => {
                for effect in engine.process(&bytes) {
                    handle_effect(effect, terminal_id, &writer_sender, &events);
                }
                publish_invalidation(
                    &events,
                    &invalidation_pending,
                    terminal_id,
                    engine.generation(),
                );
            }
            RuntimeMessage::PtyReadFailed(message)
            | RuntimeMessage::PtyWriteFailed(message)
            | RuntimeMessage::ChildWaitFailed(message) => {
                let _ = events.send(TerminalEvent::Failed {
                    terminal_id,
                    message,
                });
                if !child_exited {
                    let _ = killer.kill();
                }
            }
            RuntimeMessage::ChildExited(status) => {
                child_exited = true;
                let _ = events.send(TerminalEvent::Exited {
                    terminal_id,
                    status: ExitStatus {
                        code: Some(status.exit_code()),
                        success: status.success(),
                    },
                });
            }
            RuntimeMessage::Input(input) => {
                let bytes = encode_input(&input, engine.modes());
                if !bytes.is_empty()
                    && writer_sender.send(WriterMessage::Write(bytes)).is_err()
                {
                    break;
                }
            }
            RuntimeMessage::Resize { grid, cell } => {
                if master.resize(pty::pty_size(grid, cell)).is_err() {
                    let _ = events.send(TerminalEvent::Failed {
                        terminal_id,
                        message: "failed to resize PTY".into(),
                    });
                }
                engine.resize(grid);
                publish_invalidation(
                    &events,
                    &invalidation_pending,
                    terminal_id,
                    engine.generation(),
                );
            }
            RuntimeMessage::Snapshot { viewport, reply } => {
                let _ = reply.send(engine.snapshot(viewport));
            }
            RuntimeMessage::Close => break,
        }
    }

    if !child_exited {
        let _ = killer.kill();
    }
    let _ = writer_sender.try_send(WriterMessage::Close);
    drop(writer_sender);
    drop(master);
    drop(messages);
    join_worker(reader_join);
    join_worker(writer_join);
    join_worker(child_join);
}

fn spawn_reader(
    terminal_id: TerminalId,
    mut reader: Box<dyn Read + Send>,
    messages: SyncSender<RuntimeMessage>,
) -> Result<JoinHandle<()>, RuntimeError> {
    thread::Builder::new()
        .name(format!("huterm-reader-{}", terminal_id.get()))
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        if messages
                            .send(RuntimeMessage::PtyOutput(
                                buffer[..count].to_vec(),
                            ))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = messages.send(RuntimeMessage::PtyReadFailed(
                            error.to_string(),
                        ));
                        break;
                    }
                }
            }
        })
        .map_err(|error| RuntimeError::Thread(error.to_string()))
}

fn spawn_writer(
    terminal_id: TerminalId,
    mut writer: Box<dyn Write + Send>,
    messages: Receiver<WriterMessage>,
    runtime_messages: SyncSender<RuntimeMessage>,
) -> Result<JoinHandle<()>, RuntimeError> {
    thread::Builder::new()
        .name(format!("huterm-writer-{}", terminal_id.get()))
        .spawn(move || {
            while let Ok(message) = messages.recv() {
                match message {
                    WriterMessage::Write(bytes) => {
                        if let Err(error) = writer
                            .write_all(&bytes)
                            .and_then(|()| writer.flush())
                        {
                            let _ = runtime_messages.try_send(
                                RuntimeMessage::PtyWriteFailed(
                                    error.to_string(),
                                ),
                            );
                            break;
                        }
                    }
                    WriterMessage::Close => break,
                }
            }
        })
        .map_err(|error| RuntimeError::Thread(error.to_string()))
}

fn spawn_child_waiter(
    terminal_id: TerminalId,
    mut child: Box<dyn Child + Send + Sync>,
    messages: SyncSender<RuntimeMessage>,
) -> Result<JoinHandle<()>, RuntimeError> {
    thread::Builder::new()
        .name(format!("huterm-child-{}", terminal_id.get()))
        .spawn(move || match child.wait() {
            Ok(status) => {
                let _ = messages.send(RuntimeMessage::ChildExited(status));
            }
            Err(error) => {
                let _ = messages.try_send(RuntimeMessage::ChildWaitFailed(
                    error.to_string(),
                ));
            }
        })
        .map_err(|error| RuntimeError::Thread(error.to_string()))
}

fn handle_effect(
    effect: EngineEffect,
    terminal_id: TerminalId,
    writer: &SyncSender<WriterMessage>,
    events: &mpsc::Sender<TerminalEvent>,
) {
    match effect {
        EngineEffect::PtyWrite(bytes) => {
            let _ = writer.send(WriterMessage::Write(bytes));
        }
        EngineEffect::Title(title) => {
            let _ =
                events.send(TerminalEvent::TitleChanged { terminal_id, title });
        }
        EngineEffect::Bell => {
            let _ = events.send(TerminalEvent::Bell(terminal_id));
        }
    }
}

fn publish_invalidation(
    events: &mpsc::Sender<TerminalEvent>,
    pending: &AtomicBool,
    terminal_id: TerminalId,
    generation: u64,
) {
    if pending
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let _ = events.send(TerminalEvent::Invalidated {
            terminal_id,
            generation,
        });
    }
}

fn join_worker(worker: JoinHandle<()>) {
    let _ = worker.join();
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

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
        }
    }

    fn wait_for_text(client: &RuntimeClient, needle: &str) -> TerminalSnapshot {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let snapshot = client
                .read_snapshot(Viewport::default())
                .expect("snapshot should work");
            let text: String = snapshot
                .cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect();
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
            .read_snapshot(Viewport::default())
            .expect("final snapshot should remain available after exit");
        assert!(final_snapshot.generation >= snapshot.generation);
        thread::sleep(Duration::from_millis(25));
        while let Some(event) = client
            .try_recv_event()
            .expect("event receiver should remain available")
        {
            assert!(
                !matches!(event, TerminalEvent::Failed { .. }),
                "normal child exit should not report a runtime failure"
            );
        }
        runtime.shutdown().expect("runtime should stop cleanly");
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
            let snapshot = client
                .read_snapshot(Viewport::default())
                .expect("snapshot should work");
            if snapshot.size == GridSize::clamped(52, 13) {
                break snapshot;
            }
            assert!(Instant::now() < deadline, "terminal grid did not resize");
        };

        assert_eq!(snapshot.cells.len(), 52 * 13);
        runtime.shutdown().expect("runtime should stop cleanly");
    }

    #[test]
    fn pty_runtime_should_fail_cleanly_for_missing_program() {
        let mut missing = command("");
        missing.program =
            PathBuf::from("/definitely/missing/huterm-test-program");

        let error = TerminalRuntime::spawn(TerminalId::new(13), &missing)
            .expect_err("missing executable should fail");

        assert!(matches!(error, RuntimeError::Spawn(_)));
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
    fn shutdown_should_terminate_a_live_child_promptly() {
        let runtime = TerminalRuntime::spawn(
            TerminalId::new(15),
            &command("printf READY; sleep 30"),
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
}
