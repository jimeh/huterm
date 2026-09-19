use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use huterm_protocol::{
    AttachmentId, ClipboardDestination, ClipboardWrite, HostEffect,
    HostEffectMetadata, HostEffectOrigin, HostEffectRecipientId, TerminalId,
};

const TERMINAL_EFFECT_LIMIT: usize = 8;
const TERMINAL_BYTE_LIMIT: usize = 16 * 1024 * 1024;
const PROCESS_EFFECT_LIMIT: usize = 32;
const PROCESS_BYTE_LIMIT: usize = 32 * 1024 * 1024;
const TERMINAL_RECIPIENT_LIMIT: usize = 32;

/// Connection boundary represented by a host-effect recipient.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostEffectClientOrigin {
    /// A view embedded in the local desktop process.
    LocalEmbeddedDesktop,
    /// A local text client without desktop clipboard authority.
    LocalText,
    /// A client connected across a process or network boundary.
    Remote,
}

/// Capability and policy state used when registering a host-effect recipient.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostEffectRecipientOptions {
    /// Connection boundary of this client.
    pub origin: HostEffectClientOrigin,
    /// Whether this client is allowed to perform state-changing host effects.
    pub writable: bool,
    /// Current terminal-content clipboard permission.
    pub clipboard_allowed: bool,
}

impl HostEffectRecipientOptions {
    /// Creates options for a writable local desktop view.
    #[must_use]
    pub const fn local_desktop(clipboard_allowed: bool) -> Self {
        Self {
            origin: HostEffectClientOrigin::LocalEmbeddedDesktop,
            writable: true,
            clipboard_allowed,
        }
    }
}

/// Shared desktop-process budget used by every window and terminal registration.
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
    bytes: usize,
}

