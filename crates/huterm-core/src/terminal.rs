use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{
    self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError,
    TrySendError,
};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use huterm_protocol::{
    BufferRange, CellSize, ExitStatus, GridSize, TerminalCommand,
    TerminalEvent, TerminalId, TerminalInput, TerminalSnapshot, Viewport,
};
use thiserror::Error;

use crate::engine::{EngineEffect, TerminalEngine};
use crate::input::encode_input;
use crate::pty::{self, PtyProcess};

const MESSAGE_CAPACITY: usize = 64;
const WRITER_CAPACITY: usize = 64;
const INPUT_BYTE_CAPACITY: usize = 1024 * 1024;
const RUNTIME_POLL_INTERVAL: Duration = Duration::from_millis(2);
const SNAPSHOT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

/// In-process client handle for one terminal runtime.
#[derive(Clone, Debug)]
pub struct RuntimeClient {
    terminal_id: TerminalId,
    messages: SyncSender<RuntimeMessage>,
    controls: Sender<RuntimeControl>,
    closing: Arc<AtomicBool>,
    queued_input_bytes: Arc<AtomicUsize>,
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
        let reserved_bytes = input_bytes(&input).max(1);
        if reserved_bytes > INPUT_BYTE_CAPACITY
            || self
                .queued_input_bytes
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                    queued
                        .checked_add(reserved_bytes)
                        .filter(|total| *total <= INPUT_BYTE_CAPACITY)
                })
                .is_err()
        {
            return Err(RuntimeError::Busy);
        }
        match self.messages.try_send(RuntimeMessage::Input {
            input,
            reserved_bytes,
        }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.queued_input_bytes
                    .fetch_sub(reserved_bytes, Ordering::AcqRel);
                Err(RuntimeError::Busy)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.queued_input_bytes
                    .fetch_sub(reserved_bytes, Ordering::AcqRel);
                Err(RuntimeError::Stopped)
            }
        }
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
        match self.messages.try_send(RuntimeMessage::Resize {
            grid: GridSize::clamped(grid.columns, grid.rows),
            cell,
        }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(RuntimeError::Busy),
            Err(TrySendError::Disconnected(_)) => Err(RuntimeError::Stopped),
        }
    }

    /// Requests an immutable snapshot without waiting for the runtime owner.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal has stopped.
    pub fn request_snapshot(
        &self,
        viewport: Viewport,
    ) -> Result<SnapshotRequest, RuntimeError> {
        let (reply, receiver) = async_channel::bounded(1);
        self.controls
            .send(RuntimeControl::Snapshot { viewport, reply })
            .map_err(|_| RuntimeError::Stopped)?;
        Ok(SnapshotRequest { receiver })
    }

    /// Requests text extraction from canonical scrollback without blocking.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal has stopped.
    pub fn request_selection(
        &self,
        generation: u64,
        range: BufferRange,
    ) -> Result<SelectionRequest, RuntimeError> {
        let (reply, receiver) = async_channel::bounded(1);
        self.controls
            .send(RuntimeControl::Selection {
                generation,
                range,
                reply,
            })
            .map_err(|_| RuntimeError::Stopped)?;
        Ok(SelectionRequest { receiver })
    }

    /// Reads an immutable snapshot for a client-owned viewport.
    ///
    /// This blocking convenience is intended for worker threads and tests.
    /// Interactive clients should await [`Self::request_snapshot`] instead.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime is unavailable or does not answer.
    pub fn read_snapshot(
        &self,
        viewport: Viewport,
    ) -> Result<TerminalSnapshot, RuntimeError> {
        self.request_snapshot(viewport)?
            .recv_blocking()
            .map(|reply| reply.snapshot)
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
        self.closing.store(true, Ordering::Release);
        self.controls
            .send(RuntimeControl::Wake)
            .map_err(|_| RuntimeError::Stopped)
    }
}

/// Pending asynchronous snapshot response.
#[derive(Debug)]
pub struct SnapshotRequest {
    receiver: async_channel::Receiver<SnapshotReply>,
}

