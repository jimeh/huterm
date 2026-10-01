//! Sequenced structural events published to bounded subscriptions.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use huterm_protocol::{
    HierarchyEnvelope, HierarchyEvent, HierarchyState, SessionInfo, StreamId,
    TabInfo, WorkspaceInfo,
};
use thiserror::Error;

use super::{Mux, Session, Tab, Workspace};

/// Events one subscription may hold before it lags. Well above one burst of
/// interactive structural operations.
pub(crate) const HIERARCHY_QUEUE_CAPACITY: usize = 1024;

#[derive(Debug, Default)]
struct Queue {
    events: VecDeque<HierarchyEnvelope>,
    lagged: bool,
}

#[derive(Debug)]
struct Subscriber {
    queue: Weak<Mutex<Queue>>,
    wake: async_channel::Sender<()>,
}

/// The Mux-owned hierarchy sequence and its subscribers.
#[derive(Debug, Default)]
pub(super) struct HierarchyPublisher {
    seq: u64,
    subscribers: Vec<Subscriber>,
    /// Events built for delivery, observed by tests.
    #[cfg(test)]
    built: usize,
}

impl HierarchyPublisher {
    /// Drops subscribers whose subscription was dropped and reports whether
    /// any remain.
    fn has_subscribers(&mut self) -> bool {
        self.prune();
        !self.subscribers.is_empty()
    }

    /// Forgets subscribers whose subscription was dropped.
    fn prune(&mut self) {
        self.subscribers
            .retain(|subscriber| subscriber.queue.strong_count() > 0);
    }

    fn publish(&self, envelope: &HierarchyEnvelope) {
        for subscriber in &self.subscribers {
            let Some(queue) = subscriber.queue.upgrade() else {
                continue;
            };
            let mut queue =
                queue.lock().unwrap_or_else(PoisonError::into_inner);
            if queue.lagged {
                continue;
            }
            if queue.events.len() >= HIERARCHY_QUEUE_CAPACITY {
                queue.events = VecDeque::new();
                queue.lagged = true;
            } else {
                queue.events.push_back(envelope.clone());
            }
            drop(queue);
            let _ = subscriber.wake.try_send(());
        }
    }
}

/// A bounded, ordered feed of the hierarchy events that follow one snapshot.
///
/// Dropping the subscription unregisters it. The publisher never waits for a
/// subscriber to drain: queue access is a short constant-time critical
/// section, and when the queue would overflow, it discards the queue and the
/// subscription reports [`HierarchyRecvError::Lagged`] until it is replaced.
#[derive(Debug)]
pub struct HierarchySubscription {
    queue: Arc<Mutex<Queue>>,
    wake: async_channel::Receiver<()>,
}

impl HierarchySubscription {
    /// Takes the next queued event.
    ///
    /// # Errors
    /// Returns `Empty` when no event is queued, `Lagged` after the queue
    /// overflowed, and `Closed` once the Mux is gone and nothing remains.
    pub fn try_recv(&self) -> Result<HierarchyEnvelope, HierarchyRecvError> {
        let mut queue =
            self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        if queue.lagged {
            return Err(HierarchyRecvError::Lagged);
        }
        queue.events.pop_front().ok_or_else(|| {
            if self.wake.is_closed() {
                HierarchyRecvError::Closed
            } else {
                HierarchyRecvError::Empty
            }
        })
    }

    /// Completes once events, or a lag, arrived since the last call.
    ///
    /// The signal carries no data and coalesces: drain with
    /// [`Self::try_recv`] until it reports `Empty`.
    ///
    /// # Errors
    /// Returns `Closed` once the Mux has been dropped.
    pub async fn wait_for_activity(&self) -> Result<(), HierarchyRecvError> {
        self.wake
            .recv()
            .await
            .map_err(|_| HierarchyRecvError::Closed)
    }

    /// Consumes a pending wake signal without waiting.
    #[cfg(test)]
    pub(crate) fn take_wake(&self) -> bool {
        self.wake.try_recv().is_ok()
    }
}

/// Why a hierarchy subscription returned no event.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HierarchyRecvError {
    /// No event is queued.
    #[error("no hierarchy event is queued")]
    Empty,
    /// The queue overflowed and events were discarded; resubscribe.
    #[error("hierarchy subscription lagged; resubscribe")]
    Lagged,
    /// The Mux was dropped and no queued event remains.
    #[error("hierarchy stream closed")]
    Closed,
}

