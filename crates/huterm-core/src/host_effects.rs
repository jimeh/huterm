use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError, Weak};

use huterm_protocol::{
    AttachmentId, ClipboardDestination, ClipboardWrite, HostEffect,
    HostEffectKind, HostEffectMetadata, HostEffectOrigin,
    HostEffectRecipientId, TerminalId, TerminalNotification,
};

/// Effects one terminal may have queued.
pub(crate) const TERMINAL_EFFECT_LIMIT: usize = 8;
/// Text bytes one terminal may have queued: 64 MiB, the minimum write size
/// the Kitty clipboard protocol requires. The engine also caps Kitty
/// clipboard writes at this size, so larger ones fail before admission.
pub(crate) const TERMINAL_BYTE_LIMIT: usize = 64 * 1024 * 1024;
const PROCESS_EFFECT_LIMIT: usize = 32;
/// Room for two terminals to each hold one maximum Kitty write.
const PROCESS_BYTE_LIMIT: usize = 2 * TERMINAL_BYTE_LIMIT;
const TERMINAL_RECIPIENT_LIMIT: usize = 32;
/// The fewest effects one client may queue on a terminal while several
/// clients have recipients there.
const MIN_CLIENT_SHARE: usize = 2;

/// The effects one client's recipients may hold on a terminal while
/// `clients` clients have recipients there: every effect for a lone
/// client, so it keeps the whole queue for bursts, and otherwise an equal
/// share of at least [`MIN_CLIENT_SHARE`].
fn client_share(clients: usize) -> usize {
    if clients <= 1 {
        TERMINAL_EFFECT_LIMIT
    } else {
        (TERMINAL_EFFECT_LIMIT / clients).max(MIN_CLIENT_SHARE)
    }
}

/// Connection boundary represented by a host-effect recipient.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostEffectClientOrigin {
    /// A local desktop: a view embedded in the desktop process, or a
    /// desktop client on local IPC.
    LocalDesktop,
    /// A local text client without desktop clipboard authority.
    LocalText,
    /// A client connected across a process or network boundary.
    Remote,
}

/// The host-effect kinds a client currently permits terminal content to
/// use, from its policy settings.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostEffectPermissions {
    /// Terminal-content clipboard writes (`terminal.clipboard_write`).
    pub clipboard: bool,
    /// Terminal-content notifications (`terminal.notifications`).
    pub notifications: bool,
}

impl HostEffectPermissions {
    /// Permits every kind.
    pub const ALL: Self = Self {
        clipboard: true,
        notifications: true,
    };

    /// Whether `kind` is permitted.
    #[must_use]
    pub const fn allows(self, kind: HostEffectKind) -> bool {
        match kind {
            HostEffectKind::Clipboard => self.clipboard,
            HostEffectKind::Notification => self.notifications,
        }
    }
}

/// Capability and policy state used when registering a host-effect recipient.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostEffectRecipientOptions {
    /// Connection boundary of this client.
    pub origin: HostEffectClientOrigin,
    /// Whether this client is allowed to perform state-changing host effects.
    pub writable: bool,
    /// The kinds terminal content may currently use through this client.
    pub permissions: HostEffectPermissions,
}

impl HostEffectRecipientOptions {
    /// Creates options for a writable local desktop view.
    #[must_use]
    pub const fn local_desktop(permissions: HostEffectPermissions) -> Self {
        Self {
            origin: HostEffectClientOrigin::LocalDesktop,
            writable: true,
            permissions,
        }
    }
}

/// One client's host-effect budget, shared by every window and terminal
/// registration of that client: the desktop process in embedded mode, or
/// one IPC connection. Each terminal also counts the client's effects
/// against its share of the terminal's queue.
#[derive(Clone, Debug)]
pub struct DesktopHostEffectClient {
    process_budget: Arc<Budget>,
}

impl DesktopHostEffectClient {
    /// Creates an independent desktop-process host-effect budget.
    #[must_use]
    pub fn new() -> Self {
        Self {
            process_budget: Arc::new(Budget::new(
                PROCESS_EFFECT_LIMIT,
                PROCESS_BYTE_LIMIT,
            )),
        }
    }
}

impl Default for DesktopHostEffectClient {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct Budget {
    effect_limit: usize,
    byte_limit: usize,
    state: Mutex<BudgetState>,
}

#[derive(Default, Debug)]
struct BudgetState {
    effects: usize,
    bytes: usize,
}

impl Budget {
    fn new(effect_limit: usize, byte_limit: usize) -> Self {
        Self {
            effect_limit,
            byte_limit,
            state: Mutex::new(BudgetState::default()),
        }
    }

    fn can_reserve(&self, state: &BudgetState, bytes: usize) -> bool {
        state.effects < self.effect_limit
            && state
                .bytes
                .checked_add(bytes)
                .is_some_and(|total| total <= self.byte_limit)
    }
}

#[derive(Debug)]
struct EffectReservation {
    terminal: Arc<Budget>,
    process: Arc<Budget>,
    /// The client's share of the terminal's queue.
    share: Arc<Budget>,
    bytes: usize,
}

impl Drop for EffectReservation {
    fn drop(&mut self) {
        for budget in [&self.terminal, &self.process, &self.share] {
            let mut state = budget
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.effects -= 1;
            state.bytes -= self.bytes;
        }
    }
}

/// One admitted effect whose capacity remains reserved until it is consumed or dropped.
#[derive(Debug)]
pub struct PendingHostEffect {
    metadata: HostEffectMetadata,
    effect: HostEffect,
    registration: Weak<Registration>,
    _reservation: EffectReservation,
}

impl PendingHostEffect {
    /// Returns recipient and terminal ordering metadata.
    #[must_use]
    pub fn metadata(&self) -> HostEffectMetadata {
        self.metadata
    }

    /// Returns the effect payload without copying it.
    #[must_use]
    pub fn effect(&self) -> &HostEffect {
        &self.effect
    }

