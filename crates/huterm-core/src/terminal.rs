use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::{self, AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{
    self, Receiver, Sender, SyncSender, TryRecvError, TrySendError,
};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use huterm_protocol::{
    AttachmentId, BufferRange, ExitStatus, InputStamp, MouseTracking,
    RuntimeId, ScrollCommand, TerminalCommand, TerminalId, TerminalInput,
    TerminalMetadata, TerminalSnapshot,
};
#[cfg(test)]
use huterm_protocol::{CellSize, GridSize, TerminalPresentation};
use thiserror::Error;

use crate::engine::{EngineEffect, TerminalEngine};
use crate::foreground::{ProbeSchedule, Reports};
use crate::host_effects::HostEffectSink;
use crate::input::{encode_focus, encode_input};
use crate::pty::{self, PtyProcess};
use crate::viewer::{
    Arbiter, Effects, Registry, Report, Slot, StatusPublisher, TerminalViewer,
    ViewerOptions,
};

pub(crate) const MESSAGE_CAPACITY: usize = 64;
/// Largest PTY output message: one read plus output the PTY already holds.
const READ_BATCH_BYTES: usize = 16 * 1024;
/// Queued output messages, bounding output read ahead of the parser to 1 MiB.
/// The bound counts messages, and macOS PTY reads rarely batch past 1 KiB, so
/// fewer slots would leave too little buffer to cover a snapshot build.
const OUTPUT_CAPACITY: usize = 64;
const WRITER_CAPACITY: usize = 64;
const INPUT_BYTE_CAPACITY: usize = 1024 * 1024;
const SNAPSHOT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

/// Crate-private handle for one terminal runtime: viewers send through it,
/// and close assessment and teardown use its control requests. Clients
/// outside the crate hold a [`TerminalViewer`] instead.
#[derive(Clone, Debug)]
pub(crate) struct RuntimeClient {
    terminal_id: TerminalId,
    messages: crate::wake::SyncSender<RuntimeMessage>,
    #[cfg(test)]
    output: crate::wake::SyncSender<Vec<u8>>,
    controls: crate::wake::Sender<RuntimeControl>,
    closing: Arc<AtomicBool>,
    queued_input_bytes: Arc<AtomicUsize>,
    shutdown_groups: Arc<Mutex<Vec<i32>>>,
    host_effect_sink: HostEffectSink,
    registry: Arc<Registry>,
}

impl RuntimeClient {
    pub(crate) fn terminal_id(&self) -> TerminalId {
        self.terminal_id
    }

    pub(crate) fn registry(&self) -> Arc<Registry> {
        Arc::clone(&self.registry)
    }

    /// Sends a viewer's input, reserving its bytes against the shared input
    /// budget until the runtime dequeues it.
    pub(crate) fn offer_input(
        &self,
        slot: Arc<Slot>,
        input: TerminalInput,
        stamp: InputStamp,
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
                slot,
                input,
                stamp,
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

    /// Sends an ordered client message without blocking.
    pub(crate) fn send_message(
        &self,
        message: RuntimeMessage,
    ) -> Result<(), RuntimeError> {
        match self.messages.try_send(message) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(RuntimeError::Busy),
            Err(TrySendError::Disconnected(_)) => Err(RuntimeError::Stopped),
        }
    }

    /// Sends a priority control request.
    pub(crate) fn send_control(
        &self,
        control: RuntimeControl,
    ) -> Result<(), RuntimeError> {
        self.controls
            .send(control)
            .map_err(|_| RuntimeError::Stopped)
    }

    /// Checks whether the PTY has a foreground job other than its shell.
    #[cfg(test)]
    pub(crate) async fn has_foreground_job(
        &self,
    ) -> Result<bool, RuntimeError> {
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

    #[cfg(test)]
    pub(crate) fn queued_input_bytes(&self) -> usize {
        self.queued_input_bytes.load(Ordering::Acquire)
    }

    /// Requests orderly terminal shutdown.
    pub(crate) fn close(&self) -> Result<(), RuntimeError> {
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
    /// Immutable terminal snapshot, shared by every viewer that reads the
    /// same publication.
    pub snapshot: Arc<TerminalSnapshot>,
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
    /// first to include for its viewer. `None` when nothing changed since
    /// that viewer's last snapshot.
    pub invalidated_at: Option<Instant>,
}

impl SnapshotRequest {
    pub(crate) fn new(
        receiver: async_channel::Receiver<Result<SnapshotReply, RuntimeError>>,
    ) -> Self {
        Self { receiver }
    }

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

    pub(crate) fn recv_blocking(self) -> Result<SnapshotReply, RuntimeError> {
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
    pub(crate) fn new(
        receiver: async_channel::Receiver<Result<Option<String>, RuntimeError>>,
    ) -> Self {
        Self { receiver }
    }

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

    #[cfg(test)]
    pub(crate) fn recv_blocking(self) -> Result<Option<String>, RuntimeError> {
        self.receiver
            .recv_blocking()
            .map_err(|_| RuntimeError::Stopped)?
    }
}

/// Closes the host-effect sink and the viewer registry when the runtime
/// thread ends, including by unwinding, so no viewer waits on a stopped
/// terminal.
struct CloseOnExit {
    host_effects: HostEffectSink,
    registry: Arc<Registry>,
}

impl Drop for CloseOnExit {
    fn drop(&mut self) {
        self.host_effects.close();
        self.registry.close();
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
        Self::spawn_in(RuntimeId::new(0), terminal_id, command)
    }

    /// Starts a terminal whose viewer IDs are scoped to `runtime`.
    pub(crate) fn spawn_in(
        runtime: RuntimeId,
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
        let registry =
            Arc::new(Registry::new(terminal_id, runtime, Arc::clone(&wake)));
        let message_sender =
            crate::wake::SyncSender::new(message_sender, Arc::clone(&wake));
        let output_sender =
            crate::wake::SyncSender::new(output_sender, Arc::clone(&wake));
        #[cfg(test)]
        let client_output = output_sender.clone();
        let control_sender = crate::wake::Sender::new(control_sender, wake);
        let closing = Arc::new(AtomicBool::new(false));
        let queued_input_bytes = Arc::new(AtomicUsize::new(0));
        let host_effect_sink = HostEffectSink::new(terminal_id);
        let runtime_host_effect_sink = host_effect_sink.clone();
        let runtime_registry = Arc::clone(&registry);

        let runtime_controls = control_sender.clone();
        let runtime_closing = Arc::clone(&closing);
        let runtime_input_bytes = Arc::clone(&queued_input_bytes);
        let shutdown_groups = Arc::new(Mutex::new(Vec::new()));
        let runtime_groups = Arc::clone(&shutdown_groups);
        let join = thread::Builder::new()
            .name(format!("huterm-runtime-{}", terminal_id.get()))
            .spawn(move || {
                let close = CloseOnExit {
                    host_effects: runtime_host_effect_sink.clone(),
                    registry: Arc::clone(&runtime_registry),
                };
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
                        Arbiter::new(
                            command.grid_size,
                            command.cell_size,
                            command.presentation.clone(),
                        ),
                        process,
                        message_receiver,
                        output_receiver,
                        output_sender,
                        control_receiver,
                        runtime_controls,
                        &runtime_registry,
                        runtime_closing,
                        runtime_input_bytes,
                        runtime_groups,
                        &startup_sender,
                    )
                })();
                drop(close);
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
            shutdown_groups,
            host_effect_sink,
            registry,
        };
        Ok(Self {
            client,
            join: Some(join),
        })
    }

    /// Registers a viewer of a terminal the caller owns.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Stopped`] once the terminal stopped, or
    /// [`RuntimeError::ViewerLimit`] when it has no room for another viewer.
    pub fn subscribe(
        &self,
        options: ViewerOptions,
    ) -> Result<TerminalViewer, RuntimeError> {
        self.subscribe_for(None, options)
    }

    pub(crate) fn subscribe_for(
        &self,
        attachment: Option<AttachmentId>,
        options: ViewerOptions,
    ) -> Result<TerminalViewer, RuntimeError> {
        TerminalViewer::subscribe(self.client.clone(), attachment, options)
    }

    pub(crate) fn client(&self) -> RuntimeClient {
        self.client.clone()
    }

    pub(crate) fn registry(&self) -> Arc<Registry> {
        self.client.registry()
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
    /// The viewer lacks the capability the request needs.
    #[error("this viewer may not make that terminal request")]
    NotPermitted,
    /// The viewer's authority was withdrawn, for example by detaching.
    #[error("this terminal viewer was revoked")]
    Revoked,
    /// The terminal has no room for another viewer.
    #[error("terminal has no room for another viewer")]
    ViewerLimit,
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
pub(crate) enum RuntimeMessage {
    Input {
        slot: Arc<Slot>,
        input: TerminalInput,
        stamp: InputStamp,
        reserved_bytes: usize,
    },
    Arbitration {
        slot: Arc<Slot>,
        report: Report,
    },
    Presentation {
        slot: Arc<Slot>,
        presentation: Box<huterm_protocol::TerminalPresentation>,
    },
    Edit {
        slot: Arc<Slot>,
        edit: BufferEdit,
    },
}

impl RuntimeMessage {
    fn slot(&self) -> &Arc<Slot> {
        match self {
            Self::Input { slot, .. }
            | Self::Arbitration { slot, .. }
            | Self::Presentation { slot, .. }
            | Self::Edit { slot, .. } => slot,
        }
    }

    /// Whether the viewer may still make this request. The handle checked
    /// when it sent the message; the owner thread checks again at dequeue,
    /// because the viewer can be revoked in between, and so the rule does
    /// not depend on client code.
    fn permitted(&self) -> bool {
        let slot = self.slot();
        let capabilities = slot.capabilities;
        !slot.is_revoked()
            && match self {
                Self::Input { .. } | Self::Edit { .. } => capabilities.input,
                Self::Arbitration {
                    report: Report::Focus(_),
                    ..
                } => capabilities.input || capabilities.size,
                Self::Arbitration {
                    report: Report::Geometry(..),
                    ..
                } => capabilities.size,
                Self::Presentation { .. } => true,
            }
    }
}

/// Client changes to emulator state that bypass the PTY.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BufferEdit {
    ClearHistory,
    Reset,
}

#[derive(Debug)]
pub(crate) enum RuntimeControl {
    #[cfg(test)]
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
    /// Panics on the runtime owner.
    #[cfg(test)]
    Panic,
    /// Reports how many writes wait behind a full writer queue.
    #[cfg(test)]
    PendingWrites(Sender<usize>),
    PtyEof,
    Snapshot {
        slot: Arc<Slot>,
        /// A scroll and the viewer's number for it.
        scroll: Option<(ScrollCommand, u64)>,
        point: Option<huterm_protocol::MousePosition>,
        #[cfg(test)]
        fail_lookup: bool,
        reply: async_channel::Sender<Result<SnapshotReply, RuntimeError>>,
    },
    Selection {
        slot: Arc<Slot>,
        generation: u64,
        range: BufferRange,
        reply: async_channel::Sender<Result<Option<String>, RuntimeError>>,
    },
    WorkerFailed(String),
    WriterFailed(String),
    Wake,
}

/// The runtime's publication state: the revision that advances on every
/// snapshot-affecting change, and the last snapshot built at a revision.
#[derive(Debug, Default)]
struct Publication {
    revision: u64,
    cache: Option<Arc<TerminalSnapshot>>,
}

impl Publication {
    /// Advances the revision and notifies every viewer except `skip`.
    fn publish(
        &mut self,
        registry: &Registry,
        skip: Option<huterm_protocol::ViewerId>,
        generation: u64,
    ) {
        self.revision += 1;
        registry.publish(skip, generation);
    }
}

/// Builds the snapshot a viewer requested, applying its scroll first.
/// Reuses the last snapshot when nothing changed since it was built.
#[cfg_attr(
    test,
    expect(
        clippy::too_many_arguments,
        reason = "the test-only lookup fault adds an argument"
    )
)]
fn build_snapshot(
    engine: &mut TerminalEngine,
    publication: &mut Publication,
    arbiter: &mut Arbiter,
    registry: &Registry,
    slot: &Slot,
    scroll: Option<(ScrollCommand, u64)>,
    point: Option<huterm_protocol::MousePosition>,
    #[cfg(test)] fail_lookup: bool,
) -> Result<SnapshotReply, RuntimeError> {
    let started = Instant::now();
    // A scroll this viewer sent before typing that already returned the
    // viewport to live would move it back into history, and one from a
    // closed view would move it for every remaining viewer.
    let scroll = scroll.filter(|&(_, number)| {
        !slot.is_dropped() && !slot.scroll_superseded(number)
    });
    let requested_viewport =
        engine.requested_viewport(scroll.map(|(command, _)| command))?;
    if let Some((command, number)) = scroll {
        let before = engine.viewport_offset()?;
        engine.scroll(command)?;
        slot.scrolled(number);
        if engine.viewport_offset()? != before {
            publication.publish(registry, Some(slot.id), engine.generation());
        }
    }
    let snapshot = match &publication.cache {
        Some(cached) if cached.revision == publication.revision => {
            Arc::clone(cached)
        }
        _ => {
            let mut snapshot = engine.snapshot()?;
            snapshot.revision = publication.revision;
            snapshot.geometry_revision = arbiter.geometry_revision();
            let snapshot = Arc::new(snapshot);
            publication.cache = Some(Arc::clone(&snapshot));
            snapshot
        }
    };
    let invalidated_at = slot.snapshot_built();
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
        invalidated_at,
    })
}