impl Mux {
    /// Returns the sequence of the latest emitted hierarchy event.
    ///
    /// It advances by one per committed structural change, with or without
    /// subscribers, and is independent of close-ticket revisions.
    #[must_use]
    pub fn hierarchy_seq(&self) -> u64 {
        self.hierarchy.seq
    }

    /// Captures the current hierarchy and subscribes to every later event.
    ///
    /// The snapshot is current through [`Self::hierarchy_seq`] and the
    /// subscription's first event carries the following sequence.
    ///
    /// # Panics
    /// Panics if canonical structure repeats an identity, which would be a
    /// Mux invariant violation.
    pub fn subscribe_hierarchy(
        &mut self,
    ) -> (HierarchyState, HierarchySubscription) {
        let state = HierarchyState::from_sessions(
            StreamId::new(self.runtime_id),
            self.hierarchy.seq,
            self.sessions.iter().map(|session| {
                (
                    session.info(),
                    session.workspaces.iter().map(|id| {
                        let workspace = &self.workspaces[id];
                        (workspace.info(), workspace.tabs.iter().map(Tab::info))
                    }),
                )
            }),
        )
        .expect("canonical hierarchy has unique, scoped identities");
        let queue = Arc::new(Mutex::new(Queue::default()));
        let (wake, signal) = async_channel::bounded(1);
        // Repeated resubscription while idle must not accumulate dropped
        // subscriptions until the next emission.
        self.hierarchy.prune();
        self.hierarchy.subscribers.push(Subscriber {
            queue: Arc::downgrade(&queue),
            wake,
        });
        (
            state,
            HierarchySubscription {
                queue,
                wake: signal,
            },
        )
    }

    /// Advances the hierarchy sequence for a committed change and delivers
    /// the event. The event is built only when a subscriber exists.
    pub(super) fn emit_hierarchy(
        &mut self,
        build: impl FnOnce(&Self) -> HierarchyEvent,
    ) {
        self.hierarchy.seq = self
            .hierarchy
            .seq
            .checked_add(1)
            .expect("hierarchy sequence exhausted");
        if !self.hierarchy.has_subscribers() {
            return;
        }
        let envelope = HierarchyEnvelope {
            stream: StreamId::new(self.runtime_id),
            seq: self.hierarchy.seq,
            event: build(self),
        };
        #[cfg(test)]
        {
            self.hierarchy.built += 1;
        }
        self.hierarchy.publish(&envelope);
    }
}

impl Session {
    pub(super) fn info(&self) -> SessionInfo {
        SessionInfo {
            id: self.id,
            custom_name: self.custom_name.clone(),
            automatic_name: self.automatic_name.clone(),
        }
    }
}

impl Workspace {
    pub(super) fn info(&self) -> WorkspaceInfo {
        WorkspaceInfo {
            id: self.id,
            custom_name: self.custom_name.clone(),
            automatic_name: self.automatic_name.clone(),
        }
    }
}

