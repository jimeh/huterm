use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::{self, AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{
    self, Receiver, Sender, SyncSender, TryRecvError, TrySendError,
};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(test)]
use huterm_protocol::TerminalPresentation;
use huterm_protocol::{
    BufferRange, CellSize, ExitStatus, GridSize, ScrollCommand,
    TerminalCommand, TerminalEvent, TerminalId, TerminalInput,
    TerminalMetadata, TerminalSnapshot,
};
use thiserror::Error;

use crate::engine::{EngineEffect, TerminalEngine};
use crate::events::{EventPublisher, EventReceiver};
use crate::foreground::{ProbeSchedule, Reports};
use crate::host_effects::HostEffectSink;
use crate::input::encode_input;
use crate::presentation::PresentationUpdate;
use crate::pty::{self, PtyProcess};

const MESSAGE_CAPACITY: usize = 64;
/// Largest PTY output message: one read plus output the PTY already holds.
const READ_BATCH_BYTES: usize = 16 * 1024;
/// Queued output messages, bounding output read ahead of the parser to 1 MiB.
/// The bound counts messages, and macOS PTY reads rarely batch past 1 KiB, so
/// fewer slots would leave too little buffer to cover a snapshot build.
const OUTPUT_CAPACITY: usize = 64;
const WRITER_CAPACITY: usize = 64;
const INPUT_BYTE_CAPACITY: usize = 1024 * 1024;
const SNAPSHOT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

/// In-process client handle for one terminal runtime.
#[derive(Clone, Debug)]
pub struct RuntimeClient {
    terminal_id: TerminalId,
    messages: crate::wake::SyncSender<RuntimeMessage>,
    #[cfg(test)]
    output: crate::wake::SyncSender<Vec<u8>>,
    controls: crate::wake::Sender<RuntimeControl>,
    closing: Arc<AtomicBool>,
    queued_input_bytes: Arc<AtomicUsize>,
    events: Arc<Mutex<EventReceiver>>,
    activity: async_channel::Receiver<()>,
    shutdown_groups: Arc<Mutex<Vec<i32>>>,
    host_effect_sink: HostEffectSink,
}

impl RuntimeClient {
    /// Returns the terminal addressed by this client.
    #[must_use]
    pub fn terminal_id(&self) -> TerminalId {
        self.terminal_id
    }