fn complete_snapshot_request(
    result: Result<SnapshotReply, RuntimeError>,
    reply: &async_channel::Sender<Result<SnapshotReply, RuntimeError>>,
    status: &mut StatusPublisher,
    closing: &AtomicBool,
) {
    if let Err(error) = &result {
        status.failure(error.to_string());
        closing.store(true, Ordering::Release);
    }
    let _ = reply.try_send(result);
}

#[derive(Debug)]
enum WriterMessage {
    Write(Vec<u8>),
}

/// The PTY-facing state the runtime owner applies arbitration effects to.
struct Owner<'a> {
    engine: &'a mut TerminalEngine,
    master: &'a dyn portable_pty::MasterPty,
    writer: &'a async_channel::Sender<WriterMessage>,
    pending_writes: &'a mut VecDeque<Vec<u8>>,
    status: &'a mut StatusPublisher,
    metadata: &'a mut PublishedMetadata,
    publication: &'a mut Publication,
    registry: &'a Registry,
    child_exited: bool,
}

impl Owner<'_> {
    /// Applies arbitration effects in order: resize, presentation, mouse
    /// releases, then the focus report. Returns false when the runtime must
    /// close.
    fn apply(&mut self, effects: Effects) -> bool {
        if effects.is_empty() {
            return true;
        }
        if let Some((grid, cell)) = effects.resize {
            if !self.child_exited
                && self.master.resize(pty::pty_size(grid, cell)).is_err()
            {
                self.status.failure("failed to resize PTY".into());
            }
            let engine_effects = match self.engine.resize(grid, cell) {
                Ok(effects) => effects,
                Err(error) => {
                    self.status.failure(error.to_string());
                    return false;
                }
            };
            if !self.handle_effects(
                engine_effects,
                "PTY writer stopped during resize",
            ) {
                return false;
            }
            self.publish();
        }
        if let Some(presentation) = effects.presentation {
            if let Err(error) = self.engine.update_presentation(presentation) {
                self.status.failure(error.to_string());
                return false;
            }
            self.publish();
        }
        if self.child_exited
            || (effects.releases.is_empty() && effects.focus.is_none())
        {
            return true;
        }
        let modes = match self.engine.modes() {
            Ok(modes) => modes,
            Err(error) => {
                self.status.failure(error.to_string());
                return false;
            }
        };
        let size = self.engine.size();
        for release in effects.releases {
            let bytes =
                encode_input(&TerminalInput::Mouse(release), modes, size);
            if !self.write(bytes, "PTY writer stopped before a mouse release") {
                return false;
            }
        }
        if let Some(focused) = effects.focus {
            let bytes = encode_focus(focused, modes).to_vec();
            if !self.write(bytes, "PTY writer stopped before a focus report") {
                return false;
            }
        }
        true
    }

    fn publish(&mut self) {
        self.publication
            .publish(self.registry, None, self.engine.generation());
    }

    fn write(&mut self, bytes: Vec<u8>, failure: &str) -> bool {
        if bytes.is_empty()
            || queue_write(bytes, self.writer, self.pending_writes)
                != WriterQueueState::Disconnected
        {
            return true;
        }
        self.status.failure(failure.into());
        false
    }

    fn handle_effects(
        &mut self,
        effects: Vec<EngineEffect>,
        failure: &str,
    ) -> bool {
        for effect in effects {
            // The writer closes at root exit, so a reply such as an in-band
            // size report from a resize would otherwise stop the runtime.
            if self.child_exited && matches!(effect, EngineEffect::PtyWrite(_))
            {
                continue;
            }
            if handle_effect(
                effect,
                self.writer,
                self.pending_writes,
                self.status,
                self.metadata,
                self.master,
            ) == WriterQueueState::Disconnected
            {
                self.status.failure(failure.into());
                return false;
            }
        }
        true
    }
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
    mut arbiter: Arbiter,
    process: PtyProcess,
    messages: Receiver<RuntimeMessage>,
    output: Receiver<Vec<u8>>,
    output_sender: crate::wake::SyncSender<Vec<u8>>,
    controls: Receiver<RuntimeControl>,
    control_sender: crate::wake::Sender<RuntimeControl>,
    registry: &Registry,
    closing: Arc<AtomicBool>,
    queued_input_bytes: Arc<AtomicUsize>,
    shutdown_groups: Arc<Mutex<Vec<i32>>>,
    startup: &SyncSender<Result<(), RuntimeError>>,
) -> Result<(), RuntimeError> {
    let wake = Arc::clone(&control_sender.wake);
    let mut status = StatusPublisher::default();
    let parts = match process.into_parts() {
        Ok(parts) => parts,
        Err(error) => {
            status.failure(error.to_string());
            status.flush(registry);
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
            status.failure(error.to_string());
            status.flush(registry);
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
            status.failure(error.to_string());
            status.flush(registry);
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
    let mut publication = Publication::default();
    publication.publish(registry, None, engine.generation());

    let lifecycle = Arc::new(crate::jobs::JobLifecycle::default());
    let mut child_exited = false;
    let mut metadata = PublishedMetadata::default();
    #[cfg(test)]
    let mut pty_eof = false;
    let mut pending_writes = VecDeque::new();
    let mut output_turn = false;
    let mut probes = ProbeSchedule::default();
    let root = child.process_id();
    // Runs `$body` with an `Owner` borrowing the runtime's PTY-facing state.
    macro_rules! owner {
        (|$owner:ident| $body:expr) => {{
            let mut $owner = Owner {
                engine: &mut engine,
                master: master.as_ref(),
                writer: &writer_sender,
                pending_writes: &mut pending_writes,
                status: &mut status,
                metadata: &mut metadata,
                publication: &mut publication,
                registry,
                child_exited,
            };
            $body
        }};
    }
    while !closing.load(Ordering::Acquire) {
        // Reconciles the viewer table: adds viewers that have sent nothing
        // yet and finalizes dropped and revoked ones. While writes are
        // backed up it waits: the focus reports and releases it writes
        // would queue behind them anyway, and because finalization frees
        // slots, subscribe-and-drop churn could otherwise grow the backlog
        // without bound.
        if pending_writes.is_empty() && registry.take_changed() {
            let effects = arbiter.sync(registry);
            if !owner!(|owner| owner.apply(effects)) {
                closing.store(true, Ordering::Release);
                continue;
            }
        }
        if let Err(error) = observe_child_exit(
            child.as_mut(),
            &lifecycle,
            &mut status,
            &mut child_exited,
            &input_closed,
            &mut pending_writes,
        ) {
            status.failure(error);
            closing.store(true, Ordering::Release);
        }
        // Root exit clears the name; the exited terminal is never probed.
        if child_exited {
            probes.stop();
            metadata.publish(None, &mut status);
        } else {
            let now = Instant::now();
            if probes.due(now) {
                let probe = crate::foreground::probe(
                    master.process_group_leader(),
                    root,
                );
                probes.probed(now, probe.job);
                metadata.reports.probed(probe.group, probe.directory);
                metadata.publish(probe.name, &mut status);
            }
        }
        status.flush(registry);
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
                    slot,
                    scroll,
                    point,
                    reply,
                    #[cfg(test)]
                    fail_lookup,
                } => {
                    // Revocation and refused scrolls answer only this viewer
                    // and never reach the fatal path below.
                    if slot.is_revoked() {
                        let _ = reply.try_send(Err(RuntimeError::Revoked));
                        continue;
                    }
                    if scroll.is_some() && !slot.capabilities.viewport {
                        let _ = reply.try_send(Err(RuntimeError::NotPermitted));
                        continue;
                    }
                    let result = build_snapshot(
                        &mut engine,
                        &mut publication,
                        &mut arbiter,
                        registry,
                        &slot,
                        scroll,
                        point,
                        #[cfg(test)]
                        fail_lookup,
                    );
                    complete_snapshot_request(
                        result,
                        &reply,
                        &mut status,
                        &closing,
                    );
                }
                RuntimeControl::Selection {
                    slot,
                    generation,
                    range,
                    reply,
                } => {
                    let result = if slot.is_revoked() {
                        Err(RuntimeError::Revoked)
                    } else {
                        engine.extract_text(generation, range)
                    };
                    let _ = reply.try_send(result);
                }
                RuntimeControl::WriterFailed(message) => {
                    // Exit may have happened since the loop's initial poll.
                    if let Err(error) = observe_child_exit(
                        child.as_mut(),
                        &lifecycle,
                        &mut status,
                        &mut child_exited,
                        &input_closed,
                        &mut pending_writes,
                    ) {
                        status.failure(error);
                        closing.store(true, Ordering::Release);
                    } else if !child_exited {
                        status.failure(message);
                        closing.store(true, Ordering::Release);
                    }
                }
                RuntimeControl::WorkerFailed(message) => {
                    status.failure(message);
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
                RuntimeControl::PendingWrites(reply) => {
                    let _ = reply.send(pending_writes.len());
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
                #[cfg(test)]
                RuntimeControl::Panic => panic!("injected runtime panic"),
                #[cfg(test)]
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
                status.flush(registry);
                if controls_drained < MESSAGE_CAPACITY {
                    wake.wait_until(probes.deadline());
                }
                continue;
            }
            WriterQueueState::Disconnected => {
                status.failure(
                    "PTY writer stopped before queued input was written".into(),
                );
                closing.store(true, Ordering::Release);
                continue;
            }
        }
        let message = match next_message(&messages, &output, &mut output_turn) {
            Ok(message) => message,
            // A backlog of writes consumes the notification that asked for
            // a reconcile, so nothing else is certain to wake this thread
            // for it once they drain.
            Err(TryRecvError::Empty) if registry.has_pending_change() => {
                continue;
            }
            Err(TryRecvError::Empty) => {
                status.flush(registry);
                if controls_drained < MESSAGE_CAPACITY {
                    wake.wait_until(probes.deadline());
                }
                continue;
            }
            Err(TryRecvError::Disconnected) => break,
        };
        let healthy = match message {
            NextMessage::Output(bytes) => {
                let now = Instant::now();
                probes.output(now);
                let effects = match engine.process(&bytes) {
                    Ok(effects) => effects,
                    Err(error) => {
                        status.failure(error.to_string());
                        closing.store(true, Ordering::Release);
                        continue;
                    }
                };
                let mut healthy = true;
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
                        &writer_sender,
                        &mut pending_writes,
                        &mut status,
                        &mut metadata,
                        master.as_ref(),
                    ) == WriterQueueState::Disconnected
                    {
                        status.failure(
                            "PTY writer stopped before a terminal reply was written"
                                .into(),
                        );
                        healthy = false;
                        break;
                    }
                }
                publication.publish(registry, None, engine.generation());
                healthy
                    && end_untracked_gesture(&engine, &mut arbiter, &mut status)
            }
            NextMessage::Client(message) => owner!(|owner| {
                handle_client_message(
                    &mut owner,
                    &mut arbiter,
                    &mut probes,
                    &queued_input_bytes,
                    message,
                )
            }),
        };
        if !healthy {
            closing.store(true, Ordering::Release);
        }
        // A snapshot that shows this message's output is never ahead of the
        // status the same output produced.
        status.flush(registry);
    }

    closing.store(true, Ordering::Release);
    // A failure that ended the loop reaches viewers now, not after the
    // bounded teardown below.
    status.flush(registry);
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
    let result = if pty::reap_child(child) {
        Ok(())
    } else {
        status.failure(RuntimeError::ShutdownTimedOut.to_string());
        Err(RuntimeError::ShutdownTimedOut)
    };
    status.flush(registry);
    result
}

