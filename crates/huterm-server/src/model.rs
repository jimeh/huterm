//! A deterministic model terminal behind [`ViewerPort`], for tests and the
//! seeded fault-injection harness.
//!
//! A [`ModelTerminal`] holds a screen of shared rows and a short history,
//! a revisioned status, and its viewers, much as core's runtime does, but
//! it runs on the caller's thread and changes only when the caller tells
//! it to: a publication, a bell, an exit, a stop, a revocation. Ordered
//! messages wait in a bounded queue with a per-viewer input budget, so a
//! full queue answers `Busy`, and reach its PTY log only when the caller
//! drains it. Every snapshot shares the rows that did not change, so a
//! viewer's deltas keep them. Nothing here touches a thread, a clock, or a
//! random source, so a run replays exactly.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use huterm_core::{RuntimeError, ViewerUpdate};
use huterm_protocol::{
    BufferPoint, BufferRange, Cell, CellColor, CellStyle, CellText, ExitStatus,
    GridSize, InputStamp, LinkLookup, MousePosition, RuntimeId, ScrollCommand,
    SelectionUnit, TerminalFailure, TerminalId, TerminalInput,
    TerminalLifecycle, TerminalModes, TerminalRow, TerminalSnapshot,
    TerminalStatus, ViewerId, Viewport,
};
use huterm_wire::viewer::input_cost;

use crate::viewer::{
    Ordered, Reply, SnapshotOutcome, ViewerPort, Wake, WakeSource,
};

/// Viewers one model terminal admits, as core does.
const VIEWER_LIMIT: usize = 32;

/// History rows a model terminal keeps.
const HISTORY: usize = 64;

/// What a model terminal does with its ordered messages.
#[derive(Clone, Copy, Debug)]
pub struct ModelLimits {
    /// Ordered messages its queue holds before it answers `Busy`.
    pub queue: usize,
    /// Input bytes one viewer may have queued.
    pub viewer_input: usize,
}

impl Default for ModelLimits {
    fn default() -> Self {
        Self {
            queue: 64,
            viewer_input: huterm_wire::caps::VIEWER_INPUT_BUDGET,
        }
    }
}

/// One input that reached the model's PTY.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Written {
    /// The viewer that sent it.
    pub viewer: ViewerId,
    /// The input.
    pub input: TerminalInput,
    /// Its stamp.
    pub stamp: InputStamp,
}

#[derive(Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent viewer flags")]
struct Slot {
    id: ViewerId,
    notified: bool,
    publications: u64,
    reported: u64,
    /// The status revision this viewer's poll last reported.
    seen: u64,
    revoked: bool,
    dropped: bool,
    /// Input bytes queued for this viewer.
    reserved: usize,
    wake: async_channel::Sender<()>,
    receiver: async_channel::Receiver<()>,
    /// The wake signalled since the harness last collected it.
    signalled: bool,
}

impl Slot {
    fn signal(&mut self) {
        let _ = self.wake.try_send(());
        self.signalled = true;
    }
}

#[derive(Debug)]
struct State {
    terminal: TerminalId,
    runtime: RuntimeId,
    limits: ModelLimits,
    size: GridSize,
    rows: Vec<Arc<TerminalRow>>,
    history: VecDeque<Arc<TerminalRow>>,
    generation: u64,
    revision: u64,
    geometry_revision: u64,
    offset: usize,
    cursor_column: u16,
    status: Arc<TerminalStatus>,
    slots: Vec<Slot>,
    /// Numbers viewers across every terminal that shares it, as core
    /// numbers them across every terminal in its process. Each world has
    /// its own, so a run replays exactly.
    next_viewer: Arc<std::sync::atomic::AtomicU64>,
    queue: VecDeque<(ViewerId, Ordered, usize)>,
    pty: Vec<Written>,
    focus: Vec<(ViewerId, bool)>,
    scrolls: Vec<(ViewerId, u64)>,
    stopped: bool,
    /// Snapshot requests answer this error instead, while set.
    refuse_snapshots: Option<RuntimeError>,
}

/// A model terminal, shared by its ports and the caller that drives it.
#[derive(Clone)]
pub struct ModelTerminal {
    state: Arc<Mutex<State>>,
}

impl fmt::Debug for ModelTerminal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.lock();
        f.debug_struct("ModelTerminal")
            .field("terminal", &state.terminal)
            .field("revision", &state.revision)
            .field("viewers", &state.slots.len())
            .finish_non_exhaustive()
    }
}