impl Tab {
    pub(super) fn info(&self) -> TabInfo {
        TabInfo {
            id: self.id,
            pane_id: self.pane_id,
            terminal_id: self.terminal_id,
            custom_name: self.custom_name.clone(),
            fallback_name: self.fallback_name.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::tests::command;
    use huterm_protocol::{ApplyOutcome, SessionId, TabId, WorkspaceId};

    /// Drains the real subscription into `state`, then compares it with a
    /// fresh snapshot.
    fn converge(
        mux: &mut Mux,
        state: &mut HierarchyState,
        subscription: &HierarchySubscription,
    ) {
        let pending = mux.hierarchy_seq() - state.seq();
        assert_eq!(subscription.take_wake(), pending > 0, "wake signal");
        let mut applied = 0;
        loop {
            match subscription.try_recv() {
                Ok(envelope) => {
                    let outcome = state.apply(envelope.clone());
                    assert!(
                        matches!(outcome, ApplyOutcome::Applied(_)),
                        "{envelope:?} gave {outcome:?}"
                    );
                    applied += 1;
                }
                Err(HierarchyRecvError::Empty) => break,
                Err(error) => panic!("unexpected {error}"),
            }
        }
        assert_eq!(applied, pending);
        let (fresh, _) = mux.subscribe_hierarchy();
        assert_eq!(*state, fresh);
        assert_eq!(state.digest(), fresh.digest());
    }

    fn open(mux: &mut Mux, workspace: WorkspaceId) -> TabId {
        mux.open_tab(workspace, &command("read value"))
            .unwrap()
            .tab
            .id
    }

    #[test]
    fn reducer_follows_every_emitting_operation() {
        let mut mux = Mux::default();
        let (mut state, subscription) = mux.subscribe_hierarchy();
        let step = |mux: &mut Mux, state: &mut HierarchyState| {
            converge(mux, state, &subscription);
        };
        let first = mux.create_session(None).unwrap();
        step(&mut mux, &mut state);
        let second = mux.create_session(Some("named")).unwrap();
        step(&mut mux, &mut state);
        let alpha = mux.create_workspace(first, None).unwrap();
        step(&mut mux, &mut state);
        let beta = mux.create_workspace(first, Some("beta")).unwrap();
        step(&mut mux, &mut state);
        let gamma = mux.create_workspace(second, None).unwrap();
        step(&mut mux, &mut state);
        let one = open(&mut mux, alpha);
        step(&mut mux, &mut state);
        let two = open(&mut mux, alpha);
        step(&mut mux, &mut state);
        let three = open(&mut mux, alpha);
        step(&mut mux, &mut state);
        let four = open(&mut mux, beta);
        step(&mut mux, &mut state);

        mux.rename_tab(one, Some("one")).unwrap();
        step(&mut mux, &mut state);
        mux.rename_workspace(alpha, Some("alpha")).unwrap();
        step(&mut mux, &mut state);
        mux.rename_session(first, Some("main")).unwrap();
        step(&mut mux, &mut state);
        mux.rename_tab(one, None).unwrap();
        step(&mut mux, &mut state);
        mux.rename_workspace(beta, None).unwrap();
        step(&mut mux, &mut state);
        mux.rename_session(second, None).unwrap();
        step(&mut mux, &mut state);

        // [one, two, three] -> [three, one, two] -> [one, two, three].
        mux.reorder_tab(alpha, three, Some(one)).unwrap();
        step(&mut mux, &mut state);
        mux.reorder_tab(alpha, three, None).unwrap();
        step(&mut mux, &mut state);
        mux.reorder_tab(alpha, one, Some(three)).unwrap();
        step(&mut mux, &mut state);
        assert_eq!(state.workspace_tabs(alpha).unwrap(), [two, one, three]);
        mux.move_tab(alpha, two, beta, Some(four)).unwrap();
        step(&mut mux, &mut state);
        mux.move_tab(beta, two, gamma, None).unwrap();
        step(&mut mux, &mut state);
        mux.move_workspace(first, beta, second, Some(gamma))
            .unwrap();
        step(&mut mux, &mut state);
        mux.move_workspace(second, beta, second, None).unwrap();
        step(&mut mux, &mut state);
        assert_eq!(state.session_workspaces(second).unwrap(), [gamma, beta]);

        mux.close_tab(alpha, one).unwrap();
        step(&mut mux, &mut state);
        mux.close_workspace(beta).unwrap();
        step(&mut mux, &mut state);
        assert!(state.tab(four).is_none());

        // A failed first spawn rolls back by closing the new session.
        let rollback = mux.create_session(None).unwrap();
        mux.create_workspace(rollback, None).unwrap();
        mux.close_session(rollback).unwrap();
        step(&mut mux, &mut state);

        mux.close_session(first).unwrap();
        step(&mut mux, &mut state);
        assert!(state.tab(three).is_none());
        mux.shutdown().unwrap();
        step(&mut mux, &mut state);
        assert!(state.sessions().is_empty());
        assert_eq!(mux.terminal_count(), 0);
    }

    #[test]
    fn rejected_and_unchanged_operations_emit_nothing() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let tab = open(&mut mux, workspace);
        let (_, subscription) = mux.subscribe_hierarchy();
        let seq = mux.hierarchy_seq();
        let stale_session = SessionId::in_runtime(mux.runtime_id(), u64::MAX);
        let stale_workspace =
            WorkspaceId::in_runtime(mux.runtime_id(), u64::MAX);
        let stale_tab = TabId::in_runtime(mux.runtime_id(), u64::MAX);
        let mut failing = command("");
        failing.program = "/huterm-nonexistent-shell".into();

        assert!(mux.create_session(Some(" ")).is_err());
        assert!(mux.create_workspace(stale_session, None).is_err());
        assert!(
            mux.create_workspace(SessionId::new(session.get()), None)
                .is_err()
        );
        assert!(mux.rename_session(session, Some("")).is_err());
        assert!(mux.rename_workspace(stale_workspace, Some("x")).is_err());
        assert!(mux.rename_tab(stale_tab, Some("x")).is_err());
        assert!(
            mux.open_tab(stale_workspace, &command("read value"))
                .is_err()
        );
        assert!(mux.open_tab(workspace, &failing).is_err());
        assert!(mux.reorder_tab(workspace, tab, Some(stale_tab)).is_err());
        assert!(
            mux.move_workspace(session, stale_workspace, session, None)
                .is_err()
        );
        assert!(mux.close_tab(workspace, stale_tab).is_err());
        assert!(mux.close_workspace(stale_workspace).is_err());
        assert!(mux.close_session(stale_session).is_err());
        // Unchanged positions and names, which must not make pending close
        // assessments stale either.
        let revision = mux.revision;
        mux.reorder_tab(workspace, tab, None).unwrap();
        mux.reorder_tab(workspace, tab, Some(tab)).unwrap();
        mux.move_workspace(session, workspace, session, None)
            .unwrap();
        mux.rename_session(session, None).unwrap();
        mux.rename_workspace(workspace, None).unwrap();
        mux.rename_tab(tab, None).unwrap();
        assert_eq!(mux.revision, revision);
        mux.rename_tab(tab, Some("same")).unwrap();
        assert_eq!(mux.hierarchy_seq(), seq + 1);
        assert!(subscription.try_recv().is_ok());
        assert!(subscription.take_wake());
        let revision = mux.revision;
        mux.rename_tab(tab, Some("same")).unwrap();

        assert_eq!(mux.revision, revision);
        assert_eq!(mux.hierarchy_seq(), seq + 1);
        assert_eq!(subscription.try_recv(), Err(HierarchyRecvError::Empty));
        assert!(!subscription.take_wake());

        mux.shutdown().unwrap();
        assert_eq!(mux.hierarchy_seq(), seq + 2);
        assert!(subscription.try_recv().is_ok());
        assert!(subscription.take_wake());
        // Shutting down an empty runtime changes nothing.
        mux.shutdown().unwrap();
        assert_eq!(mux.hierarchy_seq(), seq + 2);
        assert_eq!(subscription.try_recv(), Err(HierarchyRecvError::Empty));
        assert!(!subscription.take_wake());
    }

    #[test]
    fn overflow_lags_and_resubscription_converges() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let (_, first) = mux.subscribe_hierarchy();
        mux.rename_session(session, Some("taken")).unwrap();
        let taken = first.try_recv().unwrap();
        mux.rename_session(session, Some("queued")).unwrap();
        let (mut state, second) = mux.subscribe_hierarchy();
        // Events from before the snapshot are already reflected in it.
        assert_eq!(state.apply(taken), ApplyOutcome::Stale);
        assert_eq!(state.apply(first.try_recv().unwrap()), ApplyOutcome::Stale);
        drop(first);

        for index in 0..=HIERARCHY_QUEUE_CAPACITY {
            mux.rename_session(session, Some(&format!("name {index}")))
                .unwrap();
        }
        assert!(second.take_wake());
        assert_eq!(second.try_recv(), Err(HierarchyRecvError::Lagged));
        mux.create_workspace(session, None).unwrap();
        assert!(!second.take_wake());
        assert_eq!(second.try_recv(), Err(HierarchyRecvError::Lagged));

        let (recovered, third) = mux.subscribe_hierarchy();
        drop(second);
        state = recovered;
        let workspace = mux.create_workspace(session, None).unwrap();
        mux.rename_workspace(workspace, Some("after")).unwrap();
        converge(&mut mux, &mut state, &third);
        assert_eq!(
            state.session(session).unwrap().display_name(),
            format!("name {HIERARCHY_QUEUE_CAPACITY}")
        );
    }

    #[test]
    fn emission_without_subscribers_builds_no_events() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        mux.rename_workspace(workspace, Some("quiet")).unwrap();
        assert_eq!(mux.hierarchy_seq(), 3);
        assert_eq!(mux.hierarchy.built, 0);

        let (_, dropped) = mux.subscribe_hierarchy();
        drop(dropped);
        mux.close_workspace(workspace).unwrap();
        assert_eq!(mux.hierarchy_seq(), 4);
        assert_eq!(mux.hierarchy.built, 0);
        assert!(mux.hierarchy.subscribers.is_empty());

        // Resubscribing while idle replaces dropped subscriptions instead of
        // accumulating them until the next emission.
        for _ in 0..3 {
            drop(mux.subscribe_hierarchy());
        }
        let (_, subscription) = mux.subscribe_hierarchy();
        assert_eq!(mux.hierarchy.subscribers.len(), 1);
        mux.close_session(session).unwrap();
        assert_eq!(mux.hierarchy.built, 1);
        drop(mux);
        assert!(subscription.try_recv().is_ok());
        assert_eq!(subscription.try_recv(), Err(HierarchyRecvError::Closed));
    }
}