/// Stops a gesture the application no longer tracks from blocking other
/// viewers. Returns false when the runtime must close.
fn end_untracked_gesture(
    engine: &TerminalEngine,
    arbiter: &mut Arbiter,
    status: &mut StatusPublisher,
) -> bool {
    if !arbiter.gesture_held() {
        return true;
    }
    match engine.modes() {
        Ok(modes) => {
            if modes.mouse_tracking == MouseTracking::Disabled {
                arbiter.tracking_disabled();
            }
            true
        }
        Err(error) => {
            status.failure(error.to_string());
            false
        }
    }
}

/// Handles one ordered viewer request: reconciles the viewer table if it
/// changed, checks that the viewer may still make the request, runs it, and
/// settles the viewer's queued count, which finalizes a dropped viewer once
/// its last request has run. Returns false when the runtime must close.
///
/// The caller dequeues a request only once writes have drained, so the
/// reconcile here never runs behind a backlog.
fn handle_client_message(
    owner: &mut Owner<'_>,
    arbiter: &mut Arbiter,
    probes: &mut ProbeSchedule,
    queued_input_bytes: &AtomicUsize,
    message: RuntimeMessage,
) -> bool {
    let slot = Arc::clone(message.slot());
    if let RuntimeMessage::Input { reserved_bytes, .. } = &message {
        queued_input_bytes.fetch_sub(*reserved_bytes, Ordering::AcqRel);
    }
    // A report's effects apply together with any finalization its settling
    // causes, so a dropped viewer's last focus change and its focus loss
    // cancel instead of both being written.
    let mut effects = Effects::default();
    // Viewers register, drop, and are revoked at any time, and each marks
    // the registry before its next request can be queued. Reconciling at
    // dequeue therefore gives every request its viewer's entry and runs it
    // after any finalization that preceded it. A press with no entry would
    // take a gesture that finalization could not release, and one ahead of
    // a dropped owner's finalization would be refused. Doing it here, not by
    // restarting the turn, means a pending change never delays the request.
    let reconciled = !owner.registry.take_changed()
        || owner.apply(arbiter.sync(owner.registry));
    let healthy = reconciled
        && (!message.permitted()
            || match message {
                RuntimeMessage::Input { input, stamp, .. } => {
                    handle_input(owner, arbiter, probes, &slot, &input, stamp)
                }
                RuntimeMessage::Arbitration { report, .. } => {
                    effects = arbiter.report(&slot, report);
                    true
                }
                RuntimeMessage::Presentation { presentation, .. } => {
                    effects = arbiter.presentation(&slot, *presentation);
                    true
                }
                RuntimeMessage::Edit { edit, .. } => {
                    handle_edit(owner, arbiter, edit)
                }
            });
    slot.settled();
    effects.merge(arbiter.settled(&slot, owner.registry));
    healthy && owner.apply(effects)
}

