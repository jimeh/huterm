//! A connection's host-effect lane: which viewers may hold effects, the one
//! effect being sent, its deadlines, and the client's permission changes.
//!
//! Core signals an admitted effect through its viewer's wake, the same one
//! publications use, so every wake of a viewer with a recipient puts it in
//! the ready queue. Whenever the effect slot is idle, the lane takes the
//! next current effect from the ready viewers in turn, keeping a viewer
//! queued while it yields effects, so an effect queued behind a large write
//! goes out without another wake. A viewer's effects wait until its
//! subscription's result is written, because an effect overtakes the
//! viewer's lane and the client must know the viewer first.
//!
//! Once an effect reaches the head of the lane, its first frame must be
//! written within the delivery deadline, and its last within that plus one
//! second per [`HOST_EFFECT_MIN_THROUGHPUT`] bytes. One that misses either
//! is dropped at once with its reservations, and its message is aborted
//! after the frame already with the writer. An effect reaches the head as
//! soon as no other is being sent, even while an earlier one's last frame
//! or abort still waits at the writer.

use std::time::Duration;

use huterm_core::{
    HostEffectClientOrigin, HostEffectPermissions, HostEffectRecipientOptions,
    HostEffectViewerOptions,
};
use huterm_protocol::{HostEffectKind, ViewerId};
use huterm_wire::effect::{
    EffectKinds, SetHostEffectPermission, host_effect_head,
};
use huterm_wire::handshake::{
    ClientKind, HOST_EFFECT_DELIVERY_DEADLINE, HOST_EFFECT_MIN_THROUGHPUT,
};

use super::outbound::EffectProgress;
use super::{CloseReason, Connection};
use crate::viewer::PendingEffect;

/// The effect at the head of the lane, waiting for the slot, and when it
/// got there.
#[derive(Debug)]
pub(super) struct Waiting {
    viewer: ViewerId,
    effect: Box<dyn PendingEffect>,
    reached: Duration,
}

/// The effect being sent and its deadlines.
#[derive(Clone, Copy, Debug)]
pub(super) struct Sending {
    /// When its first frame must be written.
    first: Duration,
    /// When its last frame must be written.
    complete: Duration,
}

/// What the lane did, for tests and the fault harness.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EffectStats {
    /// Effects whose message started.
    pub sent: u64,
    /// Effects dropped at a deadline: waiting at the head of the lane, or
    /// aborted while being sent.
    pub expired: u64,
}