    /// Returns the immutable engine version used by this terminal.
    #[must_use]
    pub fn engine_revision(&self) -> &'static str {
        crate::GHOSTTY_REVISION
    }

    /// Sends structured input using emulator modes owned by the runtime.
    /// Input queued after observed child exit is discarded; history remains readable.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal has stopped.
    pub fn send_input(&self, input: TerminalInput) -> Result<(), RuntimeError> {
        self.offer_input(input).map_err(|refused| refused.error)
    }

    /// Sends input like [`Self::send_input`], but returns refused input to
    /// the caller, which can then queue it for retry without copying it first.
    ///
    /// # Errors
    ///
    /// Returns the input with [`RuntimeError::Busy`] when the runtime cannot
    /// accept it yet, or with [`RuntimeError::Stopped`] when the terminal has
    /// stopped.
    pub fn offer_input(
        &self,
        input: TerminalInput,
    ) -> Result<(), RefusedInput> {
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
            return Err(RefusedInput {
                error: RuntimeError::Busy,
                input,
            });
        }
        let (error, message) =
            match self.messages.try_send(RuntimeMessage::Input {
                input,
                reserved_bytes,
            }) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(message)) => {
                    (RuntimeError::Busy, message)
                }
                Err(TrySendError::Disconnected(message)) => {
                    (RuntimeError::Stopped, message)
                }
            };
        self.queued_input_bytes
            .fetch_sub(reserved_bytes, Ordering::AcqRel);
        let RuntimeMessage::Input { input, .. } = message else {
            unreachable!("the refused message is the input just sent");
        };
        Err(RefusedInput { error, input })
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

    /// Erases the scrollback and keeps the screen, after earlier input.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal has stopped or its client queue
    /// is full.
    pub fn clear_history(&self) -> Result<(), RuntimeError> {
        self.send_edit(BufferEdit::ClearHistory)
    }

    /// Resets the emulator as RIS does, after earlier input. The PTY and
    /// its processes are unaffected.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal has stopped or its client queue
    /// is full.
    pub fn reset(&self) -> Result<(), RuntimeError> {
        self.send_edit(BufferEdit::Reset)
    }

    fn send_edit(&self, edit: BufferEdit) -> Result<(), RuntimeError> {
        match self.messages.try_send(RuntimeMessage::Edit(edit)) {
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
    pub fn request_snapshot(&self) -> Result<SnapshotRequest, RuntimeError> {
        self.snapshot_request(None, None)
    }

    /// Moves the shared viewport and reads it atomically on the runtime owner.
    ///
    /// # Errors
    /// Returns an error when the terminal has stopped.
    pub fn request_scrolled_snapshot(
        &self,
        scroll: ScrollCommand,
    ) -> Result<SnapshotRequest, RuntimeError> {
        self.snapshot_request(Some(scroll), None)
    }

    /// Requests a snapshot and optional link lookup in one engine-owner operation.
    ///
    /// # Errors
    /// Returns an error when the terminal has stopped. Lookup failures remain
    /// nonfatal outcomes in the successful snapshot reply.
    pub fn request_snapshot_with_link(
        &self,
        scroll: Option<ScrollCommand>,
        point: Option<huterm_protocol::MousePosition>,
    ) -> Result<SnapshotRequest, RuntimeError> {
        self.snapshot_request(scroll, point)
    }

    fn snapshot_request(
        &self,
        scroll: Option<ScrollCommand>,
        point: Option<huterm_protocol::MousePosition>,
    ) -> Result<SnapshotRequest, RuntimeError> {
        let (reply, receiver) = async_channel::bounded(1);
        self.controls
            .send(RuntimeControl::Snapshot {
                scroll,
                point,
                reply,
                #[cfg(test)]
                fail_lookup: false,
            })
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

    /// Reads an immutable snapshot of the shared terminal viewport.
    ///
    /// This blocking convenience is intended for worker threads and tests.
    /// Interactive clients should await [`Self::request_snapshot`] instead.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime is unavailable or does not answer.
    pub fn read_snapshot(&self) -> Result<TerminalSnapshot, RuntimeError> {
        self.request_snapshot()?
            .recv_blocking()
            .map(|reply| reply.snapshot)
    }

    /// Receives the next queued runtime event without blocking.
    ///
    /// Pending titles, metadata, invalidations, and bells coalesce to their
    /// latest state. Lifecycle transitions remain observable under a flood.
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
        Ok(event)
    }

    /// Completes once the runtime has published events since the last call.
    ///
    /// The signal carries no data and coalesces: drain events with
    /// [`Self::try_recv_event`]. Clones share one signal, so give it a single
    /// waiter.
    ///
    /// # Errors
    /// Returns an error when the terminal has stopped.
    pub async fn wait_for_activity(&self) -> Result<(), RuntimeError> {
        self.activity
            .recv()
            .await
            .map_err(|_| RuntimeError::Stopped)
    }

    /// Checks whether the PTY has a foreground job other than its shell.
    ///
    /// # Errors
    /// Returns an error if the terminal stops before answering.
    pub async fn has_foreground_job(&self) -> Result<bool, RuntimeError> {
        let (reply, receiver) = async_channel::bounded(1);
        self.controls
            .send(RuntimeControl::ForegroundJob(reply))
            .map_err(|_| RuntimeError::Stopped)?;
        receiver.recv().await.map_err(|_| RuntimeError::Stopped)
    }

    pub(crate) fn record_shutdown_groups(&self, groups: Vec<i32>) {
        *self
            .shutdown_groups
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = groups;
    }

    pub(crate) fn job_context(&self) -> Option<crate::jobs::JobContext> {
        let receiver = self.request_job_context()?;
        receiver.recv_timeout(Duration::from_secs(1)).ok()
    }

    pub(crate) fn request_job_context(
        &self,
    ) -> Option<Receiver<crate::jobs::JobContext>> {
        let (reply, receiver) = mpsc::channel();
        self.controls.send(RuntimeControl::JobContext(reply)).ok()?;
        Some(receiver)
    }

    #[cfg(test)]
    pub(crate) fn presentation(
        &self,
    ) -> Option<(TerminalPresentation, GridSize, CellSize)> {
        let (reply, receiver) = mpsc::channel();
        self.controls
            .send(RuntimeControl::Presentation(reply))
            .ok()?;
        receiver.recv_timeout(Duration::from_secs(1)).ok()
    }

    pub(crate) fn host_effect_sink(&self) -> HostEffectSink {
        self.host_effect_sink.clone()
    }

    pub(crate) fn update_presentation(
        &self,
        update: PresentationUpdate,
    ) -> Result<(), RuntimeError> {
        match self
            .messages
            .try_send(RuntimeMessage::Presentation(Box::new(update)))
        {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(RuntimeError::Busy),
            Err(TrySendError::Disconnected(_)) => Err(RuntimeError::Stopped),
        }
    }

    /// Requests orderly terminal shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error when the terminal has already stopped.
    pub fn close(&self) -> Result<(), RuntimeError> {
        self.host_effect_sink.close();
        self.closing.store(true, Ordering::Release);
        self.controls
            .send(RuntimeControl::Wake)
            .map_err(|_| RuntimeError::Stopped)
    }
}

/// Pending asynchronous snapshot response.
#[derive(Debug)]
pub struct SnapshotRequest {
    receiver: async_channel::Receiver<Result<SnapshotReply, RuntimeError>>,
}

/// A runtime snapshot and the elapsed time spent producing it.
///
/// Timing stays in the in-process client boundary so performance diagnostics do
/// not leak into the dependency-neutral wire protocol.
#[derive(Debug)]
pub struct SnapshotReply {
    /// Immutable terminal snapshot.
    pub snapshot: TerminalSnapshot,
    /// Optional link outcome resolved against this exact snapshot.
    pub link: Option<huterm_protocol::LinkLookup>,
    /// Elapsed owner-thread time spent on optional link inspection.
    pub lookup_duration: Duration,
    /// Expected viewport computed from the command and runtime state before it runs.
    pub requested_viewport: huterm_protocol::Viewport,
    /// Monotonic wall-clock duration of snapshot construction, including any
    /// scheduler preemption. Excludes request queueing and response delivery.
    pub snapshot_duration: Duration,
    /// Monotonic instant when snapshot construction completed.
    pub completed_at: Instant,
    /// Monotonic instant of the earliest invalidation this snapshot is the
    /// first to include. `None` when nothing changed since the last snapshot.
    pub invalidated_at: Option<Instant>,
}

impl SnapshotRequest {
    /// Polls the response without blocking the caller.
    ///
    /// # Errors
    ///
    /// Returns an error if the runtime stops before replying.
    pub fn try_recv(&self) -> Result<Option<SnapshotReply>, RuntimeError> {
        match self.receiver.try_recv() {
            Ok(snapshot) => snapshot.map(Some),
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
            .map_err(|_| RuntimeError::Stopped)?
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
                Ok(reply) => return reply,
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
    receiver: async_channel::Receiver<Result<Option<String>, RuntimeError>>,
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
            .map_err(|_| RuntimeError::Stopped)?
    }
}

/// Owner of one terminal's runtime thread and cleanup path.
#[derive(Debug)]
pub struct TerminalRuntime {
    client: RuntimeClient,
    join: Option<JoinHandle<Result<(), RuntimeError>>>,
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
        let command = command.clone();
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let (message_sender, message_receiver) =
            mpsc::sync_channel(MESSAGE_CAPACITY);
        let (output_sender, output_receiver) =
            mpsc::sync_channel(OUTPUT_CAPACITY);
        let (control_sender, control_receiver) = mpsc::channel();
        let wake = Arc::new(crate::wake::Wake::default());
        let message_sender =
            crate::wake::SyncSender::new(message_sender, Arc::clone(&wake));
        let output_sender =
            crate::wake::SyncSender::new(output_sender, Arc::clone(&wake));
        #[cfg(test)]
        let client_output = output_sender.clone();
        let control_sender = crate::wake::Sender::new(control_sender, wake);
        let (event_sender, event_receiver, activity) =
            EventPublisher::channel();
        let invalidation_pending = Arc::new(AtomicBool::new(false));
        let runtime_pending = Arc::clone(&invalidation_pending);
        let closing = Arc::new(AtomicBool::new(false));
        let queued_input_bytes = Arc::new(AtomicUsize::new(0));
        let host_effect_sink = HostEffectSink::new_with_activity(
            terminal_id,
            Some(Arc::downgrade(&event_sender.activity)),
        );
        let runtime_host_effect_sink = host_effect_sink.clone();

        let runtime_controls = control_sender.clone();
        let runtime_closing = Arc::clone(&closing);
        let runtime_input_bytes = Arc::clone(&queued_input_bytes);
        let shutdown_groups = Arc::new(Mutex::new(Vec::new()));
        let runtime_groups = Arc::clone(&shutdown_groups);
        let join = thread::Builder::new()
            .name(format!("huterm-runtime-{}", terminal_id.get()))
            .spawn(move || {
                let result = (|| {
                    let mut engine = TerminalEngine::new(
                        terminal_id,
                        command.grid_size,
                        command.cell_size,
                        command.presentation.clone(),
                    )?;
                    engine
                        .set_host_effect_sink(runtime_host_effect_sink.clone());
                    let process = pty::spawn(&command)?;
                    run_terminal(
                        terminal_id,
                        engine,
                        process,
                        message_receiver,
                        output_receiver,
                        output_sender,
                        control_receiver,
                        runtime_controls,
                        event_sender,
                        runtime_pending,
                        runtime_closing,
                        runtime_input_bytes,
                        runtime_groups,
                        &startup_sender,
                    )
                })();
                runtime_host_effect_sink.close();
                if let Err(error) = &result {
                    let _ = startup_sender.send(Err(error.clone()));
                }
                result
            })
            .map_err(|error| RuntimeError::Thread(error.to_string()))?;

        if let Err(error) = startup_receiver
            .recv()
            .unwrap_or(Err(RuntimeError::ThreadPanic))
        {
            let _ = join.join();
            return Err(error);
        }
        let client = RuntimeClient {
            terminal_id,
            messages: message_sender,
            #[cfg(test)]
            output: client_output,
            controls: control_sender,
            closing,
            queued_input_bytes,
            events: Arc::new(Mutex::new(event_receiver)),
            activity,
            shutdown_groups,
            host_effect_sink,
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

    pub(crate) fn host_effect_sink(&self) -> HostEffectSink {
        self.client.host_effect_sink()
    }

    /// Stops the child and joins the runtime owner.
    ///
    /// # Errors
    ///
    /// Returns an error if the runtime thread failed or its child did not exit.
    pub fn shutdown(mut self) -> Result<(), RuntimeError> {
        let _ = self.client.close();
        self.join_runtime()
    }

    fn join_runtime(&mut self) -> Result<(), RuntimeError> {
        let Some(join) = self.join.take() else {
            return Ok(());
        };
        join.join().map_err(|_| RuntimeError::ThreadPanic)?
    }
}

impl Drop for TerminalRuntime {
    fn drop(&mut self) {
        let _ = self.client.close();
        let _ = self.join_runtime();
    }
}

/// Terminal runtime startup and command error.
#[derive(Clone, Debug, Error)]
pub enum RuntimeError {
    /// Emulator initialization or operation failed.
    #[error("terminal engine error: {0}")]
    Engine(String),
    /// PTY creation or I/O setup failed.
    #[error("PTY error: {0}")]
    Pty(String),
    /// Child process startup failed.
    #[error("failed to spawn terminal child: {0}")]
    Spawn(String),
    /// A worker thread could not be created.
    #[error("failed to create terminal worker: {0}")]
    Thread(String),
    /// The child did not exit within the bounded cleanup interval.
    #[error("terminal child did not exit after shutdown")]
    ShutdownTimedOut,
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

/// Input a runtime did not accept, returned to its sender.
#[derive(Debug)]
pub struct RefusedInput {
    /// Why the runtime refused the input.
    pub error: RuntimeError,
    /// The input, unchanged.
    pub input: TerminalInput,
}

/// Ordered client requests. Their queue is separate from PTY output so a
/// flood cannot refuse input.
#[derive(Debug)]
enum RuntimeMessage {
    Input {
        input: TerminalInput,
        reserved_bytes: usize,
    },
    Resize {
        grid: GridSize,
        cell: CellSize,
    },
    Presentation(Box<PresentationUpdate>),
    Edit(BufferEdit),
}

/// Client changes to emulator state that bypass the PTY.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BufferEdit {
    ClearHistory,
    Reset,
}

#[derive(Debug)]
enum RuntimeControl {
    ForegroundJob(async_channel::Sender<bool>),
    JobContext(Sender<crate::jobs::JobContext>),
    #[cfg(test)]
    Presentation(Sender<(TerminalPresentation, GridSize, CellSize)>),
    /// Reports whether a foreground probe is armed.
    #[cfg(test)]
    ProbeArmed(Sender<bool>),
    /// Holds the runtime owner until the test releases it.
    #[cfg(test)]
    Pause {
        entered: Sender<()>,
        release: Receiver<()>,
    },
    PtyEof,
    Snapshot {
        scroll: Option<ScrollCommand>,
        point: Option<huterm_protocol::MousePosition>,
        #[cfg(test)]
        fail_lookup: bool,
        reply: async_channel::Sender<Result<SnapshotReply, RuntimeError>>,
    },
    Selection {
        generation: u64,
        range: BufferRange,
        reply: async_channel::Sender<Result<Option<String>, RuntimeError>>,
    },
    WorkerFailed(String),
    WriterFailed(String),
    Wake,
}

fn complete_snapshot_request(
    result: Result<SnapshotReply, RuntimeError>,
    reply: &async_channel::Sender<Result<SnapshotReply, RuntimeError>>,
    events: &EventPublisher,
    terminal_id: TerminalId,
    closing: &AtomicBool,
) {
    if let Err(error) = &result {
        report_failure(events, terminal_id, error.to_string());
        closing.store(true, Ordering::Release);
    }
    let _ = reply.try_send(result);
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
    mut engine: TerminalEngine,
    process: PtyProcess,
    messages: Receiver<RuntimeMessage>,
    output: Receiver<Vec<u8>>,
    output_sender: crate::wake::SyncSender<Vec<u8>>,
    controls: Receiver<RuntimeControl>,
    control_sender: crate::wake::Sender<RuntimeControl>,
    events: EventPublisher,
    invalidation_pending: Arc<AtomicBool>,
    closing: Arc<AtomicBool>,
    queued_input_bytes: Arc<AtomicUsize>,
    shutdown_groups: Arc<Mutex<Vec<i32>>>,
    startup: &SyncSender<Result<(), RuntimeError>>,
) -> Result<(), RuntimeError> {
    let wake = Arc::clone(&control_sender.wake);
    let parts = match process.into_parts() {
        Ok(parts) => parts,
        Err(error) => {
            let _ = events.send(TerminalEvent::Failed {
                terminal_id,
                message: error.to_string(),
            });
            return Err(error);
        }
    };
    let crate::pty::PtyParts {
        master,
        reader,
        reader_waiter,
        writer_waiter,
        writer,
        mut child,
        mut killer,
    } = parts;
    #[cfg(unix)]
    {
        child = match crate::child_wait::observe(child, Arc::clone(&wake)) {
            Ok(child) => child,
            Err((error, mut child)) => {
                let mut groups = pty::process_groups(child.process_id());
                pty::record_foreground_group(master.as_ref(), &mut groups);
                pty::terminate_child(child.as_mut(), killer.as_mut(), &groups);
                drop(reader);
                drop(reader_waiter);
                drop(writer_waiter);
                drop(writer);
                drop(master);
                let _ = pty::reap_child(child);
                return Err(RuntimeError::Thread(error.to_string()));
            }
        };
    }
    let mut process_groups = pty::process_groups(child.process_id());
    pty::record_foreground_group(master.as_ref(), &mut process_groups);
    let reader_cancel = reader_waiter.cancellation();
    let reader_join = match spawn_reader(
        terminal_id,
        reader,
        reader_waiter,
        output_sender,
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
            drop(writer);
            drop(writer_waiter);
            drop(master);
            let _ = pty::reap_child(child);
            return Err(error);
        }
    };
    let (writer_sender, writer_receiver) =
        async_channel::bounded(WRITER_CAPACITY);
    let input_closed = Arc::new(AtomicBool::new(false));
    let writer_capacity = Arc::new(WriterCapacity::default());
    let writer_cancel = writer_waiter.cancellation();
    let writer_join = match spawn_writer(
        terminal_id,
        writer,
        WriterReadiness::Pty(writer_waiter),
        writer_receiver,
        control_sender,
        Arc::clone(&writer_capacity),
        Arc::clone(&closing),
        Arc::clone(&input_closed),
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
            drop(output);
            reader_cancel.cancel();
            join_worker(reader_join);
            let _ = pty::reap_child(child);
            return Err(error);
        }
    };
    let _ = startup.send(Ok(()));
    let _ = events.send(TerminalEvent::Ready(terminal_id));
    let mut invalidated_at = None;
    publish_invalidation(
        &events,
        &invalidation_pending,
        &mut invalidated_at,
        terminal_id,
        engine.generation(),
    );

    let lifecycle = Arc::new(crate::jobs::JobLifecycle::default());
    let mut child_exited = false;
    let mut metadata = PublishedMetadata::default();
    #[cfg(test)]
    let mut pty_eof = false;
    let mut pending_writes = VecDeque::new();
    let mut output_turn = false;
    let mut probes = ProbeSchedule::default();
    let root = child.process_id();
    while !closing.load(Ordering::Acquire) {
        if let Err(error) = observe_child_exit(
            child.as_mut(),
            &lifecycle,
            terminal_id,
            &events,
            &mut child_exited,
            &input_closed,
            &mut pending_writes,
        ) {
            report_failure(&events, terminal_id, error);
            closing.store(true, Ordering::Release);
        }
        // Root exit clears the name; the exited terminal is never probed.
        if child_exited {
            probes.stop();
            metadata.publish(None, terminal_id, &events);
        } else {
            let now = Instant::now();
            if probes.due(now) {
                let probe = crate::foreground::probe(
                    master.process_group_leader(),
                    root,
                );
                probes.probed(now, probe.job);
                metadata.reports.probed(probe.group, probe.directory);
                metadata.publish(probe.name, terminal_id, &events);
            }
        }
        let mut controls_drained = 0;
        while controls_drained < MESSAGE_CAPACITY {
            let Ok(control) = controls.try_recv() else {
                break;
            };
            controls_drained += 1;
            if closing.load(Ordering::Acquire) {
                break;
            }
            match control {
                RuntimeControl::Snapshot {
                    scroll,
                    point,
                    reply,
                    #[cfg(test)]
                    fail_lookup,
                } => {
                    // Rearm on the owner thread at snapshot construction, not
                    // when a client merely drains its notification. Hidden or
                    // frame-blocked clients already know they are dirty and
                    // must not wake again for every PTY chunk.
                    invalidation_pending.store(false, Ordering::Release);
                    let started = Instant::now();
                    let result = (|| {
                        let requested_viewport =
                            engine.requested_viewport(scroll)?;
                        if let Some(scroll) = scroll {
                            engine.scroll(scroll)?;
                        }
                        let snapshot = engine.snapshot()?;
                        let snapshot_duration = started.elapsed();
                        let lookup_started = Instant::now();
                        let link = point.map(|point| {
                            #[cfg(test)]
                            if fail_lookup {
                                return huterm_protocol::LinkLookup::Unavailable;
                            }
                            engine.lookup_link(&snapshot, point)
                        });
                        Ok(SnapshotReply {
                            link,
                            lookup_duration: lookup_started.elapsed(),
                            snapshot,
                            requested_viewport,
                            snapshot_duration,
                            completed_at: Instant::now(),
                            invalidated_at: invalidated_at.take(),
                        })
                    })();
                    complete_snapshot_request(
                        result,
                        &reply,
                        &events,
                        terminal_id,
                        &closing,
                    );
                }
                RuntimeControl::Selection {
                    generation,
                    range,
                    reply,
                } => {
                    let _ =
                        reply.try_send(engine.extract_text(generation, range));
                }
                RuntimeControl::WriterFailed(message) => {
                    // Exit may have happened since the loop's initial poll.
                    if let Err(error) = observe_child_exit(
                        child.as_mut(),
                        &lifecycle,
                        terminal_id,
                        &events,
                        &mut child_exited,
                        &input_closed,
                        &mut pending_writes,
                    ) {
                        report_failure(&events, terminal_id, error);
                        closing.store(true, Ordering::Release);
                    } else if !child_exited {
                        report_failure(&events, terminal_id, message);
                        closing.store(true, Ordering::Release);
                    }
                }
                RuntimeControl::WorkerFailed(message) => {
                    report_failure(&events, terminal_id, message);
                    closing.store(true, Ordering::Release);
                }
                RuntimeControl::PtyEof => {
                    #[cfg(test)]
                    {
                        pty_eof = true;
                    }
                }
                RuntimeControl::JobContext(reply) => {
                    let _ = reply.send(crate::jobs::JobContext {
                        lifecycle: Arc::clone(&lifecycle),
                        shell: child.process_id(),
                        foreground: (!child_exited)
                            .then(|| master.process_group_leader())
                            .flatten(),
                        #[cfg(test)]
                        exited: child_exited,
                        #[cfg(test)]
                        pty_eof,
                        tty: master.tty_name().and_then(|name| {
                            huterm_procinfo::tty_device(&name)
                        }),
                    });
                }
                #[cfg(test)]
                RuntimeControl::Presentation(reply) => {
                    let _ = reply.send((
                        engine.presentation().clone(),
                        engine.size(),
                        engine.cell_size(),
                    ));
                }
                #[cfg(test)]
                RuntimeControl::ProbeArmed(reply) => {
                    let _ = reply.send(probes.deadline().is_some());
                }
                #[cfg(test)]
                RuntimeControl::Pause { entered, release } => {
                    let _ = entered.send(());
                    let _ = release.recv();
                }
                RuntimeControl::ForegroundJob(reply) => {
                    let busy = !child_exited && {
                        let shell = child
                            .process_id()
                            .and_then(|id| i32::try_from(id).ok());
                        let foreground = master.process_group_leader();
                        foreground.zip(shell).is_none_or(
                            |(foreground, shell)| foreground != shell,
                        )
                    };
                    let _ = reply.try_send(busy);
                }
                RuntimeControl::Wake => {}
            }
        }
        if closing.load(Ordering::Acquire) {
            break;
        }
        if child_exited {
            // Closing the channel wakes an idle writer even when no input
            // arrives after root exit. Queued writes are rejected by input_closed.
            writer_sender.close();
            writer_cancel.cancel();
        }
        let mut writer_state =
            flush_pending_write(&writer_sender, &mut pending_writes);
        if writer_state == WriterQueueState::Full {
            writer_capacity.request_wake();
            // The writer may have dequeued before it saw the request.
            writer_state =
                flush_pending_write(&writer_sender, &mut pending_writes);
        }
        match writer_state {
            WriterQueueState::Drained => {}
            WriterQueueState::Full => {
                if controls_drained < MESSAGE_CAPACITY {
                    wake.wait_until(probes.deadline());
                }
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
        let message = match next_message(&messages, &output, &mut output_turn) {
            Ok(message) => message,
            Err(TryRecvError::Empty) => {
                if controls_drained < MESSAGE_CAPACITY {
                    wake.wait_until(probes.deadline());
                }
                continue;
            }
            Err(TryRecvError::Disconnected) => break,
        };
        match message {
            NextMessage::Output(bytes) => {
                let now = Instant::now();
                probes.output(now);
                let effects = match engine.process(&bytes) {
                    Ok(effects) => effects,
                    Err(error) => {
                        report_failure(&events, terminal_id, error.to_string());
                        closing.store(true, Ordering::Release);
                        continue;
                    }
                };
                for effect in effects {
                    if child_exited
                        && matches!(effect, EngineEffect::PtyWrite(_))
                    {
                        continue;
                    }
                    // Programs can re-send an unchanged title on every
                    // chunk; only new text can mean a new program.
                    if let EngineEffect::Title(title) = &effect
                        && metadata.reports.title_text_changes(title)
                    {
                        probes.title(now);
                    }
                    if handle_effect(
                        effect,
                        terminal_id,
                        &writer_sender,
                        &mut pending_writes,
                        &events,
                        &mut metadata,
                        master.as_ref(),
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
                    &mut invalidated_at,
                    terminal_id,
                    engine.generation(),
                );
            }
            NextMessage::Client(RuntimeMessage::Input {
                input,
                reserved_bytes,
            }) => {
                queued_input_bytes.fetch_sub(reserved_bytes, Ordering::AcqRel);
                if child_exited {
                    continue;
                }
                let modes = match engine.modes() {
                    Ok(modes) => modes,
                    Err(error) => {
                        report_failure(&events, terminal_id, error.to_string());
                        closing.store(true, Ordering::Release);
                        continue;
                    }
                };
                let bytes = encode_input(&input, modes, engine.size());
                if probes.input(&bytes, Instant::now()) {
                    metadata.reports.job_control_input();
                }
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
            NextMessage::Client(RuntimeMessage::Resize { grid, cell }) => {
                if !child_exited
                    && master.resize(pty::pty_size(grid, cell)).is_err()
                {
                    let _ = events.send(TerminalEvent::Failed {
                        terminal_id,
                        message: "failed to resize PTY".into(),
                    });
                }
                let effects = match engine.resize(grid, cell) {
                    Ok(effects) => effects,
                    Err(error) => {
                        report_failure(&events, terminal_id, error.to_string());
                        closing.store(true, Ordering::Release);
                        continue;
                    }
                };
                for effect in effects {
                    if handle_effect(
                        effect,
                        terminal_id,
                        &writer_sender,
                        &mut pending_writes,
                        &events,
                        &mut metadata,
                        master.as_ref(),
                    ) == WriterQueueState::Disconnected
                    {
                        report_failure(
                            &events,
                            terminal_id,
                            "PTY writer stopped during resize".into(),
                        );
                        closing.store(true, Ordering::Release);
                        break;
                    }
                }
                publish_invalidation(
                    &events,
                    &invalidation_pending,
                    &mut invalidated_at,
                    terminal_id,
                    engine.generation(),
                );
            }
            NextMessage::Client(RuntimeMessage::Edit(edit)) => {
                let effects = match edit {
                    BufferEdit::ClearHistory => engine.clear_history(),
                    BufferEdit::Reset => engine.reset(),
                };
                let effects = match effects {
                    Ok(effects) => effects,
                    Err(error) => {
                        report_failure(&events, terminal_id, error.to_string());
                        closing.store(true, Ordering::Release);
                        continue;
                    }
                };
                for effect in effects {
                    if handle_effect(
                        effect,
                        terminal_id,
                        &writer_sender,
                        &mut pending_writes,
                        &events,
                        &mut metadata,
                        master.as_ref(),
                    ) == WriterQueueState::Disconnected
                    {
                        report_failure(
                            &events,
                            terminal_id,
                            "PTY writer stopped during a buffer edit".into(),
                        );
                        closing.store(true, Ordering::Release);
                        break;
                    }
                }
                publish_invalidation(
                    &events,
                    &invalidation_pending,
                    &mut invalidated_at,
                    terminal_id,
                    engine.generation(),
                );
            }
            NextMessage::Client(RuntimeMessage::Presentation(update)) => {
                match update.apply(&mut engine) {
                    Ok(true) => publish_invalidation(
                        &events,
                        &invalidation_pending,
                        &mut invalidated_at,
                        terminal_id,
                        engine.generation(),
                    ),
                    Ok(false) => {}
                    Err(error) => {
                        report_failure(&events, terminal_id, error.to_string());
                        closing.store(true, Ordering::Release);
                    }
                }
            }
        }
    }

    closing.store(true, Ordering::Release);
    lifecycle.retire();
    if child_exited {
        // Root exit completes the terminal. Never signal historical groups
        // after reaping, even when detached descendants remain alive.
        process_groups = pty::process_groups(None);
    } else {
        pty::record_foreground_group(master.as_ref(), &mut process_groups);
        process_groups.assessed = shutdown_groups
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .copied()
            .collect();
    }
    pty::terminate_child(child.as_mut(), killer.as_mut(), &process_groups);
    writer_cancel.cancel();
    drop(writer_sender);
    drop(master);
    drop(messages);
    drop(output);
    reader_cancel.cancel();
    join_worker(reader_join);
    join_worker(writer_join);
    if pty::reap_child(child) {
        Ok(())
    } else {
        report_failure(
            &events,
            terminal_id,
            RuntimeError::ShutdownTimedOut.to_string(),
        );
        Err(RuntimeError::ShutdownTimedOut)
    }
}

enum NextMessage {
    Client(RuntimeMessage),
    Output(Vec<u8>),
}

/// Takes client requests ahead of PTY output, so input waits behind at most
/// one output chunk. When both queues are waiting they alternate, so neither
/// can starve the other.
///
/// Input can overtake output that was read but not yet parsed, and is encoded
/// against the modes parsed so far. Those modes are never older than what the
/// client has displayed. Alacritty, kitty, and Ghostty also encode keys against
/// parsed state while read output waits, and output still in the kernel buffer
/// is overtaken by every terminal. Programs that must know a mode is active
/// query it, for example with DECRQM.
fn next_message(
    messages: &Receiver<RuntimeMessage>,
    output: &Receiver<Vec<u8>>,
    output_turn: &mut bool,
) -> Result<NextMessage, TryRecvError> {
    let client = || messages.try_recv().map(NextMessage::Client);
    let pty = || output.try_recv().map(NextMessage::Output);
    let first = if *output_turn { pty() } else { client() };
    let next = match first {
        Ok(next) => Ok(next),
        Err(first_error) => {
            let second = if *output_turn { client() } else { pty() };
            match second {
                Ok(next) => Ok(next),
                Err(TryRecvError::Disconnected)
                    if first_error == TryRecvError::Disconnected =>
                {
                    Err(TryRecvError::Disconnected)
                }
                Err(_) => Err(TryRecvError::Empty),
            }
        }
    };
    *output_turn = matches!(next, Ok(NextMessage::Client(_)));
    next
}

fn spawn_reader(
    terminal_id: TerminalId,
    mut reader: Box<dyn Read + Send>,
    reader_waiter: pty::ReadinessWaiter,
    output: crate::wake::SyncSender<Vec<u8>>,
    controls: crate::wake::Sender<RuntimeControl>,
    closing: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, RuntimeError> {
    thread::Builder::new()
        .name(format!("huterm-reader-{}", terminal_id.get()))
        .spawn(move || {
            forward_output(
                &mut *reader,
                || reader_waiter.wait(None).map(drop),
                &output,
                &controls,
                &closing,
            );
        })
        .map_err(|error| RuntimeError::Thread(error.to_string()))
}

/// Sends PTY output to the runtime in batches until end-of-file, a failure,
/// teardown, or closing. `wait` blocks until the PTY is readable or its
/// cancellation descriptor is signalled.
fn forward_output(
    reader: &mut dyn Read,
    mut wait: impl FnMut() -> std::io::Result<()>,
    output: &crate::wake::SyncSender<Vec<u8>>,
    controls: &crate::wake::Sender<RuntimeControl>,
    closing: &AtomicBool,
) {
    let mut buffer = [0_u8; 8192];
    let mut pending = None;
    let mut ended = None;
    let mut drained = false;
    while !closing.load(Ordering::Acquire) {
        if let Some(bytes) = pending.take() {
            // Teardown drops the receiver before joining this worker,
            // so bounded backpressure also has an explicit cancellation path.
            if output.send(bytes).is_err() {
                break;
            }
            continue;
        }
        // End-of-file or a failure found while batching follows the
        // output read before it.
        if let Some(control) = ended.take() {
            let _ = controls.send(control);
            break;
        }
        // A read that would block needs no retry before the wait.
        if std::mem::take(&mut drained)
            && let Err(error) = wait()
        {
            let _ = controls.send(RuntimeControl::WorkerFailed(format!(
                "PTY readiness wait failed: {error}"
            )));
            break;
        }
        match reader.read(&mut buffer) {
            Ok(0) => {
                let _ = controls.send(RuntimeControl::PtyEof);
                break;
            }
            Ok(count) => {
                let mut batch = buffer[..count].to_vec();
                // Only Unix PTYs are nonblocking. Elsewhere, a second read
                // would hold this output until the PTY wrote again.
                if cfg!(not(unix)) {
                    pending = Some(batch);
                    continue;
                }
                match read_ready(reader, &mut buffer, &mut batch) {
                    ReadyEnd::Full => {}
                    ReadyEnd::Drained => drained = true,
                    ReadyEnd::Ended(control) => ended = Some(control),
                }
                pending = Some(batch);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                drained = true;
            }
            Err(error) => {
                let _ = controls.send(RuntimeControl::WorkerFailed(format!(
                    "PTY read failed: {error}"
                )));
                break;
            }
        }
    }
}

/// Why [`read_ready`] stopped adding to a batch.
enum ReadyEnd {
    Full,
    Drained,
    Ended(RuntimeControl),
}

/// Appends output the PTY already holds to `batch`, without waiting. When
/// output arrives faster than single reads drain it, one message then carries
/// several reads; each message costs a channel send, a runtime wake, and a
/// parser call.
fn read_ready(
    reader: &mut dyn Read,
    buffer: &mut [u8],
    batch: &mut Vec<u8>,
) -> ReadyEnd {
    while batch.len() < READ_BATCH_BYTES {
        let limit = buffer.len().min(READ_BATCH_BYTES - batch.len());
        match reader.read(&mut buffer[..limit]) {
            Ok(0) => return ReadyEnd::Ended(RuntimeControl::PtyEof),
            Ok(count) => batch.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return ReadyEnd::Drained;
            }
            Err(error) => {
                return ReadyEnd::Ended(RuntimeControl::WorkerFailed(format!(
                    "PTY read failed: {error}"
                )));
            }
        }
    }
    ReadyEnd::Full
}

fn observe_child_exit(
    child: &mut dyn portable_pty::Child,
    lifecycle: &crate::jobs::JobLifecycle,
    terminal_id: TerminalId,
    events: &EventPublisher,
    exited: &mut bool,
    input_closed: &AtomicBool,
    pending_writes: &mut VecDeque<Vec<u8>>,
) -> Result<(), String> {
    if *exited {
        return Ok(());
    }
    let status = child.try_wait().map_err(|error| error.to_string())?;
    if let Some(status) = status {
        *exited = true;
        lifecycle.observe_exit();
        input_closed.store(true, Ordering::Release);
        pending_writes.clear();
        let _ = events.send(TerminalEvent::Exited {
            terminal_id,
            status: ExitStatus {
                code: Some(status.exit_code()),
                success: status.success(),
            },
        });
    }
    Ok(())
}

/// Wakes the runtime when the writer frees queue capacity, but only while the
/// runtime holds spilled writes. Waking on every dequeue would wake the
/// runtime a second time for each keystroke.
#[derive(Debug, Default)]
struct WriterCapacity {
    requested: AtomicBool,
}

impl WriterCapacity {
    /// Runtime side. The runtime must retry its spilled writes after this
    /// call: a dequeue that preceded the request freed capacity but sent no
    /// wake.
    fn request_wake(&self) {
        self.requested.store(true, Ordering::Relaxed);
        // Orders the request before the retry's queue check, pairing with
        // the fence in `dequeued`.
        atomic::fence(Ordering::SeqCst);
    }

    /// Writer side, after each dequeue. Returns whether the runtime asked to
    /// be woken, consuming the request.
    fn dequeued(&self) -> bool {
        atomic::fence(Ordering::SeqCst);
        self.requested.swap(false, Ordering::Relaxed)
    }
}

enum WriterReadiness {
    Pty(pty::ReadinessWaiter),
    #[cfg(test)]
    Immediate,
}

impl WriterReadiness {
    fn wait(&self) -> std::io::Result<()> {
        match self {
            Self::Pty(waiter) => waiter.wait(None).map(drop),
            #[cfg(test)]
            Self::Immediate => Ok(()),
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the writer thread takes ownership of each runtime handle it shares"
)]
fn spawn_writer(
    terminal_id: TerminalId,
    mut writer: Box<dyn Write + Send>,
    readiness: WriterReadiness,
    messages: async_channel::Receiver<WriterMessage>,
    controls: crate::wake::Sender<RuntimeControl>,
    capacity: Arc<WriterCapacity>,
    closing: Arc<AtomicBool>,
    input_closed: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, RuntimeError> {
    thread::Builder::new()
        .name(format!("huterm-writer-{}", terminal_id.get()))
        .spawn(move || {
            let mut current: Option<(Vec<u8>, usize)> = None;
            while !closing.load(Ordering::Acquire)
                && !input_closed.load(Ordering::Acquire)
            {
                if current.is_none() {
                    current = match messages.recv_blocking() {
                        Ok(WriterMessage::Write(bytes)) => {
                            if capacity.dequeued() {
                                controls.wake.notify();
                            }
                            Some((bytes, 0))
                        }
                        Err(_) => break,
                    };
                }
                if input_closed.load(Ordering::Acquire) {
                    break;
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
                            if let Err(error) = readiness.wait() {
                                let message = format!(
                                    "PTY write readiness wait failed: {error}"
                                );
                                let _ = controls.send(
                                    RuntimeControl::WriterFailed(message),
                                );
                                break;
                            }
                        }
                        Err(error) => {
                            let _ =
                                controls.send(RuntimeControl::WriterFailed(
                                    format!("PTY flush failed: {error}"),
                                ));
                            break;
                        }
                    }
                    continue;
                }
                match writer.write(&bytes[*offset..]) {
                    Ok(0) => {
                        let _ = controls.send(RuntimeControl::WriterFailed(
                            "PTY write returned zero bytes".into(),
                        ));
                        break;
                    }
                    Ok(count) => *offset += count,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        if let Err(error) = readiness.wait() {
                            let message = format!(
                                "PTY write readiness wait failed: {error}"
                            );
                            let _ = controls
                                .send(RuntimeControl::WriterFailed(message));
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = controls.send(RuntimeControl::WriterFailed(
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
    writer: &async_channel::Sender<WriterMessage>,
    pending_writes: &mut VecDeque<Vec<u8>>,
    events: &EventPublisher,
    metadata: &mut PublishedMetadata,
    master: &dyn portable_pty::MasterPty,
) -> WriterQueueState {
    match effect {
        EngineEffect::PtyWrite(bytes) => {
            queue_write(bytes, writer, pending_writes)
        }
        EngineEffect::Title(title) => {
            // Titles can arrive on every output chunk; read the foreground
            // group only when the text or its attribution may have changed.
            let now = Instant::now();
            if metadata.reports.title_needs_group(&title, now) {
                metadata.reports.report_title(
                    title.clone(),
                    master.process_group_leader(),
                    now,
                );
                let process =
                    metadata.current.foreground_process().map(str::to_owned);
                metadata.publish(process, terminal_id, events);
            }
            let _ =
                events.send(TerminalEvent::TitleChanged { terminal_id, title });
            WriterQueueState::Drained
        }
        EngineEffect::Directory(directory) => {
            // The report belongs to whichever group holds the foreground now.
            metadata
                .reports
                .report_directory(directory, master.process_group_leader());
            let process =
                metadata.current.foreground_process().map(str::to_owned);
            metadata.publish(process, terminal_id, events);
            WriterQueueState::Drained
        }
        EngineEffect::Bell => {
            let _ = events.send(TerminalEvent::Bell(terminal_id));
            WriterQueueState::Drained
        }
    }
}

/// Metadata last published to clients, and the reports behind it.
#[derive(Debug, Default)]
struct PublishedMetadata {
    current: TerminalMetadata,
    revision: u64,
    reports: Reports,
}

impl PublishedMetadata {
    /// Publishes the effective directory and `process` unless both are
    /// unchanged.
    fn publish(
        &mut self,
        process: Option<String>,
        terminal_id: TerminalId,
        events: &EventPublisher,
    ) {
        let replacement =
            TerminalMetadata::new(self.reports.directory(), process)
                .with_foreground_title(self.reports.title());
        if self.current == replacement {
            return;
        }
        self.current = replacement;
        self.revision = self.revision.saturating_add(1);
        let _ = events.send(TerminalEvent::MetadataChanged {
            terminal_id,
            revision: self.revision,
            metadata: self.current.clone(),
        });
    }
}

fn queue_write(
    bytes: Vec<u8>,
    writer: &async_channel::Sender<WriterMessage>,
    pending: &mut VecDeque<Vec<u8>>,
) -> WriterQueueState {
    if !pending.is_empty() {
        pending.push_back(bytes);
        return WriterQueueState::Full;
    }
    match writer.try_send(WriterMessage::Write(bytes)) {
        Ok(()) => WriterQueueState::Drained,
        Err(async_channel::TrySendError::Full(WriterMessage::Write(bytes))) => {
            pending.push_back(bytes);
            WriterQueueState::Full
        }
        Err(async_channel::TrySendError::Closed(_)) => {
            WriterQueueState::Disconnected
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WriterQueueState {
    Drained,
    Full,
    Disconnected,
}

fn flush_pending_write(
    writer: &async_channel::Sender<WriterMessage>,
    pending: &mut VecDeque<Vec<u8>>,
) -> WriterQueueState {
    while let Some(bytes) = pending.pop_front() {
        match writer.try_send(WriterMessage::Write(bytes)) {
            Ok(()) => {}
            Err(async_channel::TrySendError::Full(WriterMessage::Write(
                bytes,
            ))) => {
                pending.push_front(bytes);
                return WriterQueueState::Full;
            }
            Err(async_channel::TrySendError::Closed(_)) => {
                return WriterQueueState::Disconnected;
            }
        }
    }
    WriterQueueState::Drained
}

fn report_failure(
    events: &EventPublisher,
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
        TerminalInput::Character { text, meta } => text
            .len()
            .saturating_add(usize::from(*meta && !text.is_empty())),
        TerminalInput::Key { .. } | TerminalInput::Focus(_) => {
            std::mem::size_of::<TerminalInput>()
        }
        _ => std::mem::size_of::<TerminalInput>(),
    }
}

fn publish_invalidation(
    events: &EventPublisher,
    pending: &AtomicBool,
    invalidated_at: &mut Option<Instant>,
    terminal_id: TerminalId,
    generation: u64,
) {
    // Coalesced invalidations keep the earliest instant, so the next snapshot
    // reports how long its oldest content waited.
    invalidated_at.get_or_insert_with(Instant::now);
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
mod tests;