/// Applies a buffer edit. Returns false when the runtime must close.
fn handle_edit(
    owner: &mut Owner<'_>,
    arbiter: &mut Arbiter,
    edit: BufferEdit,
) -> bool {
    let effects = match edit {
        BufferEdit::ClearHistory => owner.engine.clear_history(),
        BufferEdit::Reset => owner.engine.reset(),
    };
    match effects {
        Ok(effects) => {
            let healthy = owner.handle_effects(
                effects,
                "PTY writer stopped during a buffer edit",
            );
            owner.publish();
            healthy
                && end_untracked_gesture(owner.engine, arbiter, owner.status)
        }
        Err(error) => {
            owner.status.failure(error.to_string());
            false
        }
    }
}

/// Handles one viewer's input: control and the return to live output come
/// first, so the application sees the new size and viewport before the
/// input. Returns false when the runtime must close.
fn handle_input(
    owner: &mut Owner<'_>,
    arbiter: &mut Arbiter,
    probes: &mut ProbeSchedule,
    slot: &Slot,
    input: &TerminalInput,
    stamp: InputStamp,
) -> bool {
    let typing = matches!(
        input,
        TerminalInput::Key { .. }
            | TerminalInput::Character { .. }
            | TerminalInput::Text(_)
            | TerminalInput::Paste(_)
    );
    if typing {
        let effects = arbiter.typed(slot);
        if !owner.apply(effects) {
            return false;
        }
        if slot.return_to_live(stamp) {
            match owner.engine.viewport_offset() {
                Ok(0) => {}
                Ok(_) => {
                    if let Err(error) = owner.engine.scroll(ScrollCommand::Live)
                    {
                        owner.status.failure(error.to_string());
                        return false;
                    }
                    owner.publish();
                }
                Err(error) => {
                    owner.status.failure(error.to_string());
                    return false;
                }
            }
        }
    }
    // After root exit, typing still moves control and returns the viewport
    // to live, but nothing reaches the application.
    if owner.child_exited {
        return true;
    }
    let modes = match owner.engine.modes() {
        Ok(modes) => modes,
        Err(error) => {
            owner.status.failure(error.to_string());
            return false;
        }
    };
    if let TerminalInput::Mouse(mouse) = input {
        let tracked = modes.mouse_tracking != MouseTracking::Disabled;
        if !arbiter.admit_mouse(slot, mouse, stamp, tracked) {
            return true;
        }
    }
    let bytes = encode_input(input, modes, owner.engine.size());
    if probes.input(&bytes, Instant::now()) {
        owner.metadata.reports.job_control_input();
    }
    owner.write(bytes, "PTY writer stopped before input was written")
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
    status: &mut StatusPublisher,
    exited: &mut bool,
    input_closed: &AtomicBool,
    pending_writes: &mut VecDeque<Vec<u8>>,
) -> Result<(), String> {
    if *exited {
        return Ok(());
    }
    let exit = child.try_wait().map_err(|error| error.to_string())?;
    if let Some(exit) = exit {
        *exited = true;
        lifecycle.observe_exit();
        input_closed.store(true, Ordering::Release);
        pending_writes.clear();
        status.exit(ExitStatus {
            code: Some(exit.exit_code()),
            success: exit.success(),
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
    writer: &async_channel::Sender<WriterMessage>,
    pending_writes: &mut VecDeque<Vec<u8>>,
    status: &mut StatusPublisher,
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
                metadata.publish(process, status);
            }
            status.title(title);
            WriterQueueState::Drained
        }
        EngineEffect::Directory(directory) => {
            // The report belongs to whichever group holds the foreground now.
            metadata
                .reports
                .report_directory(directory, master.process_group_leader());
            let process =
                metadata.current.foreground_process().map(str::to_owned);
            metadata.publish(process, status);
            WriterQueueState::Drained
        }
        EngineEffect::Bell => {
            status.bell();
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
        status: &mut StatusPublisher,
    ) {
        let replacement =
            TerminalMetadata::new(self.reports.directory(), process)
                .with_foreground_title(self.reports.title());
        if self.current == replacement {
            return;
        }
        self.current = replacement;
        self.revision = self.revision.saturating_add(1);
        status.metadata(self.revision, &self.current);
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

fn input_bytes(input: &TerminalInput) -> usize {
    match input {
        TerminalInput::Text(text) | TerminalInput::Paste(text) => text.len(),
        TerminalInput::Character { text, meta } => text
            .len()
            .saturating_add(usize::from(*meta && !text.is_empty())),
        _ => std::mem::size_of::<TerminalInput>(),
    }
}

fn join_worker(worker: JoinHandle<()>) {
    let _ = worker.join();
}

#[cfg(test)]
mod tests;