/// A row of `text`, `columns` wide.
#[must_use]
pub fn text_row(text: &str, columns: usize) -> Arc<TerminalRow> {
    let blank = Cell {
        text: CellText::EMPTY,
        foreground: CellColor::DefaultForeground,
        background: CellColor::DefaultBackground,
        style: CellStyle::default(),
    };
    let mut cells: Vec<Cell> = text
        .chars()
        .take(columns)
        .map(|character| Cell {
            text: CellText::from(character),
            ..blank.clone()
        })
        .collect();
    cells.resize(columns, blank);
    Arc::new(TerminalRow {
        cells,
        ..TerminalRow::default()
    })
}

impl ModelTerminal {
    /// A terminal `terminal` of `size`, its rows blank, numbering its
    /// viewers from `next_viewer`, which every terminal of one world
    /// shares.
    #[must_use]
    pub fn new(
        terminal: TerminalId,
        size: GridSize,
        limits: ModelLimits,
        next_viewer: Arc<std::sync::atomic::AtomicU64>,
    ) -> Self {
        let rows = (0..size.rows)
            .map(|_| text_row("", usize::from(size.columns)))
            .collect();
        Self {
            state: Arc::new(Mutex::new(State {
                terminal,
                runtime: terminal.runtime(),
                limits,
                size,
                rows,
                history: VecDeque::new(),
                generation: 1,
                revision: 1,
                geometry_revision: 1,
                offset: 0,
                cursor_column: 0,
                status: Arc::new(TerminalStatus::default()),
                slots: Vec::new(),
                next_viewer,
                queue: VecDeque::new(),
                pty: Vec::new(),
                focus: Vec::new(),
                scrolls: Vec::new(),
                stopped: false,
                refuse_snapshots: None,
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The terminal's identity.
    #[must_use]
    pub fn id(&self) -> TerminalId {
        self.lock().terminal
    }

    /// Registers a viewer, which starts notified with its wake signalled.
    ///
    /// # Errors
    /// Returns `Stopped` once the terminal stopped, and `ViewerLimit` past
    /// its viewer limit.
    pub fn subscribe(&self) -> Result<Arc<ModelPort>, RuntimeError> {
        let mut state = self.lock();
        if state.stopped {
            return Err(RuntimeError::Stopped);
        }
        if state.slots.len() >= VIEWER_LIMIT {
            return Err(RuntimeError::ViewerLimit);
        }
        let id = ViewerId::in_runtime(
            state.runtime,
            state
                .next_viewer
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        );
        let (wake, receiver) = async_channel::bounded(1);
        let seen = state.status.revision;
        let mut slot = Slot {
            id,
            notified: true,
            publications: 1,
            reported: 0,
            seen,
            revoked: false,
            dropped: false,
            reserved: 0,
            wake,
            receiver,
            signalled: false,
        };
        slot.signal();
        state.slots.push(slot);
        drop(state);
        Ok(Arc::new(ModelPort {
            terminal: self.clone(),
            id,
        }))
    }

    /// The current status.
    #[must_use]
    pub fn status(&self) -> Arc<TerminalStatus> {
        Arc::clone(&self.lock().status)
    }

    /// The current snapshot, as a viewer would build it now.
    #[must_use]
    pub fn snapshot(&self) -> Arc<TerminalSnapshot> {
        Arc::new(self.lock().snapshot())
    }

    /// Whether the terminal stopped.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.lock().stopped
    }

    /// Writes a line of output: the screen scrolls by one row, so every
    /// other row keeps its allocation at a new position.
    pub fn write_line(&self, text: &str) {
        let mut state = self.lock();
        let columns = usize::from(state.size.columns);
        let first = state.rows.remove(0);
        state.history.push_back(first);
        if state.history.len() > HISTORY {
            state.history.pop_front();
        }
        state.rows.push(text_row(text, columns));
        state.generation += 1;
        state.publish(None);
    }

    /// Rewrites one row in place.
    pub fn set_row(&self, row: usize, text: &str) {
        let mut state = self.lock();
        let columns = usize::from(state.size.columns);
        let row = row % state.rows.len();
        state.rows[row] = text_row(text, columns);
        state.generation += 1;
        state.publish(None);
    }

    /// Moves the cursor, a change with no new rows.
    pub fn move_cursor(&self, column: u16) {
        let mut state = self.lock();
        state.cursor_column = column % state.size.columns;
        state.publish(None);
    }

    /// Resizes the grid, which makes every viewer's next update complete.
    pub fn resize(&self, size: GridSize) {
        let mut state = self.lock();
        state.size = size;
        state.rows = (0..size.rows)
            .map(|_| text_row("~", usize::from(size.columns)))
            .collect();
        state.history.clear();
        state.offset = 0;
        state.cursor_column = 0;
        state.geometry_revision += 1;
        state.generation += 1;
        state.publish(None);
    }

    /// Rings the bell.
    pub fn bell(&self) {
        self.lock().change_status(|status| status.bells += 1);
    }

    /// Records a failure.
    pub fn fail(&self, message: &str) {
        self.lock().change_status(|status| {
            let sequence =
                status.failures.last().map_or(1, |last| last.sequence + 1);
            status.failures.push(TerminalFailure {
                sequence,
                message: message.into(),
            });
            if status.failures.len()
                > huterm_protocol::TERMINAL_FAILURE_CAPACITY
            {
                status.failures.remove(0);
            }
        });
    }

    /// Records the root process's exit.
    pub fn exit(&self, code: u32) {
        self.lock().change_status(|status| {
            status.lifecycle = TerminalLifecycle::Exited(ExitStatus {
                code: Some(code),
                success: code == 0,
            });
        });
    }

    /// Stops the terminal's runtime: no viewer registers again, and every
    /// wake closes after its last signal.
    pub fn stop(&self) {
        let mut state = self.lock();
        state.stopped = true;
        for slot in &mut state.slots {
            slot.signal();
            slot.wake.close();
        }
    }

    /// Withdraws `viewer`'s authority: its queued messages are discarded.
    pub fn revoke(&self, viewer: ViewerId) {
        let mut state = self.lock();
        state.queue.retain(|(id, ..)| *id != viewer);
        if let Some(slot) = state.slot(viewer) {
            slot.revoked = true;
            slot.reserved = 0;
            slot.signal();
        }
    }

    /// Makes snapshot requests answer `error` until cleared with `None`.
    pub fn refuse_snapshots(&self, error: Option<RuntimeError>) {
        self.lock().refuse_snapshots = error;
    }

    /// Runs up to `count` queued ordered messages: input reaches the PTY
    /// log, focus the focus log. Returns how many ran.
    #[expect(
        clippy::must_use_candidate,
        reason = "callers drain for the effect and may ignore the count"
    )]
    pub fn drain(&self, count: usize) -> usize {
        let mut state = self.lock();
        let mut ran = 0;
        while ran < count {
            let Some((viewer, message, cost)) = state.queue.pop_front() else {
                break;
            };
            ran += 1;
            if let Some(slot) = state.slot(viewer) {
                slot.reserved -= cost;
            }
            match message {
                Ordered::Input { input, stamp } => {
                    state.pty.push(Written {
                        viewer,
                        input,
                        stamp,
                    });
                }
                Ordered::Focus(focused) => state.focus.push((viewer, focused)),
                _ => {}
            }
        }
        state.forget_dropped();
        ran
    }

    /// Ordered messages waiting in the queue.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.lock().queue.len()
    }

    /// Every input that reached the PTY, in order.
    #[must_use]
    pub fn written(&self) -> Vec<Written> {
        self.lock().pty.clone()
    }

    /// Every focus report that ran, in order.
    #[must_use]
    pub fn focus_reports(&self) -> Vec<(ViewerId, bool)> {
        self.lock().focus.clone()
    }

    /// Every scroll a snapshot applied, with the client's number for it.
    #[must_use]
    pub fn scrolls(&self) -> Vec<(ViewerId, u64)> {
        self.lock().scrolls.clone()
    }

    /// Takes the viewers whose wake signalled since the last call, which a
    /// deterministic driver hands to the connection.
    #[must_use]
    pub fn take_signalled(&self) -> Vec<ViewerId> {
        let mut state = self.lock();
        state
            .slots
            .iter_mut()
            .filter_map(|slot| {
                std::mem::take(&mut slot.signalled).then_some(slot.id)
            })
            .collect()
    }

    /// Live viewers, dropped ones with queued messages included.
    #[must_use]
    pub fn viewers(&self) -> usize {
        self.lock().slots.len()
    }
}

impl State {
    fn slot(&mut self, viewer: ViewerId) -> Option<&mut Slot> {
        self.slots.iter_mut().find(|slot| slot.id == viewer)
    }