/// A runtime snapshot and the elapsed time spent producing it.
///
/// Timing stays in the in-process client boundary so performance diagnostics do
/// not leak into the dependency-neutral wire protocol.
#[derive(Debug)]
pub struct SnapshotReply {
    /// Immutable terminal snapshot.
    pub snapshot: TerminalSnapshot,
    /// Monotonic wall-clock duration of snapshot construction, including any
    /// scheduler preemption. Excludes request queueing and response delivery.
    pub snapshot_duration: Duration,
    /// Monotonic instant when snapshot construction completed.
    pub completed_at: Instant,
}

impl SnapshotRequest {
    /// Polls the response without blocking the caller.
    ///
    /// # Errors
    ///
    /// Returns an error if the runtime stops before replying.
    pub fn try_recv(&self) -> Result<Option<SnapshotReply>, RuntimeError> {
        match self.receiver.try_recv() {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(async_channel::TryRecvError::Empty) => Ok(None),
            Err(async_channel::TryRecvError::Closed) => {
                Err(RuntimeError::Stopped)
            }
        }
    }

    /// Waits asynchronously for the runtime response.
    ///
    /// # Errors
    ///
    /// Returns an error if the runtime stops before replying.
    pub async fn recv(self) -> Result<SnapshotReply, RuntimeError> {
        self.receiver
            .recv()
            .await
            .map_err(|_| RuntimeError::Stopped)
    }

    fn recv_blocking(self) -> Result<SnapshotReply, RuntimeError> {
        self.recv_blocking_with_timeout(SNAPSHOT_RESPONSE_TIMEOUT)
    }

    fn recv_blocking_with_timeout(
        self,
        timeout: Duration,
    ) -> Result<SnapshotReply, RuntimeError> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.receiver.try_recv() {
                Ok(reply) => return Ok(reply),
                Err(async_channel::TryRecvError::Closed) => {
                    return Err(RuntimeError::Stopped);
                }
                Err(async_channel::TryRecvError::Empty) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(RuntimeError::TimedOut);
                    }
                    thread::park_timeout(
                        deadline
                            .saturating_duration_since(now)
                            .min(Duration::from_millis(1)),
                    );
                }
            }
        }
    }
}

/// Pending asynchronous selection extraction response.
#[derive(Debug)]
pub struct SelectionRequest {
    receiver: async_channel::Receiver<Option<String>>,
}

