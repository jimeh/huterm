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
//!
//! A viewer subscribed with host effects gets a recipient with core's
//! rules in miniature: per-kind permissions under the client's numbered
//! generations, one recipient chosen per effect (the most recent eligible
//! one, never redirected), and the terminal's effect and byte limits, held
//! by each effect until it is dropped.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use huterm_core::{RuntimeError, ViewerUpdate};
use huterm_protocol::{
    BufferPoint, BufferRange, Cell, CellColor, CellStyle, CellText, ExitStatus,
    GridSize, InputStamp, LinkLookup, MousePosition, RuntimeId, ScrollCommand,
    SelectionUnit, TerminalFailure, TerminalId, TerminalInput,
    TerminalLifecycle, TerminalModes, TerminalRow, TerminalSnapshot,
    TerminalStatus, ViewerId, Viewport,
};

use huterm_core::{HostEffectClientOrigin, HostEffectViewerOptions};
use huterm_protocol::{ClipboardDestination, HostEffectKind};
use huterm_wire::effect::{
    EffectPayload, EffectPayloadRef, INITIAL_EFFECT_GENERATION,
};

use crate::viewer::{
    Ordered, PendingEffect, Reply, SnapshotOutcome, ViewerPort, Wake,
    WakeSource,
};

/// Effects one model terminal holds, as core's terminal does.
const EFFECT_LIMIT: usize = 8;

/// Effect bytes one model terminal holds, as core's terminal does.
const EFFECT_BYTE_LIMIT: usize = 64 << 20;

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
    /// The viewer's host-effect recipient, if it has one.
    effects: Option<Recipient>,
    /// When the viewer was last active, as core's activity ordinal: the
    /// most recently active eligible recipient receives each effect.
    activity: u64,
}

/// A model viewer's host-effect recipient.
#[derive(Debug)]
struct Recipient {
    /// Whether the recipient may receive effects at all: a local desktop
    /// that was not withdrawn.
    eligible: bool,
    allowed: [bool; HostEffectKind::ALL.len()],
    generation: [u64; HostEffectKind::ALL.len()],
    queue: VecDeque<ModelEffect>,
}

impl Recipient {
    fn is_current(&self, kind: HostEffectKind, generation: u64) -> bool {
        self.eligible
            && self.allowed[kind.index()]
            && self.generation[kind.index()] == generation
    }
}

/// What a model terminal's queued effects hold.
#[derive(Debug, Default)]
struct EffectBudget {
    effects: AtomicUsize,
    bytes: AtomicUsize,
}

/// One effect's hold on its terminal's budget, released when dropped.
#[derive(Debug)]
struct Reservation {
    budget: Arc<EffectBudget>,
    bytes: usize,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.effects.fetch_sub(1, Ordering::AcqRel);
        self.budget.bytes.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// An effect a model terminal admitted for one viewer.
#[derive(Debug)]
pub struct ModelEffect {
    payload: EffectPayload,
    generation: u64,
    terminal: TerminalId,
    viewer: ViewerId,
    state: Weak<Mutex<State>>,
    _reservation: Reservation,
}

impl PendingEffect for ModelEffect {
    fn is_current(&self) -> bool {
        let Some(state) = self.state.upgrade() else {
            return false;
        };
        let state = state.lock().unwrap_or_else(PoisonError::into_inner);
        state.slots.iter().any(|slot| {
            slot.id == self.viewer
                && !slot.revoked
                && !slot.dropped
                && slot.effects.as_ref().is_some_and(|recipient| {
                    recipient.is_current(self.payload.kind(), self.generation)
                })
        })
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn terminal(&self) -> TerminalId {
        self.terminal
    }

    fn payload(&self) -> Option<EffectPayloadRef<'_>> {
        Some(self.payload.as_ref())
    }
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
    /// Snapshot requests with a scroll answer this error instead, while
    /// set, as core refuses a scroll from a viewer without the viewport
    /// capability before it sends anything.
    refuse_scrolls: Option<RuntimeError>,
    /// The text every selection answers, when set.
    selection: Option<String>,
    /// What the terminal's queued and sending effects hold.
    effect_budget: Arc<EffectBudget>,
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
                refuse_scrolls: None,
                selection: None,
                effect_budget: Arc::default(),
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
        self.subscribe_with(None)
    }