/// How long an effect of `len` bytes may take from the head of the lane to
/// its last frame.
fn completion(len: usize) -> Duration {
    let nanos = (len as u128) * 1_000_000_000
        / u128::from(HOST_EFFECT_MIN_THROUGHPUT.max(1));
    HOST_EFFECT_DELIVERY_DEADLINE
        + Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// The recipient origin a client kind gets. Policy, never authentication:
/// slice 5's peer check makes every connection a local peer of the same
/// user, and a later remote transport adds its own peer identity.
const fn origin(kind: ClientKind) -> HostEffectClientOrigin {
    match kind {
        ClientKind::Desktop => HostEffectClientOrigin::LocalDesktop,
        ClientKind::Text | ClientKind::CommandLine => {
            HostEffectClientOrigin::LocalText
        }
        ClientKind::Gateway => HostEffectClientOrigin::Remote,
    }
}

impl Connection {
    /// The effect lane's counts.
    #[must_use]
    pub fn effect_stats(&self) -> EffectStats {
        self.effect_stats
    }

    /// The recipient a subscription that asked for host effects registers:
    /// the connection's own budget, an origin from its client kind, and the
    /// kinds the client permits that its negotiated version knows.
    pub(super) fn effect_options(
        &self,
        allowed: EffectKinds,
    ) -> Option<HostEffectViewerOptions> {
        let negotiated = self.negotiated?;
        let known = EffectKinds::known_at(negotiated.caps.minor());
        let allowed = allowed.intersect(known);
        Some(HostEffectViewerOptions {
            process: self.effect_client.clone(),
            options: HostEffectRecipientOptions {
                origin: origin(negotiated.client_kind),
                writable: true,
                permissions: HostEffectPermissions {
                    clipboard: allowed.contains(HostEffectKind::Clipboard),
                    notifications: allowed
                        .contains(HostEffectKind::Notification),
                },
            },
        })
    }

    /// Marks `viewer` as one that may hold effects, after its wake.
    pub(super) fn effect_woke(&mut self, viewer: ViewerId) {
        if self
            .viewers
            .get(&viewer)
            .is_some_and(|live| live.effects.is_some())
            && self.effect_ready.insert(viewer)
        {
            self.effect_queue.push_back(viewer);
        }
    }

    /// Moves the next current effect to the head of the lane and starts it
    /// once the slot is idle. While the slot still drains an earlier
    /// effect's last frame or abort, the next effect waits at the head with
    /// its delivery deadline running, so a client that reads slowly cannot
    /// hold queued effects, and their reservations, without a bound.
    pub(super) fn offer_effects(&mut self) {
        if self.closed.is_some() {
            return;
        }
        let progress = self.outbound.effect_progress();
        if matches!(progress, EffectProgress::Sending { .. }) {
            return;
        }
        if self
            .waiting
            .as_ref()
            .is_some_and(|waiting| !waiting.effect.is_current())
        {
            self.waiting = None;
        }
        if self.waiting.is_none() {
            self.waiting = self.next_ready();
        }
        if progress != EffectProgress::Idle {
            return;
        }
        self.sending = None;
        while let Some(waiting) = self.waiting.take() {
            if self.start_effect(waiting) {
                return;
            }
            self.waiting = self.next_ready();
        }
    }

    /// The next current effect from the ready viewers in turn, at the head
    /// of the lane from now.
    fn next_ready(&mut self) -> Option<Waiting> {
        let mut turns = self.effect_queue.len();
        while turns > 0 {
            turns -= 1;
            let id = self.effect_queue.pop_front()?;
            let Some(live) = self.viewers.get(&id) else {
                self.effect_ready.remove(&id);
                continue;
            };
            if !self.outbound.viewer_announced(id) {
                // Its subscription's result is not out yet.
                self.effect_queue.push_back(id);
                continue;
            }
            let Some(effect) = live.port.next_effect() else {
                self.effect_ready.remove(&id);
                continue;
            };
            // The viewer may hold more; it stays in turn.
            self.effect_queue.push_back(id);
            return Some(Waiting {
                viewer: id,
                effect,
                reached: self.clock,
            });
        }
        None
    }

    /// Starts `waiting`'s message, or drops it when it is no longer current
    /// or has no wire form. Returns whether it started.
    fn start_effect(&mut self, waiting: Waiting) -> bool {
        let Waiting {
            viewer,
            effect,
            reached,
        } = waiting;
        let Some(payload) = effect.payload() else {
            return false;
        };
        if !effect.is_current() {
            return false;
        }
        self.last_effect += 1;
        // A payload past the wire's limits, which core's bounds rule out,
        // is dropped rather than sent.
        let Ok(head) = host_effect_head(
            self.scope,
            self.last_effect,
            viewer,
            effect.generation(),
            effect.terminal(),
            payload,
        ) else {
            return false;
        };
        let len = head.len() + payload.streamed().len();
        self.outbound.start_effect(head, effect);
        self.effect_stats.sent += 1;
        self.sending = Some(Sending {
            first: reached + HOST_EFFECT_DELIVERY_DEADLINE,
            complete: reached + completion(len),
        });
        true
    }

    /// The effect lane's next deadline: the waiting effect's delivery
    /// deadline, or the sending effect's first-frame or completion one.
    pub(super) fn effect_deadline(&self) -> Option<Duration> {
        if let Some(waiting) = &self.waiting {
            return Some(waiting.reached + HOST_EFFECT_DELIVERY_DEADLINE);
        }
        let sending = self.sending?;
        match self.outbound.effect_progress() {
            EffectProgress::Sending {
                first_written: false,
            } => Some(sending.first),
            EffectProgress::Sending {
                first_written: true,
            } => Some(sending.complete),
            EffectProgress::Idle | EffectProgress::Draining => None,
        }
    }

    /// Drops an effect that missed its deadline at `now`: the waiting one,
    /// or the one being sent, whose message is aborted unless its last
    /// frame is already cut, in which case it still arrives.
    pub(super) fn check_effect_deadline(&mut self, now: Duration) {
        if self.effect_deadline().is_none_or(|at| now < at) {
            return;
        }
        if self.waiting.take().is_some() {
            self.effect_stats.expired += 1;
            return;
        }
        self.sending = None;
        if self.outbound.abort_effect() {
            self.effect_stats.expired += 1;
        }
    }

    /// Applies the client's numbered permission change, in order with the
    /// viewer's input.
    pub(super) fn effect_permission(
        &mut self,
        change: SetHostEffectPermission,
    ) {
        let SetHostEffectPermission {
            viewer,
            kind,
            generation,
            allowed,
        } = change;
        let Some(live) = self.viewers.get_mut(&viewer) else {
            // A change for a viewer that ended is dropped, as its input is.
            return;
        };
        let Some(generations) = live.effects.as_mut() else {
            self.shut(CloseReason::Violation(
                "a permission change for a viewer without host effects".into(),
            ));
            return;
        };
        let current = generations[kind.index()];
        if generation <= current {
            self.shut(CloseReason::Violation(format!(
                "permission generation {generation} after {current}"
            )));
            return;
        }
        generations[kind.index()] = generation;
        // Core numbers the recipient's generation from the same start, so
        // it takes every change the connection accepted.
        let _ = live.port.set_effect_permission(kind, allowed, generation);
    }

    /// Withdraws every recipient as the connection ends, so no effect is
    /// queued for a viewer a job may still hold.
    pub(super) fn revoke_effects(&mut self) {
        for live in self.viewers.values() {
            if live.effects.is_some() {
                live.port.revoke_effects();
            }
        }
        self.effect_queue.clear();
        self.effect_ready.clear();
        self.waiting = None;
        self.sending = None;
    }
}