impl SelectionRequest {
    /// Waits asynchronously for extracted text.
    ///
    /// A successful `None` means the terminal generation or range was stale.
    ///
    /// # Errors
    ///
    /// Returns an error if the runtime stops before replying.
    pub async fn recv(self) -> Result<Option<String>, RuntimeError> {
        self.receiver
            .recv()
            .await
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
        let (control_sender, control_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        let invalidation_pending = Arc::new(AtomicBool::new(false));
        let runtime_pending = Arc::clone(&invalidation_pending);
        let closing = Arc::new(AtomicBool::new(false));
        let queued_input_bytes = Arc::new(AtomicUsize::new(0));
        let initial_size = command.grid_size;
        let runtime_sender = message_sender.clone();
        let runtime_controls = control_sender.clone();
        let runtime_closing = Arc::clone(&closing);
        let runtime_input_bytes = Arc::clone(&queued_input_bytes);
        let join = thread::Builder::new()
            .name(format!("huterm-runtime-{}", terminal_id.get()))
            .spawn(move || {
                run_terminal(
                    terminal_id,
                    initial_size,
                    process,
                    message_receiver,
                    runtime_sender,
                    control_receiver,
                    runtime_controls,
                    event_sender,
                    runtime_pending,
                    runtime_closing,
                    runtime_input_bytes,
                );
            })
            .map_err(|error| RuntimeError::Thread(error.to_string()))?;

        let client = RuntimeClient {
            terminal_id,
            messages: message_sender,
            controls: control_sender,
            closing,
            queued_input_bytes,
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
    /// The bounded runtime ingress queue cannot accept more work yet.
    #[error("terminal runtime is busy")]
    Busy,
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
    Input {
        input: TerminalInput,
        reserved_bytes: usize,
    },
    Resize {
        grid: GridSize,
        cell: CellSize,
    },
}

#[derive(Debug)]
enum RuntimeControl {
    Snapshot {
        viewport: Viewport,
        reply: async_channel::Sender<SnapshotReply>,
    },
    Selection {
        generation: u64,
        range: BufferRange,
        reply: async_channel::Sender<Option<String>>,
    },
    WorkerFailed(String),
    Wake,
}

#[derive(Debug)]
enum WriterMessage {
    Write(Vec<u8>),
}

#[expect(
    clippy::needless_pass_by_value,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the runtime owner keeps startup, event handling, and shutdown in one auditable path"
)]
fn run_terminal(
    terminal_id: TerminalId,
    initial_size: GridSize,
    process: PtyProcess,
    messages: Receiver<RuntimeMessage>,
    message_sender: SyncSender<RuntimeMessage>,
    controls: Receiver<RuntimeControl>,
    control_sender: Sender<RuntimeControl>,
    events: mpsc::Sender<TerminalEvent>,
    invalidation_pending: Arc<AtomicBool>,
    closing: Arc<AtomicBool>,
    queued_input_bytes: Arc<AtomicUsize>,
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
        reader_waiter,
        writer,
        mut child,
        mut killer,
    } = parts;
    let mut process_groups = pty::process_groups(child.process_id());
    pty::record_foreground_group(master.as_ref(), &mut process_groups);
    let reader_join = match spawn_reader(
        terminal_id,
        reader,
        reader_waiter,
        message_sender.clone(),
        control_sender.clone(),
        Arc::clone(&closing),
    ) {
        Ok(join) => join,
        Err(error) => {
            report_failure(&events, terminal_id, error.to_string());
            pty::record_foreground_group(master.as_ref(), &mut process_groups);
            pty::terminate_child(
                child.as_mut(),
                killer.as_mut(),
                &process_groups,
            );
            return;
        }
    };
    let (writer_sender, writer_receiver) = mpsc::sync_channel(WRITER_CAPACITY);
    let writer_join = match spawn_writer(
        terminal_id,
        writer,
        writer_receiver,
        control_sender,
        Arc::clone(&closing),
    ) {
        Ok(join) => join,
        Err(error) => {
            report_failure(&events, terminal_id, error.to_string());
            closing.store(true, Ordering::Release);
            pty::record_foreground_group(master.as_ref(), &mut process_groups);
            pty::terminate_child(
                child.as_mut(),
                killer.as_mut(),
                &process_groups,
            );
            drop(master);
            drop(messages);
            join_worker(reader_join);
            return;
        }
    };
    drop(message_sender);
    let mut engine = TerminalEngine::new(terminal_id, initial_size);
    let _ = events.send(TerminalEvent::Ready(terminal_id));
    publish_invalidation(
        &events,
        &invalidation_pending,
        terminal_id,
        engine.generation(),
    );

