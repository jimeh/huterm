//! Terminal viewers: per-client registrations that display and drive one
//! terminal, with their own wake, capabilities, and status baseline.
//!
//! The registry is shared between the runtime owner thread, Mux, and viewer
//! handles. Only the owner thread publishes; everything else registers,
//! revokes, or reads.

mod arbitration;
mod status;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use huterm_protocol::{
    AttachmentId, BufferRange, CellSize, ExitStatus, GridSize, InputStamp,
    MousePosition, RuntimeId, ScrollCommand, TerminalFailure, TerminalId,
    TerminalInput, TerminalLifecycle, TerminalPresentation, TerminalSnapshot,
    TerminalStatus, ViewerCapabilities, ViewerId,
};

pub(crate) use arbitration::{Arbiter, Effects, Report};
pub(crate) use status::StatusPublisher;

use crate::host_effects::{
    DesktopHostEffectClient, HostEffectRecipient, HostEffectRecipientOptions,
};
use crate::terminal::{
    BufferEdit, RefusedInput, RuntimeClient, RuntimeControl, RuntimeError,
    RuntimeMessage, SelectionRequest, SnapshotRequest,
};

/// Live viewers one terminal admits.
pub(crate) const VIEWER_LIMIT: usize = 32;

/// Numbers viewers across every terminal in the process, so a viewer ID
/// names one registration within its runtime scope.
static NEXT_VIEWER: AtomicU64 = AtomicU64::new(1);

/// Host-effect registration for a viewer that can receive effects such as
/// clipboard writes.
#[derive(Clone, Debug)]
pub struct HostEffectViewerOptions {
    /// The desktop process budget the recipient draws from.
    pub process: DesktopHostEffectClient,
    /// Connection boundary and policy.
    pub options: HostEffectRecipientOptions,
}

/// What a new viewer may do, and the state it starts with.
#[derive(Clone, Debug)]
pub struct ViewerOptions {
    /// Requests the viewer may make. The `host_effects` flag is granted
    /// exactly when [`Self::host_effects`] is supplied.
    pub capabilities: ViewerCapabilities,
    /// Host-effect registration, when the viewer receives host effects.
    pub host_effects: Option<HostEffectViewerOptions>,
    /// Whether the viewer starts focused.
    pub focused: bool,
    /// The grid and cell size the viewer starts requesting.
    pub geometry: Option<(GridSize, CellSize)>,
    /// The presentation the viewer starts submitting.
    pub presentation: Option<TerminalPresentation>,
    /// Set only by the client that created the terminal: the viewer then
    /// treats an exit before its first poll as an exit transition.
    pub spawned: bool,
}

impl ViewerOptions {
    /// Options for a viewer with `capabilities` and no initial state.
    #[must_use]
    pub fn new(capabilities: ViewerCapabilities) -> Self {
        Self {
            capabilities,
            host_effects: None,
            focused: false,
            geometry: None,
            presentation: None,
            spawned: false,
        }
    }
}

/// State a slot carries until the owner thread first sees it.
#[derive(Debug)]
pub(crate) struct Initial {
    pub(crate) focused: bool,
    pub(crate) geometry: Option<(GridSize, CellSize)>,
    pub(crate) presentation: Option<TerminalPresentation>,
}

/// One viewer's shared registration. Every request carries it, so the owner
/// thread checks revocation and capabilities without a lookup.
#[derive(Debug)]
pub(crate) struct Slot {
    pub(crate) id: ViewerId,
    /// Registration order; ties in control go to the latest.
    pub(crate) order: u64,
    pub(crate) attachment: Option<AttachmentId>,
    pub(crate) capabilities: ViewerCapabilities,
    wake: async_channel::Sender<()>,
    notified: AtomicBool,
    invalidated_at: Mutex<Option<Instant>>,
    revoked: AtomicBool,
    dropped: AtomicBool,
    /// Ordered messages sent but not yet processed or refused.
    queued: AtomicUsize,
    /// The viewer's activity ordinal, shared with its host-effect recipient.
    pub(crate) activity: Arc<AtomicU64>,
    /// The number of this viewer's latest scroll the owner thread applied.
    /// Scroll bookkeeping lives on the slot, which every request carries,
    /// so it holds before the viewer is reconciled and after it is
    /// finalized.
    applied_scroll: AtomicU64,
    /// Scrolls up to this number were sent before typing that returned the
    /// viewport to live.
    live_through: AtomicU64,
    initial: Mutex<Option<Initial>>,
}