    /// Whether the delivery still belongs to its recipient's live
    /// generation for its kind, as [`HostEffectRecipient::is_current`]
    /// checks, for a holder without the recipient.
    #[must_use]
    pub fn is_current(&self) -> bool {
        let kind = self.effect.kind();
        self.registration.upgrade().is_some_and(|registration| {
            registration.is_eligible(kind)
                && registration.generation(kind)
                    == self.metadata.recipient_generation
        })
    }
}

/// How a recipient's viewer wakes and how recently it was used.
#[derive(Clone, Debug, Default)]
pub(crate) struct RecipientLink {
    /// Signalled when an effect is queued for this recipient.
    pub(crate) activity: Option<async_channel::Sender<()>>,
    /// The viewer's activity ordinal; the most recently used recipient wins.
    pub(crate) ordinal: Arc<AtomicU64>,
}

/// One kind's permission and the generation that numbers its changes.
#[derive(Debug)]
struct KindState {
    allowed: AtomicBool,
    generation: AtomicU64,
}

#[derive(Debug)]
struct Registration {
    id: HostEffectRecipientId,
    active: AtomicBool,
    attachment: Option<AttachmentId>,
    options: HostEffectRecipientOptions,
    /// Indexed by [`HostEffectKind::index`].
    kinds: [KindState; HostEffectKind::ALL.len()],
    link: RecipientLink,
    order: u64,
    process_budget: Arc<Budget>,
    /// The client's share of this terminal's queue, which every
    /// registration of the client on the terminal counts against.
    share: Arc<Budget>,
    queue: Mutex<VecDeque<PendingHostEffect>>,
}

impl Registration {
    fn kind(&self, kind: HostEffectKind) -> &KindState {
        &self.kinds[kind.index()]
    }

    fn is_eligible(&self, kind: HostEffectKind) -> bool {
        self.options.origin == HostEffectClientOrigin::LocalDesktop
            && self.options.writable
            && self.active.load(Ordering::Acquire)
            && self.kind(kind).allowed.load(Ordering::Acquire)
    }

    fn generation(&self, kind: HostEffectKind) -> u64 {
        self.kind(kind).generation.load(Ordering::Acquire)
    }

    fn advance_generation(&self, kind: HostEffectKind) {
        if self
            .kind(kind)
            .generation
            .try_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .is_err()
        {
            self.active.store(false, Ordering::Release);
        }
    }

    /// Drops the queued effects `matches` selects after releasing the
    /// queue, so freeing their reservations never waits for a budget while
    /// holding it.
    fn clear_queue(&self, matches: impl Fn(HostEffectKind) -> bool) {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (cleared, kept) = std::mem::take(&mut *queue)
            .into_iter()
            .partition::<VecDeque<_>, _>(|pending| {
                matches(pending.effect.kind())
            });
        *queue = kept;
        drop(queue);
        drop(cleared);
    }

    fn revoke_permission(&self, kind: HostEffectKind) {
        self.begin_permission_revocation(kind);
        self.clear_queue(|queued| queued == kind);
    }

    fn begin_permission_revocation(&self, kind: HostEffectKind) {
        self.kind(kind).allowed.store(false, Ordering::Release);
        self.advance_generation(kind);
    }

    fn revoke_permanently(&self) {
        self.active.store(false, Ordering::Release);
        for kind in HostEffectKind::ALL {
            self.advance_generation(kind);
        }
        self.clear_queue(|_| true);
    }
}

/// Non-cloneable UI-side registration for receiving terminal host effects.
#[derive(Debug)]
pub struct HostEffectRecipient {
    registration: Arc<Registration>,
}

impl HostEffectRecipient {
    /// Updates whether terminal content may use `kind` through this
    /// recipient. Removing permission cancels that kind's queued and
    /// already-popped deliveries, and leaves other kinds alone.
    pub fn set_allowed(&self, kind: HostEffectKind, allowed: bool) {
        if allowed {
            if self.registration.active.load(Ordering::Acquire) {
                self.registration
                    .kind(kind)
                    .allowed
                    .store(true, Ordering::Release);
            }
        } else {
            self.registration.revoke_permission(kind);
        }
    }

    /// Sets whether terminal content may use `kind`, under a `generation`
    /// the caller numbers, as a client across IPC numbers its permission
    /// changes. Every effect admitted from now on carries `generation`,
    /// and deliveries of `kind` admitted under an earlier one are no
    /// longer current; removing permission also cancels them. Returns
    /// false, changing nothing, for a generation not above the current
    /// one.
    #[must_use = "a refused generation changed nothing"]
    pub fn set_permission(
        &self,
        kind: HostEffectKind,
        allowed: bool,
        generation: u64,
    ) -> bool {
        let state = self.registration.kind(kind);
        if generation <= state.generation.load(Ordering::Acquire) {
            return false;
        }
        if allowed {
            state.generation.store(generation, Ordering::Release);
            if self.registration.active.load(Ordering::Acquire) {
                state.allowed.store(true, Ordering::Release);
            }
            // Deliveries under the earlier generation are no longer
            // current; they leave as `try_next` meets them.
        } else {
            // Permission goes first, so admission racing the change sees
            // it denied or sees the new generation, as in revocation.
            state.allowed.store(false, Ordering::Release);
            state.generation.store(generation, Ordering::Release);
            self.registration.clear_queue(|queued| queued == kind);
        }
        true
    }

    /// Withdraws the recipient for good, as dropping it does, while its
    /// owner still holds it: every queued and popped delivery stops being
    /// current, and no effect selects it again.
    pub fn revoke(&self) {
        self.registration.revoke_permanently();
    }

    /// Removes and returns the next currently valid delivery without blocking.
    #[must_use]
    pub fn try_next(&self) -> Option<PendingHostEffect> {
        // Stale deliveries are dropped after the queue is released, as in
        // `clear_queue`.
        let mut stale = Vec::new();
        let mut queue = self.registration.queue.try_lock().ok()?;
        let next = loop {
            match queue.pop_front() {
                Some(pending) if self.is_current(&pending) => {
                    break Some(pending);
                }
                Some(pending) => stale.push(pending),
                None => break None,
            }
        };
        drop(queue);
        drop(stale);
        next
    }