    let mut child_exited = false;
    let mut pending_writes = VecDeque::new();
    while !closing.load(Ordering::Acquire) {
        pty::record_foreground_group(master.as_ref(), &mut process_groups);
        if !child_exited {
            match child.try_wait() {
                Ok(Some(status)) => {
                    child_exited = true;
                    let _ = events.send(TerminalEvent::Exited {
                        terminal_id,
                        status: ExitStatus {
                            code: Some(status.exit_code()),
                            success: status.success(),
                        },
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    report_failure(&events, terminal_id, error.to_string());
                    closing.store(true, Ordering::Release);
                }
            }
        }
        while let Ok(control) = controls.try_recv() {
            if closing.load(Ordering::Acquire) {
                break;
            }
            match control {
                RuntimeControl::Snapshot { viewport, reply } => {
                    let started = Instant::now();
                    let snapshot = engine.snapshot(viewport);
                    let snapshot_duration = started.elapsed();
                    let _ = reply.try_send(SnapshotReply {
                        snapshot,
                        snapshot_duration,
                        completed_at: Instant::now(),
                    });
                }
                RuntimeControl::Selection {
                    generation,
                    range,
                    reply,
                } => {
                    let _ =
                        reply.try_send(engine.extract_text(generation, range));
                }
                RuntimeControl::WorkerFailed(message) => {
                    report_failure(&events, terminal_id, message);
                    closing.store(true, Ordering::Release);
                }
                RuntimeControl::Wake => {}
            }
        }
        if closing.load(Ordering::Acquire) {
            break;
        }
        match flush_pending_write(&writer_sender, &mut pending_writes) {
            WriterQueueState::Drained => {}
            WriterQueueState::Full => {
                thread::sleep(RUNTIME_POLL_INTERVAL);
                continue;
            }
            WriterQueueState::Disconnected => {
                report_failure(
                    &events,
                    terminal_id,
                    "PTY writer stopped before queued input was written".into(),
                );
                closing.store(true, Ordering::Release);
                continue;
            }
        }
        let message = match messages.recv_timeout(RUNTIME_POLL_INTERVAL) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match message {
            RuntimeMessage::PtyOutput(bytes) => {
                for effect in engine.process(&bytes) {
                    if handle_effect(
                        effect,
                        terminal_id,
                        &writer_sender,
                        &mut pending_writes,
                        &events,
                    ) == WriterQueueState::Disconnected
                    {
                        report_failure(
                            &events,
                            terminal_id,
                            "PTY writer stopped before a terminal reply was written"
                                .into(),
                        );
                        closing.store(true, Ordering::Release);
                        break;
                    }
                }
                publish_invalidation(
                    &events,
                    &invalidation_pending,
                    terminal_id,
                    engine.generation(),
                );
            }
            RuntimeMessage::Input {
                input,
                reserved_bytes,
            } => {
                queued_input_bytes.fetch_sub(reserved_bytes, Ordering::AcqRel);
                let bytes = encode_input(&input, engine.modes(), engine.size());
                if !bytes.is_empty()
                    && queue_write(bytes, &writer_sender, &mut pending_writes)
                        == WriterQueueState::Disconnected
                {
                    report_failure(
                        &events,
                        terminal_id,
                        "PTY writer stopped before input was written".into(),
                    );
                    closing.store(true, Ordering::Release);
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
        }
    }

    closing.store(true, Ordering::Release);
    pty::record_foreground_group(master.as_ref(), &mut process_groups);
    pty::terminate_child(child.as_mut(), killer.as_mut(), &process_groups);
    drop(writer_sender);
    drop(master);
    drop(messages);
    join_worker(reader_join);
    join_worker(writer_join);
}

fn spawn_reader(
    terminal_id: TerminalId,
    mut reader: Box<dyn Read + Send>,
    reader_waiter: pty::ReaderWaiter,
    messages: SyncSender<RuntimeMessage>,
    controls: Sender<RuntimeControl>,
    closing: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, RuntimeError> {
    thread::Builder::new()
        .name(format!("huterm-reader-{}", terminal_id.get()))
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            let mut pending = None;
            while !closing.load(Ordering::Acquire) {
                if let Some(bytes) = pending.take() {
                    match messages.try_send(RuntimeMessage::PtyOutput(bytes)) {
                        Ok(()) => continue,
                        Err(TrySendError::Full(RuntimeMessage::PtyOutput(
                            bytes,
                        ))) => {
                            pending = Some(bytes);
                            thread::sleep(RUNTIME_POLL_INTERVAL);
                            continue;
                        }
                        Err(TrySendError::Disconnected(_)) => break,
                        Err(TrySendError::Full(_)) => unreachable!(
                            "reader only sends PTY output messages"
                        ),
                    }
                }
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        pending = Some(buffer[..count].to_vec());
                    }
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        if let Err(error) =
                            reader_waiter.wait(RUNTIME_POLL_INTERVAL)
                        {
                            let _ = controls.send(
                                RuntimeControl::WorkerFailed(format!(
                                    "PTY readiness wait failed: {error}"
                                )),
                            );
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = controls.send(RuntimeControl::WorkerFailed(
                            format!("PTY read failed: {error}"),
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
    controls: Sender<RuntimeControl>,
    closing: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, RuntimeError> {
    thread::Builder::new()
        .name(format!("huterm-writer-{}", terminal_id.get()))
        .spawn(move || {
            let mut current: Option<(Vec<u8>, usize)> = None;
            while !closing.load(Ordering::Acquire) {
                if current.is_none() {
                    current = match messages.recv_timeout(RUNTIME_POLL_INTERVAL)
                    {
                        Ok(WriterMessage::Write(bytes)) => Some((bytes, 0)),
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                }
                let Some((bytes, offset)) = current.as_mut() else {
                    continue;
                };
                if *offset == bytes.len() {
                    match writer.flush() {
                        Ok(()) => current = None,
                        Err(error)
                            if error.kind()
                                == std::io::ErrorKind::WouldBlock =>
                        {
                            thread::sleep(RUNTIME_POLL_INTERVAL);
                        }
                        Err(error) => {
                            let _ =
                                controls.send(RuntimeControl::WorkerFailed(
                                    format!("PTY flush failed: {error}"),
                                ));
                            break;
                        }
                    }
                    continue;
                }
                match writer.write(&bytes[*offset..]) {
                    Ok(0) => {
                        let _ = controls.send(RuntimeControl::WorkerFailed(
                            "PTY write returned zero bytes".into(),
                        ));
                        break;
                    }
                    Ok(count) => *offset += count,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        thread::sleep(RUNTIME_POLL_INTERVAL);
                    }
                    Err(error) => {
                        let _ = controls.send(RuntimeControl::WorkerFailed(
                            format!("PTY write failed: {error}"),
                        ));
                        break;
                    }
                }
            }
        })
        .map_err(|error| RuntimeError::Thread(error.to_string()))
}

fn handle_effect(
    effect: EngineEffect,
    terminal_id: TerminalId,
    writer: &SyncSender<WriterMessage>,
    pending_writes: &mut VecDeque<Vec<u8>>,
    events: &mpsc::Sender<TerminalEvent>,
) -> WriterQueueState {
    match effect {
        EngineEffect::PtyWrite(bytes) => {
            queue_write(bytes, writer, pending_writes)
        }
        EngineEffect::Title(title) => {
            let _ =
                events.send(TerminalEvent::TitleChanged { terminal_id, title });
            WriterQueueState::Drained
        }
        EngineEffect::Bell => {
            let _ = events.send(TerminalEvent::Bell(terminal_id));
            WriterQueueState::Drained
        }
    }
}

fn queue_write(
    bytes: Vec<u8>,
    writer: &SyncSender<WriterMessage>,
    pending: &mut VecDeque<Vec<u8>>,
) -> WriterQueueState {
    if !pending.is_empty() {
        pending.push_back(bytes);
        return WriterQueueState::Full;
    }
    match writer.try_send(WriterMessage::Write(bytes)) {
        Ok(()) => WriterQueueState::Drained,
        Err(TrySendError::Full(WriterMessage::Write(bytes))) => {
            pending.push_back(bytes);
            WriterQueueState::Full
        }
        Err(TrySendError::Disconnected(_)) => WriterQueueState::Disconnected,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WriterQueueState {
    Drained,
    Full,
    Disconnected,
}

fn flush_pending_write(
    writer: &SyncSender<WriterMessage>,
    pending: &mut VecDeque<Vec<u8>>,
) -> WriterQueueState {
    while let Some(bytes) = pending.pop_front() {
        match writer.try_send(WriterMessage::Write(bytes)) {
            Ok(()) => {}
            Err(TrySendError::Full(WriterMessage::Write(bytes))) => {
                pending.push_front(bytes);
                return WriterQueueState::Full;
            }
            Err(TrySendError::Disconnected(_)) => {
                return WriterQueueState::Disconnected;
            }
        }
    }
    WriterQueueState::Drained
}

fn report_failure(
    events: &mpsc::Sender<TerminalEvent>,
    terminal_id: TerminalId,
    message: String,
) {
    let _ = events.send(TerminalEvent::Failed {
        terminal_id,
        message,
    });
}

fn input_bytes(input: &TerminalInput) -> usize {
    match input {
        TerminalInput::Text(text) | TerminalInput::Paste(text) => text.len(),
        TerminalInput::Key { .. } | TerminalInput::Focus(_) => {
            std::mem::size_of::<TerminalInput>()
        }
        _ => std::mem::size_of::<TerminalInput>(),
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
    use std::io;
    use std::path::PathBuf;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn pty_mouse_reports_preserve_keyboard_order_and_disable_silence() {
        use huterm_protocol::{
            Modifiers, MouseAction, MouseButton, MouseInput, MousePosition,
            MouseTracking,
        };
        let expected = b"\x1b[<0;2;3MK\x1b[<0;2;3m";
        let script = format!(
            "stty raw -echo; printf '\\033[?1000h\\033[?1006hREADY'; bytes=$(dd bs=1 count={} 2>/dev/null | od -An -tx1 | tr -d ' \\n'); printf 'HEX:%s:DONE\\033[?1000lDISABLED' \"$bytes\"; byte=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\n'); printf 'SILENT:%s:END' \"$byte\"",
            expected.len(),
        );
        let runtime =
            TerminalRuntime::spawn(TerminalId::new(91), &command(&script))
                .unwrap();
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
        let text: String = disabled
            .cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
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

    #[cfg(unix)]
    #[test]
    fn pty_runtime_should_process_bursty_output_without_poll_delay() {
        let started = Instant::now();
        let runtime = TerminalRuntime::spawn(
            TerminalId::new(17),
            &command(
                "dd if=/dev/zero bs=1048576 count=1 2>/dev/null; printf DONE",
            ),
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
    fn shutdown_should_escalate_for_a_hup_resistant_child() {
        let runtime = TerminalRuntime::spawn(
            TerminalId::new(15),
            &command(
                "trap '' HUP TERM; printf READY; while :; do sleep 30; done",
            ),
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
            &command(
                "trap '' HUP TERM; printf READY; while :; do sleep 30; done",
            ),
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
    fn pending_writer_spill_should_drain_fifo_until_full() {
        let (sender, receiver) = mpsc::sync_channel(2);
        let mut pending = VecDeque::from([
            b"one".to_vec(),
            b"two".to_vec(),
            b"three".to_vec(),
        ]);

        assert_eq!(
            flush_pending_write(&sender, &mut pending),
            WriterQueueState::Full
        );
        assert_eq!(pending, VecDeque::from([b"three".to_vec()]));
        let receive = || match receiver.try_recv().expect("write should exist")
        {
            WriterMessage::Write(bytes) => bytes,
        };
        assert_eq!((receive(), receive()), (b"one".to_vec(), b"two".to_vec()));

        assert_eq!(
            flush_pending_write(&sender, &mut pending),
            WriterQueueState::Drained
        );
        assert_eq!(receive(), b"three".to_vec());
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
    fn writer_should_resume_after_partial_would_block_without_repeating_bytes()
    {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = PartialWouldBlockWriter {
            bytes: Arc::clone(&bytes),
            block_next: false,
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        let (controls, _control_receiver) = mpsc::channel();
        let closing = Arc::new(AtomicBool::new(false));
        let join = spawn_writer(
            TerminalId::new(18),
            Box::new(writer),
            receiver,
            controls,
            Arc::clone(&closing),
        )
        .expect("writer worker should start");
        sender
            .send(WriterMessage::Write(b"abcdef".to_vec()))
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
}