impl Drop for EffectReservation {
    fn drop(&mut self) {
        for budget in [&self.terminal, &self.process] {
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
}

#[derive(Debug)]
struct Registration {
    id: HostEffectRecipientId,
    generation: AtomicU64,
    active: AtomicBool,
    attachment: AttachmentId,
    options: HostEffectRecipientOptions,
    allowed: AtomicBool,
    focus_ordinal: AtomicU64,
    order: u64,
    process_budget: Arc<Budget>,
    queue: Mutex<VecDeque<PendingHostEffect>>,
}

impl Registration {
    fn is_eligible(&self) -> bool {
        self.options.origin == HostEffectClientOrigin::LocalEmbeddedDesktop
            && self.options.writable
            && self.active.load(Ordering::Acquire)
            && self.allowed.load(Ordering::Acquire)
    }

    fn advance_generation(&self) {
        if self
            .generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .is_err()
        {
            self.active.store(false, Ordering::Release);
        }
    }

    fn clear_queue(&self) {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    fn revoke_permission(&self) {
        self.begin_permission_revocation();
        self.clear_queue();
    }

    fn begin_permission_revocation(&self) {
        self.allowed.store(false, Ordering::Release);
        self.advance_generation();
    }

    fn revoke_permanently(&self) {
        self.active.store(false, Ordering::Release);
        self.advance_generation();
        self.clear_queue();
    }
}

/// Non-cloneable UI-side registration for receiving terminal host effects.
#[derive(Debug)]
pub struct HostEffectRecipient {
    registration: Arc<Registration>,
    sink: Weak<SinkInner>,
}

impl HostEffectRecipient {
    /// Updates terminal-content clipboard permission for this recipient.
    /// Removing permission cancels queued and already-popped deliveries.
    pub fn set_allowed(&self, allowed: bool) {
        if allowed {
            if self.registration.active.load(Ordering::Acquire) {
                self.registration.allowed.store(true, Ordering::Release);
            }
        } else {
            self.registration.revoke_permission();
        }
    }

    /// Records that this view most recently held focus.
    pub fn note_focus(&self) {
        let Some(sink) = self.sink.upgrade() else {
            return;
        };
        let Ok(previous) = sink.next_focus.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |ordinal| ordinal.checked_add(1),
        ) else {
            return;
        };
        self.registration
            .focus_ordinal
            .store(previous + 1, Ordering::Release);
    }

    /// Removes and returns the next currently valid delivery without blocking.
    #[must_use]
    pub fn try_next(&self) -> Option<PendingHostEffect> {
        let mut queue = self.registration.queue.try_lock().ok()?;
        while let Some(pending) = queue.pop_front() {
            if self.is_current(&pending) {
                return Some(pending);
            }
        }
        None
    }

    /// Rechecks that a popped delivery still belongs to this live generation.
    /// Call immediately before invoking the platform host operation. An operation
    /// that has already started cannot be undone by later revocation.
    #[must_use]
    pub fn is_current(&self, pending: &PendingHostEffect) -> bool {
        pending.registration.upgrade().is_some_and(|registration| {
            Arc::ptr_eq(&registration, &self.registration)
                && registration.active.load(Ordering::Acquire)
                && registration.allowed.load(Ordering::Acquire)
                && registration.generation.load(Ordering::Acquire)
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
}

#[derive(Debug)]
struct SinkInner {
    activity: Option<Weak<async_channel::Sender<()>>>,
    terminal_id: TerminalId,
    closed: AtomicBool,
    terminal_budget: Arc<Budget>,
    next_focus: AtomicU64,
    next_sequence: AtomicU64,
    state: Mutex<SinkState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostEffectAdmission {
    Accepted,
    NoRecipient,
    Denied,
    Full,
    Contended,
    Closed,
}

/// Nonblocking terminal-side admission point for client-mediated host effects.
#[derive(Clone, Debug)]
pub(crate) struct HostEffectSink {
    inner: Arc<SinkInner>,
}

impl HostEffectSink {
    #[cfg(test)]
    pub(crate) fn new(terminal_id: TerminalId) -> Self {
        Self::new_with_activity(terminal_id, None)
    }

    pub(crate) fn new_with_activity(
        terminal_id: TerminalId,
        activity: Option<Weak<async_channel::Sender<()>>>,
    ) -> Self {
        Self {
            inner: Arc::new(SinkInner {
                activity,
                terminal_id,
                closed: AtomicBool::new(false),
                terminal_budget: Arc::new(Budget::new(
                    TERMINAL_EFFECT_LIMIT,
                    TERMINAL_BYTE_LIMIT,
                )),
                next_focus: AtomicU64::new(0),
                next_sequence: AtomicU64::new(0),
                state: Mutex::new(SinkState {
                    registrations: Vec::new(),
                    next_registration: 0,
                }),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn admit_owned(&self, text: String) -> HostEffectAdmission {
        let bytes = text.capacity();
        self.admit(bytes, || text)
    }

    pub(crate) fn admit_borrowed(&self, text: &str) -> HostEffectAdmission {
        self.admit(text.len(), || text.to_owned())
    }

    fn admit(
        &self,
        bytes: usize,
        make_text: impl FnOnce() -> String,
    ) -> HostEffectAdmission {
        if self.inner.closed.load(Ordering::Acquire) {
            return HostEffectAdmission::Closed;
        }
        let (registration, selected_generation) = match self.select_recipient()
        {
            Ok(selected) => selected,
            Err(status) => return status,
        };

        let Ok(mut terminal_budget) =
            self.inner.terminal_budget.state.try_lock()
        else {
            return HostEffectAdmission::Contended;
        };
        let Ok(mut process_budget) =
            registration.process_budget.state.try_lock()
        else {
            return HostEffectAdmission::Contended;
        };
        if !self
            .inner
            .terminal_budget
            .can_reserve(&terminal_budget, bytes)
            || !registration
                .process_budget
                .can_reserve(&process_budget, bytes)
        {
            return HostEffectAdmission::Full;
        }
        let Ok(mut queue) = registration.queue.try_lock() else {
            return HostEffectAdmission::Contended;
        };
        if !registration.is_eligible()
            || registration.generation.load(Ordering::Acquire)
                != selected_generation
        {
            return HostEffectAdmission::Denied;
        }

        let text = make_text();
        let actual_bytes = text.capacity();
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
        let Ok(sequence) = self.inner.next_sequence.fetch_update(
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
        queue.push_back(PendingHostEffect {
            metadata: HostEffectMetadata {
                origin: HostEffectOrigin {
                    terminal_id: self.inner.terminal_id,
                    sequence,
                },
                recipient: registration.id,
                recipient_generation: selected_generation,
            },
            effect: HostEffect::ClipboardWrite(ClipboardWrite::new(
                ClipboardDestination::System,
                text,
            )),
            registration: Arc::downgrade(&registration),
            _reservation: EffectReservation {
                terminal: Arc::clone(&self.inner.terminal_budget),
                process: Arc::clone(&registration.process_budget),
                bytes: actual_bytes,
            },
        });
        drop(queue);
        if let Some(signal) =
            self.inner.activity.as_ref().and_then(Weak::upgrade)
        {
            let _ = signal.try_send(());
        }
        HostEffectAdmission::Accepted
    }

    fn select_recipient(
        &self,
    ) -> Result<(Arc<Registration>, u64), HostEffectAdmission> {
        let Ok(mut state) = self.inner.state.try_lock() else {
            return Err(HostEffectAdmission::Contended);
        };
        state.registrations.retain(|weak| weak.strong_count() != 0);
        if state.registrations.is_empty() {
            return Err(HostEffectAdmission::NoRecipient);
        }
        let Some(registration) = state
            .registrations
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|registration| registration.is_eligible())
            .max_by(|left, right| {
                left.focus_ordinal
                    .load(Ordering::Acquire)
                    .cmp(&right.focus_ordinal.load(Ordering::Acquire))
                    .then_with(|| right.order.cmp(&left.order))
            })
        else {
            return Err(HostEffectAdmission::Denied);
        };
        let selected_generation =
            registration.generation.load(Ordering::Acquire);
        Ok((registration, selected_generation))
    }

    pub(crate) fn close(&self) {
        self.inner.closed.store(true, Ordering::Release);
        self.invalidate_all();
    }

    pub(crate) fn invalidate_attachment(&self, attachment: AttachmentId) {
        self.invalidate_where(|registration| {
            registration.attachment == attachment
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
        attachment: AttachmentId,
        process: &DesktopHostEffectClient,
        options: HostEffectRecipientOptions,
    ) -> Option<HostEffectRecipient> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.registrations.retain(|weak| weak.strong_count() != 0);
        if self.inner.closed.load(Ordering::Acquire)
            || state.registrations.len() >= TERMINAL_RECIPIENT_LIMIT
        {
            return None;
        }
        state.next_registration = state.next_registration.checked_add(1)?;
        let registration_order = state.next_registration;
        let registration = Arc::new(Registration {
            id: HostEffectRecipientId::new(registration_order),
            generation: AtomicU64::new(1),
            active: AtomicBool::new(true),
            attachment,
            options,
            allowed: AtomicBool::new(options.clipboard_allowed),
            focus_ordinal: AtomicU64::new(0),
            order: registration_order,
            process_budget: Arc::clone(&process.process_budget),
            queue: Mutex::new(VecDeque::new()),
        });
        state.registrations.push(Arc::downgrade(&registration));
        Some(HostEffectRecipient {
            registration,
            sink: Arc::downgrade(&self.inner),
        })
    }
}

#[cfg(test)]
pub(crate) fn test_fixture(
    terminal_id: TerminalId,
) -> (HostEffectSink, HostEffectRecipient) {
    let sink = HostEffectSink::new(terminal_id);
    let process = DesktopHostEffectClient::new();
    let recipient = sink
        .register(
            AttachmentId::new(1),
            &process,
            HostEffectRecipientOptions::local_desktop(true),
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
        sink.register(AttachmentId::new(attachment), process, options)
            .expect("test registration should fit")
    }

    fn text(pending: &PendingHostEffect) -> &str {
        match pending.effect() {
            HostEffect::ClipboardWrite(write) => write.text(),
            effect => panic!("unexpected host effect: {effect:?}"),
        }
    }

    #[test]
    fn admitted_effect_wakes_without_a_terminal_event_and_does_not_keep_signal_alive()
     {
        let (sender, signal) = async_channel::bounded(1);
        let sender = Arc::new(sender);
        let sink = HostEffectSink::new_with_activity(
            TerminalId::new(1),
            Some(Arc::downgrade(&sender)),
        );
        let process = DesktopHostEffectClient::new();
        let recipient = recipient(
            &sink,
            &process,
            1,
            HostEffectRecipientOptions::local_desktop(true),
        );
        assert_eq!(sink.admit_borrowed("copy"), HostEffectAdmission::Accepted);
        signal.try_recv().unwrap();
        assert_eq!(text(&recipient.try_next().unwrap()), "copy");
        drop(sender);
        assert!(signal.is_closed());
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
                HostEffectRecipientOptions::local_desktop(true),
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
            HostEffectRecipientOptions::local_desktop(true),
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
                HostEffectRecipientOptions::local_desktop(true),
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
                HostEffectRecipientOptions::local_desktop(true),
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

        recipient.set_allowed(false);
        assert!(!recipient.is_current(&stale));
        assert_eq!(sink.admit_borrowed("denied"), HostEffectAdmission::Denied);

        recipient.set_allowed(true);
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
    fn focus_selects_one_recipient_without_fallback_or_replay() {
        let sink = HostEffectSink::new(TerminalId::new(3));
        let process = DesktopHostEffectClient::new();
        let first = recipient(
            &sink,
            &process,
            1,
            HostEffectRecipientOptions::local_desktop(true),
        );
        let second = recipient(
            &sink,
            &process,
            2,
            HostEffectRecipientOptions::local_desktop(true),
        );

        assert_eq!(sink.admit_borrowed("first"), HostEffectAdmission::Accepted);
        assert_eq!(
            text(&first.try_next().expect("first registered wins tie")),
            "first"
        );
        assert!(second.try_next().is_none());

        second.note_focus();
        assert_eq!(
            sink.admit_borrowed("second"),
            HostEffectAdmission::Accepted
        );
        drop(second);
        assert!(first.try_next().is_none());
        assert_eq!(sink.admit_borrowed("again"), HostEffectAdmission::Accepted);
        assert_eq!(
            text(&first.try_next().expect("future admission can use first")),
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
                    clipboard_allowed: true,
                },
            ),
            (
                5,
                HostEffectRecipientOptions {
                    origin: HostEffectClientOrigin::Remote,
                    writable: true,
                    clipboard_allowed: true,
                },
            ),
            (
                6,
                HostEffectRecipientOptions {
                    origin: HostEffectClientOrigin::LocalEmbeddedDesktop,
                    writable: false,
                    clipboard_allowed: true,
                },
            ),
        ] {
            let sink = HostEffectSink::new(TerminalId::new(id));
            let _recipient = recipient(&sink, &process, id, options);
            assert_eq!(sink.admit_borrowed("no"), HostEffectAdmission::Denied);
        }
    }

    #[test]
    fn admission_drops_on_queue_contention_while_capacity_is_reserved() {
        let (sink, recipient) = test_fixture(TerminalId::new(7));
        assert_eq!(sink.admit_borrowed("held"), HostEffectAdmission::Accepted);
        let queue = recipient
            .registration
            .queue
            .lock()
            .expect("test queue should not be poisoned");

        assert_eq!(
            sink.admit_borrowed("contended"),
            HostEffectAdmission::Contended
        );

        drop(queue);
        assert_eq!(
            text(&recipient.try_next().expect("held effect remains")),
            "held"
        );
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

        recipient.set_allowed(false);
        recipient.set_allowed(true);
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
                registration.begin_permission_revocation();
                revoked_worker.wait();
                registration.clear_queue();
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
}