    /// Notifies every viewer but `skip` that a snapshot is due; one already
    /// notified gets no further wake until its snapshot is built.
    fn publish(&mut self, skip: Option<ViewerId>) {
        self.revision += 1;
        for slot in &mut self.slots {
            if Some(slot.id) == skip || slot.notified {
                continue;
            }
            slot.notified = true;
            slot.publications += 1;
            slot.signal();
        }
    }

    fn change_status(&mut self, change: impl FnOnce(&mut TerminalStatus)) {
        let mut status = (*self.status).clone();
        change(&mut status);
        status.revision += 1;
        self.status = Arc::new(status);
        for slot in &mut self.slots {
            slot.signal();
        }
    }

    fn forget_dropped(&mut self) {
        let queue = &self.queue;
        self.slots.retain(|slot| {
            !slot.dropped || queue.iter().any(|(id, ..)| *id == slot.id)
        });
    }

    fn snapshot(&self) -> TerminalSnapshot {
        let rows = usize::from(self.size.rows);
        let all: Vec<&Arc<TerminalRow>> =
            self.history.iter().chain(self.rows.iter()).collect();
        let offset = self.offset.min(self.history.len());
        let end = all.len() - offset;
        let visible = all[end - rows..end]
            .iter()
            .map(|row| Arc::clone(row))
            .collect();
        TerminalSnapshot {
            terminal_id: self.terminal,
            generation: self.generation,
            revision: self.revision,
            geometry_revision: self.geometry_revision,
            size: self.size,
            rows: visible,
            cursor: (offset == 0).then_some(huterm_protocol::Cursor {
                row: self.size.rows - 1,
                column: self.cursor_column,
                shape: huterm_protocol::CursorShape::Block,
            }),
            modes: TerminalModes::default(),
            viewport: Viewport {
                bottom_offset: offset,
            },
            history_size: self.history.len(),
            cursor_color: None,
            default_background: None,
        }
    }
}

/// A viewer of a [`ModelTerminal`].
pub struct ModelPort {
    terminal: ModelTerminal,
    id: ViewerId,
}

impl fmt::Debug for ModelPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelPort")
            .field("id", &self.id)
            .field("terminal", &self.terminal)
            .finish()
    }
}