    /// Rechecks that a popped delivery still belongs to this live generation.
    /// Call immediately before invoking the platform host operation. An operation
    /// that has already started cannot be undone by later revocation.
    #[must_use]
    pub fn is_current(&self, pending: &PendingHostEffect) -> bool {
        let kind = pending.effect.kind();
        pending.registration.upgrade().is_some_and(|registration| {
            Arc::ptr_eq(&registration, &self.registration)
                && registration.is_eligible(kind)
                && registration.generation(kind)
                    == pending.metadata.recipient_generation
        })
    }
}

impl Drop for HostEffectRecipient {
    fn drop(&mut self) {
        self.registration.revoke_permanently();
    }
}

#[derive(Debug)]
struct SinkState {
    registrations: Vec<Weak<Registration>>,
    next_registration: u64,
    /// Each client's share of the terminal's queue, by its budget, kept
    /// while the client lives so effects it still holds keep counting
    /// when its recipients come and go.
    shares: Vec<(Weak<Budget>, Arc<Budget>)>,
}

#[derive(Debug)]
struct SinkInner {
    terminal_id: TerminalId,
    closed: AtomicBool,
    terminal_budget: Arc<Budget>,
    next_sequence: AtomicU64,
    state: Mutex<SinkState>,
    /// How often admission waited for a lock another thread held.
    #[cfg(test)]
    waits: AtomicU64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostEffectAdmission {
    Accepted,
    NoRecipient,
    Denied,
    Full,
    Closed,
}

/// Terminal-side admission point for client-mediated host effects.
///
/// Admission runs on the terminal's runtime thread and never waits for a
/// client to act. When another thread holds one of its locks, admission
/// waits instead of dropping the effect: a lost write is visible to the
/// user, and every holder keeps these locks briefly. Admission releases
/// the sink state before it takes the terminal budget, then the process
/// budget, then the client's share, then the queue. Other threads may take the queue or a budget
/// while holding the sink state, but never take another of these locks
/// while holding the queue or a budget, so the waits cannot form a cycle.
/// That is why reservations are freed after the queue is released.
#[derive(Clone, Debug)]
pub(crate) struct HostEffectSink {
    inner: Arc<SinkInner>,
}

impl HostEffectSink {
    pub(crate) fn new(terminal_id: TerminalId) -> Self {
        Self {
            inner: Arc::new(SinkInner {
                terminal_id,
                closed: AtomicBool::new(false),
                terminal_budget: Arc::new(Budget::new(
                    TERMINAL_EFFECT_LIMIT,
                    TERMINAL_BYTE_LIMIT,
                )),
                next_sequence: AtomicU64::new(0),
                state: Mutex::new(SinkState {
                    registrations: Vec::new(),
                    next_registration: 0,
                    shares: Vec::new(),
                }),
                #[cfg(test)]
                waits: AtomicU64::new(0),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn admit_owned(&self, text: String) -> HostEffectAdmission {
        let bytes = text.capacity();
        self.admit(HostEffectKind::Clipboard, bytes, || clipboard(text))
    }

    pub(crate) fn admit_borrowed(&self, text: &str) -> HostEffectAdmission {
        self.admit(HostEffectKind::Clipboard, text.len(), || {
            clipboard(text.to_owned())
        })
    }

    pub(crate) fn admit_notification(
        &self,
        notification: TerminalNotification,
    ) -> HostEffectAdmission {
        let bytes = notification.allocation_capacity();
        self.admit(HostEffectKind::Notification, bytes, || {
            HostEffect::Notification(notification)
        })
    }

    fn admit(
        &self,
        kind: HostEffectKind,
        bytes: usize,
        make_effect: impl FnOnce() -> HostEffect,
    ) -> HostEffectAdmission {
        if self.inner.closed.load(Ordering::Acquire) {
            return HostEffectAdmission::Closed;
        }
        let (registration, selected_generation, clients) =
            match self.select_recipient(kind) {
                Ok(selected) => selected,
                Err(status) => return status,
            };

        let mut terminal_budget = self.lock(&self.inner.terminal_budget.state);
        let mut process_budget = self.lock(&registration.process_budget.state);
        let mut share = self.lock(&registration.share.state);
        if !self
            .inner
            .terminal_budget
            .can_reserve(&terminal_budget, bytes)
            || !registration
                .process_budget
                .can_reserve(&process_budget, bytes)
            || share.effects >= client_share(clients)
        {
            return HostEffectAdmission::Full;
        }
        let mut queue = self.lock(&registration.queue);
        if !registration.is_eligible(kind)
            || registration.generation(kind) != selected_generation
        {
            return HostEffectAdmission::Denied;
        }

        let effect = make_effect();
        let actual_bytes = effect.allocation_capacity();
        if actual_bytes != bytes
            && (!self
                .inner
                .terminal_budget
                .can_reserve(&terminal_budget, actual_bytes)
                || !registration
                    .process_budget
                    .can_reserve(&process_budget, actual_bytes))
        {
            return HostEffectAdmission::Full;
        }
        let Ok(sequence) = self.inner.next_sequence.try_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |sequence| sequence.checked_add(1),
        ) else {
            return HostEffectAdmission::Closed;
        };
        terminal_budget.effects += 1;
        terminal_budget.bytes += actual_bytes;
        process_budget.effects += 1;
        process_budget.bytes += actual_bytes;
        share.effects += 1;
        share.bytes += actual_bytes;
        queue.push_back(PendingHostEffect {
            metadata: HostEffectMetadata {
                origin: HostEffectOrigin {
                    terminal_id: self.inner.terminal_id,
                    sequence,
                },
                recipient: registration.id,
                recipient_generation: selected_generation,
            },
            effect,
            registration: Arc::downgrade(&registration),
            _reservation: EffectReservation {
                terminal: Arc::clone(&self.inner.terminal_budget),
                process: Arc::clone(&registration.process_budget),
                share: Arc::clone(&registration.share),
                bytes: actual_bytes,
            },
        });
        drop(queue);
        if let Some(signal) = &registration.link.activity {
            let _ = signal.try_send(());
        }
        HostEffectAdmission::Accepted
    }

    /// Selects the recipient for an effect of `kind`, with its generation
    /// and the number of clients whose recipients share the terminal.
    fn select_recipient(
        &self,
        kind: HostEffectKind,
    ) -> Result<(Arc<Registration>, u64, usize), HostEffectAdmission> {
        let mut state = self.lock(&self.inner.state);
        state.registrations.retain(|weak| weak.strong_count() != 0);
        if state.registrations.is_empty() {
            return Err(HostEffectAdmission::NoRecipient);
        }
        let eligible: Vec<Arc<Registration>> = state
            .registrations
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|registration| registration.is_eligible(kind))
            .collect();
        drop(state);
        // Clients that could receive this effect share the terminal's
        // queue; one whose recipients cannot, such as a text client, takes
        // no share.
        let mut clients: Vec<&Arc<Budget>> = Vec::new();
        for registration in &eligible {
            if !clients
                .iter()
                .any(|client| Arc::ptr_eq(client, &registration.process_budget))
            {
                clients.push(&registration.process_budget);
            }
        }
        let clients = clients.len();
        let Some(registration) = eligible.iter().max_by(|left, right| {
            left.link
                .ordinal
                .load(Ordering::Acquire)
                .cmp(&right.link.ordinal.load(Ordering::Acquire))
                .then_with(|| left.order.cmp(&right.order))
        }) else {
            return Err(HostEffectAdmission::Denied);
        };
        let selected_generation = registration.generation(kind);
        Ok((Arc::clone(registration), selected_generation, clients))
    }

    /// Locks one of admission's mutexes, waiting while another thread
    /// holds it.
    #[cfg_attr(
        not(test),
        expect(clippy::unused_self, reason = "only tests count the waits")
    )]
    fn lock<'a, T>(&self, mutex: &'a Mutex<T>) -> MutexGuard<'a, T> {
        match mutex.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                #[cfg(test)]
                self.inner.waits.fetch_add(1, Ordering::AcqRel);
                mutex.lock().unwrap_or_else(PoisonError::into_inner)
            }
        }
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Acquire)
    }

    pub(crate) fn close(&self) {
        self.inner.closed.store(true, Ordering::Release);
        self.invalidate_all();
    }

    pub(crate) fn invalidate_attachment(&self, attachment: AttachmentId) {
        self.invalidate_where(|registration| {
            registration.attachment == Some(attachment)
        });
    }

    pub(crate) fn invalidate_all(&self) {
        self.invalidate_where(|_| true);
    }

    fn invalidate_where(&self, matches: impl Fn(&Registration) -> bool) {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for registration in state.registrations.iter().filter_map(Weak::upgrade)
        {
            if matches(&registration) {
                registration.revoke_permanently();
            }
        }
    }

    pub(crate) fn register(
        &self,
        attachment: Option<AttachmentId>,
        process: &DesktopHostEffectClient,
        options: HostEffectRecipientOptions,
        link: RecipientLink,
    ) -> Option<HostEffectRecipient> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A permanently revoked registration can outlive its viewer's
        // finalization while the viewer's handle is still held; it no longer
        // counts toward the limit, as its finalized viewer slot does not.
        state.registrations.retain(|weak| {
            weak.upgrade().is_some_and(|registration| {
                registration.active.load(Ordering::Acquire)
            })
        });
        if self.inner.closed.load(Ordering::Acquire)
            || state.registrations.len() >= TERMINAL_RECIPIENT_LIMIT
        {
            return None;
        }
        state.next_registration = state.next_registration.checked_add(1)?;
        let registration_order = state.next_registration;
        state
            .shares
            .retain(|(client, _)| client.strong_count() != 0);
        // Dead clients left above, so a pointer names one live client.
        let client = Arc::as_ptr(&process.process_budget);
        let existing = state
            .shares
            .iter()
            .find(|(known, _)| std::ptr::eq(known.as_ptr(), client))
            .map(|(_, share)| Arc::clone(share));
        let share = existing.unwrap_or_else(|| {
            let share =
                Arc::new(Budget::new(TERMINAL_EFFECT_LIMIT, usize::MAX));
            state.shares.push((
                Arc::downgrade(&process.process_budget),
                Arc::clone(&share),
            ));
            share
        });
        let registration = Arc::new(Registration {
            id: HostEffectRecipientId::new(registration_order),
            active: AtomicBool::new(true),
            attachment,
            options,
            kinds: HostEffectKind::ALL.map(|kind| KindState {
                allowed: AtomicBool::new(options.permissions.allows(kind)),
                generation: AtomicU64::new(1),
            }),
            link,
            order: registration_order,
            process_budget: Arc::clone(&process.process_budget),
            share,
            queue: Mutex::new(VecDeque::new()),
        });
        state.registrations.push(Arc::downgrade(&registration));
        Some(HostEffectRecipient { registration })
    }
}