impl Slot {
    pub(crate) fn is_revoked(&self) -> bool {
        self.revoked.load(Ordering::Acquire)
    }

    pub(crate) fn is_dropped(&self) -> bool {
        self.dropped.load(Ordering::Acquire)
    }

    /// Counts a message before it is sent; pair with [`Self::settled`].
    fn queue(&self) {
        self.queued.fetch_add(1, Ordering::AcqRel);
    }

    /// Records that a counted message was refused or processed. Returns
    /// whether none remain.
    pub(crate) fn settled(&self) -> bool {
        self.queued.fetch_sub(1, Ordering::AcqRel) == 1
    }

    pub(crate) fn queued(&self) -> usize {
        self.queued.load(Ordering::Acquire)
    }

    pub(crate) fn take_initial(&self) -> Option<Initial> {
        self.initial
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Clears the viewer's notification as its snapshot is built, returning
    /// the earliest invalidation that snapshot is the first to include.
    pub(crate) fn snapshot_built(&self) -> Option<Instant> {
        self.notified.store(false, Ordering::Release);
        self.invalidated_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    fn signal(&self) {
        let _ = self.wake.try_send(());
    }

    /// Records the number of a scroll the owner thread applied.
    pub(crate) fn scrolled(&self, number: u64) {
        self.applied_scroll.fetch_max(number, Ordering::AcqRel);
    }

    /// Whether typing stamped with `stamp` returns the shared viewport to
    /// live: not when this viewer has a later scroll applied. When it does,
    /// the viewer's scrolls sent before that typing are superseded.
    pub(crate) fn return_to_live(&self, stamp: InputStamp) -> bool {
        if !self.capabilities.viewport
            || self.applied_scroll.load(Ordering::Acquire) > stamp.scrolls
        {
            return false;
        }
        self.live_through.fetch_max(stamp.scrolls, Ordering::AcqRel);
        true
    }

    /// Whether a scroll was sent before typing that has already returned
    /// the viewport to live. Scrolls are controls and typing is a message,
    /// so a scroll that arrives after a turn drains its controls can be
    /// applied after typing the user produced later.
    pub(crate) fn scroll_superseded(&self, number: u64) -> bool {
        number <= self.live_through.load(Ordering::Acquire)
    }
}

#[derive(Debug)]
struct RegistryState {
    closed: bool,
    next: u64,
    slots: Vec<Arc<Slot>>,
    status: Arc<TerminalStatus>,
}

/// The viewers of one terminal and the state they read.
#[derive(Debug)]
pub(crate) struct Registry {
    terminal_id: TerminalId,
    runtime: RuntimeId,
    owner: Arc<crate::wake::Wake>,
    changed: AtomicBool,
    generation: AtomicU64,
    state: Mutex<RegistryState>,
}

/// Why a viewer could not register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RegisterError {
    Closed,
    Full,
}

impl Registry {
    pub(crate) fn new(
        terminal_id: TerminalId,
        runtime: RuntimeId,
        owner: Arc<crate::wake::Wake>,
    ) -> Self {
        Self {
            terminal_id,
            runtime,
            owner,
            changed: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            state: Mutex::new(RegistryState {
                closed: false,
                next: 0,
                slots: Vec::new(),
                status: Arc::new(TerminalStatus::default()),
            }),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, RegistryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Registers a slot that starts notified with its wake signalled, so the
    /// viewer cannot miss a publication between registration and its first
    /// poll. The wake and activity ordinal are created by the caller, so a
    /// host-effect recipient can share them before the slot exists.
    pub(crate) fn register(
        &self,
        attachment: Option<AttachmentId>,
        capabilities: ViewerCapabilities,
        initial: Initial,
        wake: async_channel::Sender<()>,
        activity: Arc<AtomicU64>,
    ) -> Result<Arc<Slot>, RegisterError> {
        let mut state = self.state();
        if state.closed {
            return Err(RegisterError::Closed);
        }
        if state.slots.len() >= VIEWER_LIMIT {
            return Err(RegisterError::Full);
        }
        state.next += 1;
        let slot = Arc::new(Slot {
            id: ViewerId::in_runtime(
                self.runtime,
                NEXT_VIEWER.fetch_add(1, Ordering::Relaxed),
            ),
            order: state.next,
            attachment,
            capabilities,
            wake,
            notified: AtomicBool::new(true),
            invalidated_at: Mutex::new(None),
            revoked: AtomicBool::new(false),
            dropped: AtomicBool::new(false),
            queued: AtomicUsize::new(0),
            activity,
            applied_scroll: AtomicU64::new(0),
            live_through: AtomicU64::new(0),
            initial: Mutex::new(Some(initial)),
        });
        slot.signal();
        state.slots.push(Arc::clone(&slot));
        drop(state);
        self.mark_changed();
        Ok(slot)
    }

    /// Registers a slot with its own wake, returning the wake's receiver.
    #[cfg(test)]
    pub(crate) fn register_with_wake(
        &self,
        attachment: Option<AttachmentId>,
        capabilities: ViewerCapabilities,
        initial: Initial,
    ) -> Result<(Arc<Slot>, async_channel::Receiver<()>), RegisterError> {
        let (wake, signal) = async_channel::bounded(1);
        self.register(attachment, capabilities, initial, wake, Arc::default())
            .map(|slot| (slot, signal))
    }

    /// Asks the owner thread to reconcile its viewer table.
    fn mark_changed(&self) {
        self.changed.store(true, Ordering::Release);
        self.owner.notify();
    }

    pub(crate) fn has_pending_change(&self) -> bool {
        self.changed.load(Ordering::Acquire)
    }

    pub(crate) fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }

    /// The registered slots, for the owner thread to reconcile.
    pub(crate) fn slots(&self) -> Vec<Arc<Slot>> {
        self.state().slots.clone()
    }

    /// Frees a finalized slot so it no longer counts toward the limit, and
    /// closes its wake: a revoked viewer's waiter receives the pending
    /// revocation signal, then stops.
    pub(crate) fn remove(&self, id: ViewerId) {
        self.state().slots.retain(|slot| {
            let keep = slot.id != id;
            if !keep {
                slot.wake.close();
            }
            keep
        });
    }

    /// Withdraws the authority of every viewer `matches` selects. Their
    /// queued requests are discarded, and the owner thread finalizes them
    /// on its next turn.
    pub(crate) fn revoke_where(&self, matches: impl Fn(&Slot) -> bool) {
        let state = self.state();
        let mut any = false;
        for slot in state.slots.iter().filter(|slot| matches(slot)) {
            if !slot.revoked.swap(true, Ordering::AcqRel) {
                slot.signal();
                any = true;
            }
        }
        drop(state);
        if any {
            self.mark_changed();
        }
    }

    pub(crate) fn revoke_attachment(&self, attachment: AttachmentId) {
        self.revoke_where(|slot| slot.attachment == Some(attachment));
    }

    pub(crate) fn revoke_all(&self) {
        self.revoke_where(|_| true);
    }

    /// Notifies every viewer except `skip` that a snapshot is due. A viewer
    /// already notified gets no further wake until its snapshot is built.
    pub(crate) fn publish(&self, skip: Option<ViewerId>, generation: u64) {
        self.generation.store(generation, Ordering::Release);
        let state = self.state();
        for slot in &state.slots {
            if Some(slot.id) == skip
                || slot.notified.swap(true, Ordering::AcqRel)
            {
                continue;
            }
            *slot
                .invalidated_at
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
            slot.signal();
        }
    }

    /// Replaces the published status and wakes every viewer.
    pub(crate) fn publish_status(&self, status: Arc<TerminalStatus>) {
        let mut state = self.state();
        state.status = status;
        for slot in &state.slots {
            slot.signal();
        }
    }

    pub(crate) fn status(&self) -> Arc<TerminalStatus> {
        Arc::clone(&self.state().status)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Rejects new viewers and closes every wake. The final status stays
    /// readable, so a viewer finishes its last update before retiring.
    pub(crate) fn close(&self) {
        let mut state = self.state();
        state.closed = true;
        for slot in &state.slots {
            slot.wake.close();
        }
    }

    pub(crate) fn terminal_id(&self) -> TerminalId {
        self.terminal_id
    }
}

/// What a viewer learned from one poll.
#[derive(Clone, Debug, Default)]
pub struct ViewerUpdate {
    /// The current status, when it changed since the previous poll.
    pub status: Option<Arc<TerminalStatus>>,
    /// The current content generation, while a snapshot is due.
    pub invalidated: Option<u64>,
    /// The root process's exit, the first time this viewer sees it
    /// happen. A viewer that started after the exit never reports it.
    pub exited: Option<ExitStatus>,
    /// Bells rung since the previous poll.
    pub bells: u64,
    /// Failures reported since the previous poll, oldest first.
    pub failures: Vec<TerminalFailure>,
    /// Failures since the previous poll that the bounded log dropped.
    pub missed_failures: u64,
    /// The viewer's authority was withdrawn; its requests now fail with
    /// [`RuntimeError::Revoked`].
    pub revoked: bool,
}

/// A handle on one viewer's wake, for the task that drains the viewer. It
/// shares the viewer's single signal, so give it one waiter.
#[derive(Clone, Debug)]
pub struct ViewerWake {
    wake: async_channel::Receiver<()>,
}

impl ViewerWake {
    /// Completes once the terminal published something the viewer has not
    /// polled. See [`TerminalViewer::wait`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Stopped`] once the terminal stopped or the
    /// revoked viewer was finalized.
    pub async fn wait(&self) -> Result<(), RuntimeError> {
        self.wake.recv().await.map_err(|_| RuntimeError::Stopped)
    }
}

#[derive(Debug)]
struct Baseline {
    revision: Option<u64>,
    running: bool,
    bells: u64,
    failure: u64,
}

/// One client's registration to display and drive a terminal.
///
/// Dropping the viewer unregisters it without blocking: requests it already
/// sent still run, then the runtime releases any mouse button it held,
/// reports its focus loss, and hands control to another viewer.
#[derive(Debug)]
pub struct TerminalViewer {
    slot: Arc<Slot>,
    client: RuntimeClient,
    registry: Arc<Registry>,
    wake: async_channel::Receiver<()>,
    scrolls: AtomicU64,
    baseline: Mutex<Baseline>,
    host_effects: Option<HostEffectRecipient>,
}

impl TerminalViewer {
    pub(crate) fn subscribe(
        client: RuntimeClient,
        attachment: Option<AttachmentId>,
        options: ViewerOptions,
    ) -> Result<Self, RuntimeError> {
        let registry = client.registry();
        let capabilities = ViewerCapabilities {
            host_effects: options.host_effects.is_some(),
            ..options.capabilities
        };
        let (wake_sender, wake) = async_channel::bounded(1);
        let activity = Arc::new(AtomicU64::new(0));
        // The recipient registers first: a slot the owner thread could see
        // must never need undoing, or its initial focus and size would apply
        // and then be withdrawn.
        let host_effects = match options.host_effects {
            Some(host) => {
                let sink = client.host_effect_sink();
                let recipient = sink.register(
                    attachment,
                    &host.process,
                    host.options,
                    crate::host_effects::RecipientLink {
                        activity: Some(wake_sender.clone()),
                        ordinal: Arc::clone(&activity),
                    },
                );
                Some(recipient.ok_or(if sink.is_closed() {
                    RuntimeError::Stopped
                } else {
                    RuntimeError::ViewerLimit
                })?)
            }
            None => None,
        };
        let slot = registry
            .register(
                attachment,
                capabilities,
                Initial {
                    focused: options.focused,
                    geometry: options.geometry.map(|(grid, cell)| {
                        (GridSize::clamped(grid.columns, grid.rows), cell)
                    }),
                    presentation: options.presentation,
                },
                wake_sender,
                activity,
            )
            .map_err(|error| match error {
                RegisterError::Closed => RuntimeError::Stopped,
                RegisterError::Full => RuntimeError::ViewerLimit,
            })?;
        let status = registry.status();
        let baseline = if options.spawned {
            Baseline {
                revision: None,
                running: true,
                bells: 0,
                failure: 0,
            }
        } else {
            Baseline {
                revision: None,
                running: status.lifecycle == TerminalLifecycle::Running,
                bells: status.bells,
                failure: status.last_failure(),
            }
        };
        Ok(Self {
            slot,
            client,
            registry,
            wake,
            scrolls: AtomicU64::new(0),
            baseline: Mutex::new(baseline),
            host_effects,
        })
    }

    /// The viewer's identity.
    #[must_use]
    pub fn id(&self) -> ViewerId {
        self.slot.id
    }

    /// The terminal this viewer displays.
    #[must_use]
    pub fn terminal_id(&self) -> TerminalId {
        self.registry.terminal_id()
    }

    /// The requests this viewer may make.
    #[must_use]
    pub fn capabilities(&self) -> ViewerCapabilities {
        self.slot.capabilities
    }

    /// Returns the immutable engine version used by this terminal.
    #[must_use]
    pub fn engine_revision(&self) -> &'static str {
        crate::GHOSTTY_REVISION
    }

    /// The host-effect recipient, when the viewer receives host effects.
    #[must_use]
    pub fn host_effects(&self) -> Option<&HostEffectRecipient> {
        self.host_effects.as_ref()
    }

    /// Facts to attach to input the user produces now. Pass the geometry
    /// revision of the snapshot that mouse coordinates were computed
    /// against, if any.
    #[must_use]
    pub fn stamp(&self, geometry: Option<u64>) -> InputStamp {
        InputStamp {
            scrolls: self.scrolls.load(Ordering::Acquire),
            geometry,
        }
    }

    fn check(&self, allowed: bool) -> Result<(), RuntimeError> {
        if self.slot.is_revoked() {
            Err(RuntimeError::Revoked)
        } else if !allowed {
            Err(RuntimeError::NotPermitted)
        } else {
            Ok(())
        }
    }

    /// Sends structured input stamped now.
    ///
    /// # Errors
    ///
    /// Returns an error when the viewer may not send input, was revoked, or
    /// the terminal has stopped or cannot accept it yet.
    pub fn send_input(&self, input: TerminalInput) -> Result<(), RuntimeError> {
        self.offer_input(input, self.stamp(None))
            .map_err(|refused| refused.error)
    }

    /// Sends input with the stamp recorded when the user produced it,
    /// returning refused input so the caller can retry it without a copy.
    ///
    /// # Errors
    ///
    /// Returns the input with [`RuntimeError::Busy`] when the runtime
    /// cannot accept it yet, or with the permanent error otherwise.
    pub fn offer_input(
        &self,
        input: TerminalInput,
        stamp: InputStamp,
    ) -> Result<(), RefusedInput> {
        if let Err(error) = self.check(self.slot.capabilities.input) {
            return Err(RefusedInput { error, input });
        }
        self.slot.queue();
        self.client
            .offer_input(Arc::clone(&self.slot), input, stamp)
            .inspect_err(|_| {
                self.slot.settled();
            })
    }

    /// Reports whether this viewer's view holds focus in an active window.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Busy`] when the runtime cannot accept it yet;
    /// retry it in order with the viewer's input.
    pub fn set_focus(&self, focused: bool) -> Result<(), RuntimeError> {
        let capabilities = self.slot.capabilities;
        self.check(capabilities.input || capabilities.size)?;
        self.send_counted(RuntimeMessage::Arbitration {
            slot: Arc::clone(&self.slot),
            report: Report::Focus(focused),
        })
    }

    /// Reports the grid and cell size this viewer would display.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Busy`] when the runtime cannot accept it yet.
    pub fn report_geometry(
        &self,
        grid: GridSize,
        cell: CellSize,
    ) -> Result<(), RuntimeError> {
        self.check(self.slot.capabilities.size)?;
        self.send_counted(RuntimeMessage::Arbitration {
            slot: Arc::clone(&self.slot),
            report: Report::Geometry(
                GridSize::clamped(grid.columns, grid.rows),
                cell,
            ),
        })
    }

    /// Submits the presentation this viewer would apply. It takes effect
    /// while this viewer controls the terminal.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Busy`] when the runtime cannot accept it yet.
    pub fn update_presentation(
        &self,
        presentation: TerminalPresentation,
    ) -> Result<(), RuntimeError> {
        self.check(true)?;
        self.send_counted(RuntimeMessage::Presentation {
            slot: Arc::clone(&self.slot),
            presentation: Box::new(presentation),
        })
    }

    /// Erases the scrollback and keeps the screen, after earlier input.
    ///
    /// # Errors
    ///
    /// Returns an error when the viewer may not send input, or the runtime
    /// stopped or cannot accept it yet.
    pub fn clear_history(&self) -> Result<(), RuntimeError> {
        self.send_edit(BufferEdit::ClearHistory)
    }

    /// Resets the emulator as RIS does, after earlier input. The PTY and
    /// its processes are unaffected.
    ///
    /// # Errors
    ///
    /// Returns an error when the viewer may not send input, or the runtime
    /// stopped or cannot accept it yet.
    pub fn reset(&self) -> Result<(), RuntimeError> {
        self.send_edit(BufferEdit::Reset)
    }

    fn send_edit(&self, edit: BufferEdit) -> Result<(), RuntimeError> {
        self.check(self.slot.capabilities.input)?;
        self.send_counted(RuntimeMessage::Edit {
            slot: Arc::clone(&self.slot),
            edit,
        })
    }

    /// Sends an ordered message counted toward this viewer's queue, so a
    /// dropped viewer is finalized only after the message runs.
    fn send_counted(
        &self,
        message: RuntimeMessage,
    ) -> Result<(), RuntimeError> {
        self.slot.queue();
        self.client.send_message(message).inspect_err(|_| {
            self.slot.settled();
        })
    }

    /// Requests a snapshot of the shared viewport without waiting.
    ///
    /// # Errors
    ///
    /// Returns an error when the viewer was revoked or the terminal stopped.
    pub fn request_snapshot(&self) -> Result<SnapshotRequest, RuntimeError> {
        self.request_snapshot_with_link(None, None)
    }

    /// Moves the shared viewport and reads it atomically on the runtime
    /// owner.
    ///
    /// # Errors
    ///
    /// Returns an error when the viewer may not scroll, was revoked, or the
    /// terminal stopped.
    pub fn request_scrolled_snapshot(
        &self,
        scroll: ScrollCommand,
    ) -> Result<SnapshotRequest, RuntimeError> {
        self.request_snapshot_with_link(Some(scroll), None)
    }

    /// Requests a snapshot, an optional scroll, and an optional link lookup
    /// in one owner-thread operation.
    ///
    /// # Errors
    ///
    /// Returns an error when the viewer may not scroll, was revoked, or the
    /// terminal stopped. Lookup failures remain nonfatal outcomes in the
    /// successful reply.
    pub fn request_snapshot_with_link(
        &self,
        scroll: Option<ScrollCommand>,
        point: Option<MousePosition>,
    ) -> Result<SnapshotRequest, RuntimeError> {
        self.check(scroll.is_none() || self.slot.capabilities.viewport)?;
        let scroll = scroll.map(|command| {
            (command, self.scrolls.fetch_add(1, Ordering::AcqRel) + 1)
        });
        let (reply, receiver) = async_channel::bounded(1);
        self.client.send_control(RuntimeControl::Snapshot {
            slot: Arc::clone(&self.slot),
            scroll,
            point,
            reply,
            #[cfg(test)]
            fail_lookup: false,
        })?;
        Ok(SnapshotRequest::new(receiver))
    }

    /// Requests text extraction from canonical scrollback without blocking.
    ///
    /// # Errors
    ///
    /// Returns an error when the viewer was revoked or the terminal stopped.
    pub fn request_selection(
        &self,
        generation: u64,
        range: BufferRange,
    ) -> Result<SelectionRequest, RuntimeError> {
        self.check(true)?;
        let (reply, receiver) = async_channel::bounded(1);
        self.client.send_control(RuntimeControl::Selection {
            slot: Arc::clone(&self.slot),
            generation,
            range,
            reply,
        })?;
        Ok(SelectionRequest::new(receiver))
    }

    /// Reads a snapshot of the shared viewport, blocking briefly.
    ///
    /// Intended for worker threads and tests; interactive clients should
    /// await [`Self::request_snapshot`].
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime is unavailable or does not answer.
    pub fn read_snapshot(&self) -> Result<Arc<TerminalSnapshot>, RuntimeError> {
        self.request_snapshot()?
            .recv_blocking()
            .map(|reply| reply.snapshot)
    }

    /// A handle on this viewer's wake for the task that drains it.
    #[must_use]
    pub fn wake_handle(&self) -> ViewerWake {
        ViewerWake {
            wake: self.wake.clone(),
        }
    }

    /// Completes once the terminal published something this viewer has not
    /// polled, including a host effect or revocation. The signal carries no
    /// data and coalesces; call [`Self::poll`] after it.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Stopped`] once the terminal stopped and the
    /// final status has been signalled, or once a revoked viewer was
    /// finalized after its revocation was signalled.
    pub async fn wait(&self) -> Result<(), RuntimeError> {
        self.wake.recv().await.map_err(|_| RuntimeError::Stopped)
    }

    #[cfg(test)]
    pub(crate) fn slot(&self) -> Arc<Slot> {
        Arc::clone(&self.slot)
    }

    /// Consumes a pending wake without waiting.
    #[cfg(test)]
    pub(crate) fn wake_pending(&self) -> bool {
        self.wake.try_recv().is_ok()
    }

    /// Reads what changed since the previous poll.
    pub fn poll(&self) -> ViewerUpdate {
        let mut update = ViewerUpdate {
            revoked: self.slot.is_revoked(),
            ..ViewerUpdate::default()
        };
        if self.slot.notified.load(Ordering::Acquire) {
            update.invalidated = Some(self.registry.generation());
        }
        // The baseline lock covers reading the status, so concurrent polls
        // cannot commit an older revision over a newer one.
        let mut baseline =
            self.baseline.lock().unwrap_or_else(PoisonError::into_inner);
        let status = self.registry.status();
        if baseline.revision == Some(status.revision) {
            return update;
        }
        baseline.revision = Some(status.revision);
        if let TerminalLifecycle::Exited(exit) = status.lifecycle
            && baseline.running
        {
            baseline.running = false;
            update.exited = Some(exit);
        }
        update.bells = status.bells.saturating_sub(baseline.bells);
        baseline.bells = status.bells;
        let (failures, missed) = status.failures_since(baseline.failure);
        update.failures = failures.to_vec();
        update.missed_failures = missed;
        baseline.failure = status.last_failure();
        drop(baseline);
        update.status = Some(status);
        update
    }
}

impl Drop for TerminalViewer {
    fn drop(&mut self) {
        self.slot.dropped.store(true, Ordering::Release);
        self.registry.mark_changed();
    }
}