impl Drop for ModelPort {
    fn drop(&mut self) {
        let mut state = self.terminal.lock();
        if let Some(slot) = state.slot(self.id) {
            slot.dropped = true;
        }
        state.forget_dropped();
    }
}

struct ModelWake(async_channel::Receiver<()>);

impl WakeSource for ModelWake {
    fn wait(&self) -> Wake {
        let receiver = self.0.clone();
        Box::pin(async move { receiver.recv().await.is_ok() })
    }
}

fn ready<T: Send + 'static>(value: Result<T, RuntimeError>) -> Reply<T> {
    Box::pin(async move { value })
}

impl ViewerPort for ModelPort {
    fn id(&self) -> ViewerId {
        self.id
    }

    fn poll(&self) -> ViewerUpdate {
        let mut state = self.terminal.lock();
        let status = Arc::clone(&state.status);
        let Some(slot) = state.slot(self.id) else {
            return ViewerUpdate::default();
        };
        let mut update = ViewerUpdate {
            revoked: slot.revoked,
            ..ViewerUpdate::default()
        };
        if slot.notified && slot.reported != slot.publications {
            slot.reported = slot.publications;
            update.invalidated = Some(0);
        }
        if status.revision > slot.seen {
            slot.seen = status.revision;
            update.status = Some(status);
        }
        update
    }

    fn wake(&self) -> Box<dyn WakeSource> {
        let state = self.terminal.lock();
        let receiver = state
            .slots
            .iter()
            .find(|slot| slot.id == self.id)
            .map_or_else(
                || {
                    let (_, receiver) = async_channel::bounded(1);
                    receiver
                },
                |slot| slot.receiver.clone(),
            );
        Box::new(ModelWake(receiver))
    }

    fn offer(&self, message: Ordered) -> Result<(), (RuntimeError, Ordered)> {
        let mut state = self.terminal.lock();
        if state.stopped {
            return Err((RuntimeError::Stopped, message));
        }
        let limits = state.limits;
        let full = state.queue.len() >= limits.queue;
        let cost = match &message {
            Ordered::Input { input, .. } => input_cost(input),
            _ => 0,
        };
        let id = self.id;
        let Some(slot) = state.slot(id) else {
            return Err((RuntimeError::Revoked, message));
        };
        if slot.revoked {
            return Err((RuntimeError::Revoked, message));
        }
        if full || slot.reserved + cost > limits.viewer_input {
            return Err((RuntimeError::Busy, message));
        }
        slot.reserved += cost;
        state.queue.push_back((id, message, cost));
        Ok(())
    }