    /// Registers a viewer as [`Self::subscribe`] does, with a host-effect
    /// recipient when `effects` asks for one. Only a local desktop's is
    /// eligible, as in core.
    ///
    /// # Errors
    /// As [`Self::subscribe`].
    pub fn subscribe_with(
        &self,
        effects: Option<&HostEffectViewerOptions>,
    ) -> Result<Arc<ModelPort>, RuntimeError> {
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
            activity: 0,
            effects: effects.map(|options| {
                let permissions = options.options.permissions;
                Recipient {
                    eligible: options.options.origin
                        == HostEffectClientOrigin::LocalDesktop
                        && options.options.writable,
                    allowed: HostEffectKind::ALL
                        .map(|kind| permissions.allows(kind)),
                    generation: [INITIAL_EFFECT_GENERATION;
                        HostEffectKind::ALL.len()],
                    queue: VecDeque::new(),
                }
            }),
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

    /// Admits a clipboard write, as OSC 52 asks, for the most recently
    /// registered viewer whose recipient permits it, and wakes that viewer.
    /// Returns whether it was admitted: false without such a recipient, or
    /// with the terminal's effect or byte limit reached.
    #[must_use]
    pub fn clipboard(&self, text: &str) -> bool {
        self.admit_clipboard(text).is_some()
    }

    /// Admits a clipboard write as [`Self::clipboard`] does, returning the
    /// viewer whose recipient it went to.
    #[must_use]
    pub fn admit_clipboard(&self, text: &str) -> Option<ViewerId> {
        self.admit(EffectPayload::ClipboardWrite {
            destination: ClipboardDestination::System,
            text: text.to_owned(),
        })
    }

    /// Admits a notification as [`Self::clipboard`] admits a write.
    #[must_use]
    pub fn notify(&self, title: &str, body: &str) -> bool {
        self.admit(EffectPayload::Notification {
            title: title.to_owned(),
            body: body.to_owned(),
        })
        .is_some()
    }

    fn admit(&self, payload: EffectPayload) -> Option<ViewerId> {
        let weak = Arc::downgrade(&self.state);
        let mut state = self.lock();
        if state.stopped {
            return None;
        }
        let kind = payload.kind();
        let bytes = match &payload {
            EffectPayload::ClipboardWrite { text, .. } => text.len(),
            EffectPayload::Notification { title, body } => {
                title.len() + body.len()
            }
        };
        let budget = Arc::clone(&state.effect_budget);
        let terminal = state.terminal;
        // The most recently active eligible recipient, the latest
        // registered on a tie.
        let slot = state
            .slots
            .iter_mut()
            .rev()
            .filter(|slot| {
                !slot.revoked
                    && !slot.dropped
                    && slot.effects.as_ref().is_some_and(|recipient| {
                        recipient.eligible && recipient.allowed[kind.index()]
                    })
            })
            .reduce(|best, slot| {
                if slot.activity > best.activity {
                    slot
                } else {
                    best
                }
            })?;
        if budget.effects.load(Ordering::Acquire) >= EFFECT_LIMIT
            || budget.bytes.load(Ordering::Acquire) + bytes > EFFECT_BYTE_LIMIT
        {
            return None;
        }
        budget.effects.fetch_add(1, Ordering::AcqRel);
        budget.bytes.fetch_add(bytes, Ordering::AcqRel);
        let viewer = slot.id;
        let recipient = slot.effects.as_mut()?;
        let generation = recipient.generation[kind.index()];
        recipient.queue.push_back(ModelEffect {
            payload,
            generation,
            terminal,
            viewer,
            state: weak,
            _reservation: Reservation { budget, bytes },
        });
        slot.signal();
        Some(viewer)
    }

    /// Marks `viewer` as the most recently active, as typing or a focus
    /// gain does in core, so the next effect selects it if it is eligible.
    pub fn activate(&self, viewer: ViewerId) {
        let mut state = self.lock();
        let next = state
            .slots
            .iter()
            .map(|slot| slot.activity)
            .max()
            .unwrap_or(0)
            + 1;
        if let Some(slot) = state.slot(viewer) {
            slot.activity = next;
        }
    }

    /// The effects and bytes the terminal's queued and sending effects
    /// hold.
    #[must_use]
    pub fn effects_held(&self) -> (usize, usize) {
        let state = self.lock();
        (
            state.effect_budget.effects.load(Ordering::Acquire),
            state.effect_budget.bytes.load(Ordering::Acquire),
        )
    }

    /// Withdraws `viewer`'s authority: its queued messages are discarded.
    pub fn revoke(&self, viewer: ViewerId) {
        let mut state = self.lock();
        state.queue.retain(|(id, ..)| *id != viewer);
        if let Some(slot) = state.slot(viewer) {
            slot.revoked = true;
            slot.reserved = 0;
            if let Some(recipient) = slot.effects.as_mut() {
                recipient.eligible = false;
                recipient.queue.clear();
            }
            slot.signal();
        }
    }

    /// Makes snapshot requests with a scroll answer `error` until cleared
    /// with `None`.
    pub fn refuse_scrolls(&self, error: Option<RuntimeError>) {
        self.lock().refuse_scrolls = error;
    }

    /// Makes every selection answer `text`.
    pub fn set_selection(&self, text: String) {
        self.lock().selection = Some(text);
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
            commands: std::sync::Arc::new([]),
            sticky_prompt: None,
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

/// What one input reserves against a model viewer's budget: its text's
/// bytes, or a few for a key or mouse report, as core reserves.
fn core_cost(input: &TerminalInput) -> usize {
    match input {
        TerminalInput::Text(text) | TerminalInput::Paste(text) => text.len(),
        TerminalInput::Character { text, key, .. } => {
            text.len() + key.as_ref().map_or(0, |key| key.text.len())
        }
        _ => 8,
    }
    .max(1)
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

    fn next_effect(&self) -> Option<Box<dyn PendingEffect>> {
        let mut state = self.terminal.lock();
        let slot = state.slot(self.id)?;
        let usable = !slot.revoked && !slot.dropped;
        let recipient = slot.effects.as_mut()?;
        while let Some(effect) = recipient.queue.pop_front() {
            if usable
                && recipient
                    .is_current(effect.payload.kind(), effect.generation)
            {
                return Some(Box::new(effect));
            }
        }
        None
    }

    fn set_effect_permission(
        &self,
        kind: HostEffectKind,
        allowed: bool,
        generation: u64,
    ) -> bool {
        let mut state = self.terminal.lock();
        let Some(recipient) =
            state.slot(self.id).and_then(|slot| slot.effects.as_mut())
        else {
            return false;
        };
        if generation <= recipient.generation[kind.index()] {
            return false;
        }
        recipient.generation[kind.index()] = generation;
        recipient.allowed[kind.index()] = allowed;
        if !allowed {
            recipient
                .queue
                .retain(|effect| effect.payload.kind() != kind);
        }
        true
    }

    fn revoke_effects(&self) {
        let mut state = self.terminal.lock();
        if let Some(recipient) =
            state.slot(self.id).and_then(|slot| slot.effects.as_mut())
        {
            recipient.eligible = false;
            recipient.queue.clear();
        }
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
        // Core counts input by its text, as `huterm-core` does, not by
        // the wire's window cost.
        let cost = match &message {
            Ordered::Input { input, .. } => core_cost(input),
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
        if let Some(error) = &state.refuse_scrolls
            && scroll.is_some()
        {
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
                // The model records no prompts, so a prompt jump finds
                // none and stays.
                ScrollCommand::Prompt(_) => state.offset,
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
            state.selection.clone().unwrap_or_else(|| {
                format!(
                    "{}..{}",
                    range.start.rows_from_live_bottom,
                    range.end.rows_from_live_bottom
                )
            })
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
                let Some(model) = self.get(terminal) else {
                    return crate::Completion::Subscribed(Err(
                        huterm_wire::error::WireError::new(
                            huterm_wire::error::ErrorCode::StaleTarget,
                            "no such terminal",
                        ),
                    ));
                };
                crate::Completion::Subscribed(
                    match model.subscribe_with(options.host_effects.as_ref()) {
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
                    },
                )
            }
            job => crate::runtime::execute(server, job),
        }
    }
}