fn clipboard(text: String) -> HostEffect {
    HostEffect::ClipboardWrite(ClipboardWrite::new(
        ClipboardDestination::System,
        text,
    ))
}

#[cfg(test)]
pub(crate) fn test_fixture(
    terminal_id: TerminalId,
) -> (HostEffectSink, HostEffectRecipient) {
    test_fixture_with_clipboard(terminal_id, true)
}

/// A sink with one local desktop recipient whose `terminal.clipboard_write`
/// policy is `allow` when `clipboard_allowed` is set and `deny` otherwise.
#[cfg(test)]
pub(crate) fn test_fixture_with_clipboard(
    terminal_id: TerminalId,
    clipboard_allowed: bool,
) -> (HostEffectSink, HostEffectRecipient) {
    let sink = HostEffectSink::new(terminal_id);
    let process = DesktopHostEffectClient::new();
    let recipient = sink
        .register(
            Some(AttachmentId::new(1)),
            &process,
            HostEffectRecipientOptions::local_desktop(HostEffectPermissions {
                clipboard: clipboard_allowed,
                notifications: true,
            }),
            RecipientLink::default(),
        )
        .expect("fresh test fixture should accept its first recipient");
    (sink, recipient)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipient(
        sink: &HostEffectSink,
        process: &DesktopHostEffectClient,
        attachment: u64,
        options: HostEffectRecipientOptions,
    ) -> HostEffectRecipient {
        sink.register(
            Some(AttachmentId::new(attachment)),
            process,
            options,
            RecipientLink::default(),
        )
        .expect("test registration should fit")
    }

    fn text(pending: &PendingHostEffect) -> &str {
        match pending.effect() {
            HostEffect::ClipboardWrite(write) => write.text(),
            effect => panic!("unexpected host effect: {effect:?}"),
        }
    }

    #[test]
    fn admitted_effect_wakes_only_the_selected_recipient() {
        let sink = HostEffectSink::new(TerminalId::new(1));
        let process = DesktopHostEffectClient::new();
        let link = |ordinal| {
            let (activity, signal) = async_channel::bounded(1);
            (
                RecipientLink {
                    activity: Some(activity),
                    ordinal: Arc::new(AtomicU64::new(ordinal)),
                },
                signal,
            )
        };
        let (active_link, active_signal) = link(2);
        let (idle_link, idle_signal) = link(1);
        let options = HostEffectRecipientOptions::local_desktop(
            HostEffectPermissions::ALL,
        );
        let active = sink
            .register(
                Some(AttachmentId::new(1)),
                &process,
                options,
                active_link,
            )
            .unwrap();
        let _idle = sink
            .register(Some(AttachmentId::new(2)), &process, options, idle_link)
            .unwrap();

        assert_eq!(sink.admit_borrowed("copy"), HostEffectAdmission::Accepted);
        active_signal.try_recv().unwrap();
        assert!(idle_signal.try_recv().is_err());
        assert_eq!(text(&active.try_next().unwrap()), "copy");
    }

    #[test]
    fn permanently_revoked_registrations_free_their_place_in_the_limit() {
        let sink = HostEffectSink::new(TerminalId::new(1));
        let process = DesktopHostEffectClient::new();
        let options = HostEffectRecipientOptions::local_desktop(
            HostEffectPermissions::ALL,
        );
        let register = |attachment| {
            sink.register(
                Some(AttachmentId::new(attachment)),
                &process,
                options,
                RecipientLink::default(),
            )
        };
        let limit = u64::try_from(TERMINAL_RECIPIENT_LIMIT).unwrap();
        // Revoked viewers can still hold their recipients.
        let held: Vec<_> = (1..=limit)
            .map(|attachment| register(attachment).unwrap())
            .collect();
        assert!(register(limit + 1).is_none());
        sink.invalidate_all();
        assert!(
            register(limit + 1).is_some(),
            "revoked registrations no longer count"
        );
        drop(held);
    }

    #[test]
    fn desktop_clipboard_registrations_do_not_limit_terminal_count() {
        let process = DesktopHostEffectClient::new();
        let mut terminals = Vec::new();
        for id in 1..=300 {
            let sink = HostEffectSink::new(TerminalId::new(id));
            let recipient = recipient(
                &sink,
                &process,
                id,
                HostEffectRecipientOptions::local_desktop(
                    HostEffectPermissions::ALL,
                ),
            );
            terminals.push((sink, recipient));
        }
        let (sink, recipient) = terminals.last().unwrap();
        assert_eq!(
            sink.admit_borrowed("last tab"),
            HostEffectAdmission::Accepted
        );
        assert_eq!(text(&recipient.try_next().unwrap()), "last tab");
    }

    #[test]
    fn empty_clears_count_toward_terminal_effect_limit() {
        let sink = HostEffectSink::new(TerminalId::new(1));
        let process = DesktopHostEffectClient::new();
        let recipient = recipient(
            &sink,
            &process,
            1,
            HostEffectRecipientOptions::local_desktop(
                HostEffectPermissions::ALL,
            ),
        );

        for _ in 0..TERMINAL_EFFECT_LIMIT {
            assert_eq!(sink.admit_borrowed(""), HostEffectAdmission::Accepted);
        }
        assert_eq!(sink.admit_borrowed(""), HostEffectAdmission::Full);

        drop(recipient.try_next().expect("first clear should be queued"));
        assert_eq!(sink.admit_borrowed(""), HostEffectAdmission::Accepted);
    }

    #[test]
    fn process_budget_counts_owned_allocation_capacity_across_terminals() {
        let process = DesktopHostEffectClient::new();
        let mut recipients = Vec::new();
        let mut sinks = Vec::new();
        for id in 1..=3 {
            let sink = HostEffectSink::new(TerminalId::new(id));
            recipients.push(recipient(
                &sink,
                &process,
                id,
                HostEffectRecipientOptions::local_desktop(
                    HostEffectPermissions::ALL,
                ),
            ));
            sinks.push(sink);
        }

        for sink in &sinks[..2] {
            assert_eq!(
                sink.admit_owned(String::with_capacity(TERMINAL_BYTE_LIMIT)),
                HostEffectAdmission::Accepted
            );
        }
        assert_eq!(
            sinks[2].admit_owned(String::with_capacity(1)),
            HostEffectAdmission::Full
        );

        drop(recipients[0].try_next().expect("first effect should exist"));
        assert_eq!(
            sinks[2].admit_owned(String::with_capacity(1)),
            HostEffectAdmission::Accepted
        );
    }

    #[test]
    fn process_effect_count_is_shared_across_terminals() {
        let process = DesktopHostEffectClient::new();
        let mut recipients = Vec::new();
        let mut sinks = Vec::new();
        for id in 11..=15 {
            let sink = HostEffectSink::new(TerminalId::new(id));
            recipients.push(recipient(
                &sink,
                &process,
                id,
                HostEffectRecipientOptions::local_desktop(
                    HostEffectPermissions::ALL,
                ),
            ));
            sinks.push(sink);
        }

        for sink in &sinks[..4] {
            for _ in 0..TERMINAL_EFFECT_LIMIT {
                assert_eq!(
                    sink.admit_borrowed(""),
                    HostEffectAdmission::Accepted
                );
            }
        }
        assert_eq!(sinks[4].admit_borrowed(""), HostEffectAdmission::Full);

        drop(recipients[0].try_next().expect("one clear should exist"));
        assert_eq!(sinks[4].admit_borrowed(""), HostEffectAdmission::Accepted);
    }

    #[test]
    fn terminal_byte_limit_rejects_owned_allocation_capacity() {
        let (sink, recipient) = test_fixture(TerminalId::new(16));

        assert_eq!(
            sink.admit_owned(String::with_capacity(TERMINAL_BYTE_LIMIT + 1)),
            HostEffectAdmission::Full
        );
        assert!(recipient.try_next().is_none());
        assert_eq!(
            sink.admit_owned(String::with_capacity(TERMINAL_BYTE_LIMIT)),
            HostEffectAdmission::Accepted
        );
    }

    #[test]
    fn permission_revocation_invalidates_popped_delivery_and_allows_fresh_generation()
     {
        let (sink, recipient) = test_fixture(TerminalId::new(2));
        assert_eq!(
            sink.admit_borrowed("before"),
            HostEffectAdmission::Accepted
        );
        let stale = recipient.try_next().expect("effect should be queued");
        assert!(recipient.is_current(&stale));

        recipient.set_allowed(HostEffectKind::Clipboard, false);
        assert!(!recipient.is_current(&stale));
        assert_eq!(sink.admit_borrowed("denied"), HostEffectAdmission::Denied);

        recipient.set_allowed(HostEffectKind::Clipboard, true);
        assert_eq!(sink.admit_borrowed("after"), HostEffectAdmission::Accepted);
        let fresh =
            recipient.try_next().expect("fresh effect should be queued");
        assert!(recipient.is_current(&fresh));
        assert_ne!(
            stale.metadata().recipient_generation,
            fresh.metadata().recipient_generation
        );
    }

    #[test]
    fn activity_selects_one_recipient_without_fallback_or_replay() {
        let sink = HostEffectSink::new(TerminalId::new(3));
        let process = DesktopHostEffectClient::new();
        let options = HostEffectRecipientOptions::local_desktop(
            HostEffectPermissions::ALL,
        );
        let first_ordinal = Arc::new(AtomicU64::new(0));
        let first = sink
            .register(
                Some(AttachmentId::new(1)),
                &process,
                options,
                RecipientLink {
                    activity: None,
                    ordinal: Arc::clone(&first_ordinal),
                },
            )
            .unwrap();
        let second = recipient(&sink, &process, 2, options);

        assert_eq!(
            sink.admit_borrowed("second"),
            HostEffectAdmission::Accepted
        );
        assert_eq!(
            text(&second.try_next().expect("latest registration wins tie")),
            "second"
        );
        assert!(first.try_next().is_none());

        first_ordinal.store(1, Ordering::Release);
        assert_eq!(sink.admit_borrowed("first"), HostEffectAdmission::Accepted);
        drop(first);
        assert!(second.try_next().is_none(), "no redirect after drop");
        assert_eq!(sink.admit_borrowed("again"), HostEffectAdmission::Accepted);
        assert_eq!(
            text(&second.try_next().expect("future admission can use second")),
            "again"
        );
    }

    #[test]
    fn unsupported_or_read_only_recipients_are_ineligible() {
        let process = DesktopHostEffectClient::new();
        for (id, options) in [
            (
                4,
                HostEffectRecipientOptions {
                    origin: HostEffectClientOrigin::LocalText,
                    writable: true,
                    permissions: HostEffectPermissions::ALL,
                },
            ),
            (
                5,
                HostEffectRecipientOptions {
                    origin: HostEffectClientOrigin::Remote,
                    writable: true,
                    permissions: HostEffectPermissions::ALL,
                },
            ),
            (
                6,
                HostEffectRecipientOptions {
                    origin: HostEffectClientOrigin::LocalDesktop,
                    writable: false,
                    permissions: HostEffectPermissions::ALL,
                },
            ),
        ] {
            let sink = HostEffectSink::new(TerminalId::new(id));
            let _recipient = recipient(&sink, &process, id, options);
            assert_eq!(sink.admit_borrowed("no"), HostEffectAdmission::Denied);
            // Text and remote clients present no notifications either,
            // until their policies exist.
            assert_eq!(
                sink.admit_notification(note("no")),
                HostEffectAdmission::Denied
            );
        }
    }

    fn note(body: &str) -> TerminalNotification {
        TerminalNotification::new(String::new(), body.to_owned())
    }

    #[test]
    fn each_kind_has_its_own_permission_and_generation() {
        let (sink, recipient) = test_fixture(TerminalId::new(17));
        assert_eq!(sink.admit_borrowed("copy"), HostEffectAdmission::Accepted);
        assert_eq!(
            sink.admit_notification(note("first")),
            HostEffectAdmission::Accepted
        );
        let popped = recipient.try_next().expect("the copy is queued");
        assert!(matches!(popped.effect(), HostEffect::ClipboardWrite(_)));

        // Revoking notifications cancels only queued notifications.
        recipient.set_allowed(HostEffectKind::Notification, false);
        assert!(recipient.is_current(&popped));
        assert!(recipient.try_next().is_none(), "the notification is gone");
        assert_eq!(
            sink.admit_notification(note("denied")),
            HostEffectAdmission::Denied
        );
        assert_eq!(sink.admit_borrowed("copy"), HostEffectAdmission::Accepted);

        // Revoking the clipboard leaves a queued notification current.
        recipient.set_allowed(HostEffectKind::Notification, true);
        assert_eq!(
            sink.admit_notification(note("second")),
            HostEffectAdmission::Accepted
        );
        recipient.set_allowed(HostEffectKind::Clipboard, false);
        assert!(!recipient.is_current(&popped));
        let queued = recipient.try_next().expect("the notification stays");
        match queued.effect() {
            HostEffect::Notification(note) => assert_eq!(note.body(), "second"),
            effect => panic!("unexpected host effect: {effect:?}"),
        }
        assert!(recipient.is_current(&queued));
        assert!(recipient.try_next().is_none(), "the copy was cancelled");
    }

    #[test]
    fn notifications_select_only_recipients_that_permit_them() {
        let sink = HostEffectSink::new(TerminalId::new(18));
        let process = DesktopHostEffectClient::new();
        let newest_denies = HostEffectPermissions {
            clipboard: true,
            notifications: false,
        };
        let older = recipient(
            &sink,
            &process,
            1,
            HostEffectRecipientOptions::local_desktop(
                HostEffectPermissions::ALL,
            ),
        );
        let newer = recipient(
            &sink,
            &process,
            2,
            HostEffectRecipientOptions::local_desktop(newest_denies),
        );
        assert_eq!(
            sink.admit_notification(note("note")),
            HostEffectAdmission::Accepted
        );
        assert_eq!(sink.admit_borrowed("copy"), HostEffectAdmission::Accepted);
        assert!(matches!(
            older.try_next().unwrap().effect(),
            HostEffect::Notification(_)
        ));
        assert!(older.try_next().is_none(), "the copy went to the newer one");
        assert_eq!(text(&newer.try_next().unwrap()), "copy");
    }

    #[test]
    fn admission_waits_for_a_held_queue_and_delivers_in_order() {
        let (sink, recipient) = test_fixture(TerminalId::new(7));
        assert_eq!(sink.admit_borrowed("held"), HostEffectAdmission::Accepted);
        // The view holds its queue, as `try_next` does on the UI thread.
        let queue = recipient
            .registration
            .queue
            .lock()
            .expect("test queue should not be poisoned");
        std::thread::scope(|scope| {
            let admission = scope.spawn(|| sink.admit_borrowed("contended"));
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(5);
            while sink.inner.waits.load(Ordering::Acquire) == 0 {
                assert!(
                    !admission.is_finished(),
                    "admission finished while the queue was held"
                );
                assert!(
                    std::time::Instant::now() < deadline,
                    "admission never reached the held queue"
                );
                std::thread::yield_now();
            }
            drop(queue);
            assert_eq!(
                admission.join().unwrap(),
                HostEffectAdmission::Accepted
            );
        });
        assert_eq!(text(&recipient.try_next().unwrap()), "held");
        assert_eq!(text(&recipient.try_next().unwrap()), "contended");
        assert!(recipient.try_next().is_none());
    }

    #[test]
    fn close_invalidates_popped_and_queued_deliveries() {
        let (sink, recipient) = test_fixture(TerminalId::new(8));
        assert_eq!(
            sink.admit_borrowed("popped"),
            HostEffectAdmission::Accepted
        );
        let popped = recipient.try_next().expect("effect should be queued");
        assert_eq!(
            sink.admit_borrowed("queued"),
            HostEffectAdmission::Accepted
        );

        sink.close();

        assert!(!recipient.is_current(&popped));
        assert!(recipient.try_next().is_none());
        assert_eq!(sink.admit_borrowed("late"), HostEffectAdmission::Closed);
    }

    #[test]
    fn popped_delivery_keeps_capacity_reserved_across_permission_change() {
        let (sink, recipient) = test_fixture(TerminalId::new(9));
        assert_eq!(sink.admit_borrowed("held"), HostEffectAdmission::Accepted);
        let held = recipient.try_next().expect("effect should be queued");

        recipient.set_allowed(HostEffectKind::Clipboard, false);
        recipient.set_allowed(HostEffectKind::Clipboard, true);
        for _ in 1..TERMINAL_EFFECT_LIMIT {
            assert_eq!(
                sink.admit_borrowed("queued"),
                HostEffectAdmission::Accepted
            );
        }
        assert_eq!(sink.admit_borrowed("full"), HostEffectAdmission::Full);

        drop(held);
        assert_eq!(
            sink.admit_borrowed("released"),
            HostEffectAdmission::Accepted
        );
    }

    #[test]
    fn permission_revocation_wins_over_concurrent_admission() {
        use std::sync::Barrier;

        let (sink, recipient) = test_fixture(TerminalId::new(10));
        assert_eq!(
            sink.admit_borrowed("queued"),
            HostEffectAdmission::Accepted
        );
        let queue = recipient
            .registration
            .queue
            .lock()
            .expect("test queue should not be poisoned");
        let revoked = Arc::new(Barrier::new(2));

        std::thread::scope(|scope| {
            let revoked_worker = Arc::clone(&revoked);
            let registration = &recipient.registration;
            scope.spawn(move || {
                registration
                    .begin_permission_revocation(HostEffectKind::Clipboard);
                revoked_worker.wait();
                registration.clear_queue(|_| true);
            });
            revoked.wait();

            assert_eq!(
                sink.admit_borrowed("racing"),
                HostEffectAdmission::Denied
            );
            drop(queue);
        });

        assert!(recipient.try_next().is_none());
    }

    /// A recipient of a client of its own whose most recent activity is
    /// `ordinal`, so tests choose which one admission selects.
    fn client_recipient(
        sink: &HostEffectSink,
        process: &DesktopHostEffectClient,
        attachment: u64,
        ordinal: u64,
    ) -> HostEffectRecipient {
        sink.register(
            Some(AttachmentId::new(attachment)),
            process,
            HostEffectRecipientOptions::local_desktop(
                HostEffectPermissions::ALL,
            ),
            RecipientLink {
                activity: None,
                ordinal: Arc::new(AtomicU64::new(ordinal)),
            },
        )
        .expect("test registration should fit")
    }

    #[test]
    fn a_lone_client_keeps_the_whole_queue_and_two_clients_split_it() {
        let sink = HostEffectSink::new(TerminalId::new(19));
        let first = DesktopHostEffectClient::new();
        let lone = client_recipient(&sink, &first, 1, 3);
        // Another of the same client's viewers is still one client.
        let sibling = client_recipient(&sink, &first, 2, 2);
        for _ in 0..TERMINAL_EFFECT_LIMIT {
            assert_eq!(sink.admit_borrowed(""), HostEffectAdmission::Accepted);
        }
        assert_eq!(sink.admit_borrowed(""), HostEffectAdmission::Full);
        while lone.try_next().is_some() {}

        // A second client halves the share. The first, more active, fills
        // its half; the second still gets every effect once it is chosen.
        let second = DesktopHostEffectClient::new();
        let other = client_recipient(&sink, &second, 3, 1);
        let half = TERMINAL_EFFECT_LIMIT / 2;
        for _ in 0..half {
            assert_eq!(sink.admit_borrowed(""), HostEffectAdmission::Accepted);
        }
        assert_eq!(sink.admit_borrowed(""), HostEffectAdmission::Full);
        drop(lone);
        drop(sibling);
        assert!(
            other.try_next().is_none(),
            "nothing was redirected to the other client"
        );
        let third = DesktopHostEffectClient::new();
        let _idle = client_recipient(&sink, &third, 4, 0);
        // The dropped client's reservations went with its queue, so the
        // remaining two clients each take their half.
        for _ in 0..half {
            assert_eq!(sink.admit_borrowed(""), HostEffectAdmission::Accepted);
        }
        assert_eq!(sink.admit_borrowed(""), HostEffectAdmission::Full);
        let mut delivered = 0;
        while other.try_next().is_some() {
            delivered += 1;
        }
        assert_eq!(delivered, half);
    }

    #[test]
    fn ineligible_clients_take_no_share_and_shares_have_a_floor() {
        let sink = HostEffectSink::new(TerminalId::new(20));
        let desktop = DesktopHostEffectClient::new();
        let recipient = client_recipient(&sink, &desktop, 1, 1);
        let text = DesktopHostEffectClient::new();
        let _text = sink
            .register(
                Some(AttachmentId::new(2)),
                &text,
                HostEffectRecipientOptions {
                    origin: HostEffectClientOrigin::LocalText,
                    writable: true,
                    permissions: HostEffectPermissions::ALL,
                },
                RecipientLink::default(),
            )
            .unwrap();
        for _ in 0..TERMINAL_EFFECT_LIMIT {
            assert_eq!(sink.admit_borrowed(""), HostEffectAdmission::Accepted);
        }
        while recipient.try_next().is_some() {}
        assert_eq!(client_share(5), MIN_CLIENT_SHARE);
        assert_eq!(client_share(2), TERMINAL_EFFECT_LIMIT / 2);
    }

    #[test]
    fn caller_numbered_permissions_stamp_effects_and_refuse_stale_numbers() {
        let (sink, recipient) = test_fixture(TerminalId::new(21));
        assert!(recipient.set_permission(HostEffectKind::Clipboard, true, 5));
        assert_eq!(sink.admit_borrowed("five"), HostEffectAdmission::Accepted);
        let five = recipient.try_next().unwrap();
        assert_eq!(five.metadata().recipient_generation, 5);
        assert!(five.is_current());

        // A revoke cancels queued deliveries and a re-grant does not
        // revive one already popped.
        assert_eq!(
            sink.admit_borrowed("queued"),
            HostEffectAdmission::Accepted
        );
        assert!(recipient.set_permission(HostEffectKind::Clipboard, false, 6));
        assert!(!five.is_current());
        assert!(recipient.try_next().is_none(), "the queued copy is gone");
        assert_eq!(sink.admit_borrowed("denied"), HostEffectAdmission::Denied);
        assert!(recipient.set_permission(HostEffectKind::Clipboard, true, 7));
        assert!(!five.is_current());
        assert_eq!(sink.admit_borrowed("seven"), HostEffectAdmission::Accepted);
        let seven = recipient.try_next().unwrap();
        assert_eq!(seven.metadata().recipient_generation, 7);
        assert!(seven.is_current() && recipient.is_current(&seven));

        // A stale or repeated number changes nothing.
        assert!(!recipient.set_permission(HostEffectKind::Clipboard, false, 7));
        assert!(!recipient.set_permission(HostEffectKind::Clipboard, false, 3));
        assert!(seven.is_current());
        // Another kind keeps its own numbers.
        assert!(recipient.set_permission(
            HostEffectKind::Notification,
            false,
            2
        ));
        assert!(seven.is_current());
    }

    #[test]
    fn revoking_a_held_recipient_ends_its_deliveries_and_selection() {
        let (sink, recipient) = test_fixture(TerminalId::new(22));
        assert_eq!(
            sink.admit_borrowed("popped"),
            HostEffectAdmission::Accepted
        );
        let popped = recipient.try_next().unwrap();
        assert_eq!(
            sink.admit_borrowed("queued"),
            HostEffectAdmission::Accepted
        );
        recipient.revoke();
        assert!(!popped.is_current());
        assert!(recipient.try_next().is_none());
        assert_eq!(sink.admit_borrowed("late"), HostEffectAdmission::Denied);
        // Granting again cannot bring a revoked recipient back.
        assert!(recipient.set_permission(HostEffectKind::Clipboard, true, 9));
        assert_eq!(sink.admit_borrowed("later"), HostEffectAdmission::Denied);
    }
}