    fn snapshot(
        &self,
        scroll: Option<(ScrollCommand, u64)>,
        point: Option<MousePosition>,
    ) -> Reply<SnapshotOutcome> {
        let mut state = self.terminal.lock();
        if let Some(error) = &state.refuse_snapshots {
            let error = match error {
                RuntimeError::Stopped => RuntimeError::Stopped,
                RuntimeError::Revoked => RuntimeError::Revoked,
                RuntimeError::Busy => RuntimeError::Busy,
                _ => RuntimeError::NotPermitted,
            };
            return ready(Err(error));
        }
        if state.stopped {
            return ready(Err(RuntimeError::Stopped));
        }
        let id = self.id;
        if state.slot(id).is_none_or(|slot| slot.revoked) {
            return ready(Err(RuntimeError::Revoked));
        }
        if let Some((command, number)) = scroll {
            let history = state.history.len();
            let offset = match command {
                ScrollCommand::Relative(rows) => {
                    let offset =
                        i64::try_from(state.offset).unwrap_or(i64::MAX);
                    usize::try_from(offset.saturating_add(rows).max(0))
                        .unwrap_or(0)
                }
                ScrollCommand::Absolute(offset) => offset,
                ScrollCommand::Live => 0,
            }
            .min(history);
            state.scrolls.push((id, number));
            if offset != state.offset {
                state.offset = offset;
                state.publish(Some(id));
            }
        }
        if let Some(slot) = state.slot(id) {
            slot.notified = false;
        }
        let snapshot = Arc::new(state.snapshot());
        ready(Ok(SnapshotOutcome {
            snapshot,
            link: point.map(|_| LinkLookup::NoMatch),
        }))
    }

    fn selection(
        &self,
        generation: u64,
        range: BufferRange,
    ) -> Reply<Option<String>> {
        let state = self.terminal.lock();
        let text = (generation == state.generation).then(|| {
            format!(
                "{}..{}",
                range.start.rows_from_live_bottom,
                range.end.rows_from_live_bottom
            )
        });
        ready(Ok(text))
    }

    fn select_units(
        &self,
        generation: u64,
        anchor: BufferPoint,
        head: BufferPoint,
        unit: SelectionUnit,
    ) -> Reply<Option<BufferRange>> {
        let state = self.terminal.lock();
        let _ = unit;
        ready(Ok((generation == state.generation)
            .then(|| BufferRange::ordered(anchor, head))))
    }
}

/// Model terminals by the server's identity for them, and the subscription
/// a model executor answers.
#[derive(Clone, Debug, Default)]
pub struct ModelTerminals {
    /// By the terminal's number, which a client's identity for it shares.
    terminals: Arc<Mutex<HashMap<u64, ModelTerminal>>>,
}

impl ModelTerminals {
    /// Adds `terminal`.
    pub fn insert(&self, terminal: ModelTerminal) {
        self.terminals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(terminal.id().get(), terminal);
    }

    /// The terminal `id`, if any.
    #[must_use]
    pub fn get(&self, id: TerminalId) -> Option<ModelTerminal> {
        self.terminals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id.get())
            .cloned()
    }

    /// Runs `job` against the model terminals: subscriptions register
    /// model viewers, viewer jobs run on their ports, and every other job
    /// runs against `server`'s `Mux`.
    #[must_use]
    pub fn execute(
        &self,
        server: &crate::Server,
        job: crate::Job,
    ) -> crate::Completion {
        use crate::work::{Operation, RequestJob, Subscription};
        match job {
            crate::Job::Request(RequestJob {
                operation:
                    Operation::Subscribe {
                        terminal, options, ..
                    },
                ..
            }) => {
                let _ = options;
                let Some(model) = self.get(terminal) else {
                    return crate::Completion::Subscribed(Err(
                        huterm_wire::error::WireError::new(
                            huterm_wire::error::ErrorCode::StaleTarget,
                            "no such terminal",
                        ),
                    ));
                };
                crate::Completion::Subscribed(match model.subscribe() {
                    Ok(port) => Ok(Subscription::Viewer {
                        status: model.status(),
                        port,
                    }),
                    Err(RuntimeError::Stopped) => {
                        Ok(Subscription::Stopped(model.status()))
                    }
                    Err(error) => Err(huterm_wire::error::WireError::new(
                        crate::runtime::runtime_code(&error),
                        error.to_string(),
                    )),
                })
            }
            job => crate::runtime::execute(server, job),
        }
    }
}
