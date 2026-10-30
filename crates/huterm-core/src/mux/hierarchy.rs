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
            // Overflow already emptied the queue; a dropped Mux ends the
            // stream instead of inviting another resubscription.
            return Err(if self.wake.is_closed() {
                HierarchyRecvError::Closed
            } else {
                HierarchyRecvError::Lagged
            });
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
            custom_name: self.custom_name.clone(),
            layout: self.layout.clone(),
            zoomed: self.zoomed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::tests::command;
    use huterm_protocol::{
        ApplyOutcome, NodeId, PaneId, PaneNode, SessionId, SplitDirection,
        SplitId, TabId, WorkspaceId,
    };

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
        // The projection records each tab as core's layout changes do.
        for tab in mux.workspaces.values().flat_map(|w| &w.tabs) {
            assert_eq!(state.tab_snapshot(tab.id), mux.snapshot(tab.id));
        }
    }

    fn open(mux: &mut Mux, workspace: WorkspaceId) -> TabId {
        mux.open_tab(workspace, &command("read value"))
            .unwrap()
            .tab
            .id
    }

    fn split(
        mux: &mut Mux,
        workspace: WorkspaceId,
        tab: TabId,
        pane: PaneId,
        direction: SplitDirection,
    ) -> PaneId {
        mux.split_pane(workspace, tab, pane, direction, &command("read value"))
            .unwrap()
            .pane
            .id
    }

    /// The root split's identity and children.
    fn root_split(mux: &Mux, tab: TabId) -> (SplitId, Vec<NodeId>) {
        let PaneNode::Split(split) = mux.tab(tab).unwrap().layout().root()
        else {
            panic!("tab {tab:?} is not split");
        };
        (
            split.id,
            split.children.iter().map(|child| child.node.id()).collect(),
        )
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "walk every emitting operation through one evolving runtime"
    )]
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

        // Tab one: [left | right over below], resized, zoomed, then a close
        // that clears the zoom and collapses the nested split.
        let left = mux.tab(one).unwrap().first_pane().id;
        let right = split(&mut mux, alpha, one, left, SplitDirection::Right);
        step(&mut mux, &mut state);
        let below = split(&mut mux, alpha, one, right, SplitDirection::Down);
        step(&mut mux, &mut state);
        mux.rename_pane(right, Some("right")).unwrap();
        step(&mut mux, &mut state);
        assert_eq!(
            state
                .tab(one)
                .unwrap()
                .layout
                .pane(right)
                .unwrap()
                .custom_name
                .as_deref(),
            Some("right")
        );
        mux.rename_pane(right, None).unwrap();
        step(&mut mux, &mut state);
        mux.rename_pane(below, Some("below")).unwrap();
        step(&mut mux, &mut state);
        let (root, children) = root_split(&mux, one);
        mux.resize_split(alpha, one, root, &children, &[300_000, 700_000])
            .unwrap();
        step(&mut mux, &mut state);
        mux.set_pane_zoom(alpha, one, Some(below)).unwrap();
        step(&mut mux, &mut state);
        assert_eq!(state.tab(one).unwrap().zoomed, Some(below));
        mux.close_pane(alpha, one, below).unwrap();
        step(&mut mux, &mut state);
        assert_eq!(state.tab(one).unwrap().zoomed, None);
        assert_eq!(state.tab(one).unwrap().layout.pane_count(), 2);
        // Splitting clears a shared zoom.
        mux.set_pane_zoom(alpha, one, Some(right)).unwrap();
        step(&mut mux, &mut state);
        split(&mut mux, alpha, one, left, SplitDirection::Left);
        step(&mut mux, &mut state);
        assert_eq!(state.tab(one).unwrap().zoomed, None);
        mux.set_pane_zoom(alpha, one, Some(right)).unwrap();
        step(&mut mux, &mut state);
        mux.set_pane_zoom(alpha, one, None).unwrap();
        step(&mut mux, &mut state);
        // Closing every sibling of the zoomed pane clears the zoom with the
        // last close, which leaves one pane.
        mux.set_pane_zoom(alpha, one, Some(right)).unwrap();
        step(&mut mux, &mut state);
        let others: Vec<_> = mux
            .tab(one)
            .unwrap()
            .layout()
            .panes()
            .into_iter()
            .map(|pane| pane.id)
            .filter(|pane| *pane != right)
            .collect();
        mux.close_pane(alpha, one, others[0]).unwrap();
        step(&mut mux, &mut state);
        assert_eq!(state.tab(one).unwrap().zoomed, Some(right));
        mux.close_pane(alpha, one, others[1]).unwrap();
        step(&mut mux, &mut state);
        assert_eq!(state.tab(one).unwrap().zoomed, None);
        assert_eq!(mux.tab(one).unwrap().zoomed(), None);

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
        // Closing a tab's only pane closes the tab.
        let only = mux.tab(four).unwrap().first_pane().id;
        mux.close_pane(beta, four, only).unwrap();
        step(&mut mux, &mut state);
        assert!(state.tab(four).is_none());
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
        assert_eq!(state.sessions(), []);
        assert_eq!(mux.terminal_count(), 0);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "walk every pane move, its undo, and its redo in one runtime"
    )]
    fn reducer_follows_every_pane_move_and_its_undo_and_redo() {
        use crate::mux::{
            Beside, MoveDestination, MuxError, PaneRef,
            RestoreDirection::{Redo, Undo},
        };
        use huterm_protocol::LayoutChange;
        let mut mux = Mux::default();
        let (mut state, subscription) = mux.subscribe_hierarchy();
        let step = |mux: &mut Mux, state: &mut HierarchyState| {
            converge(mux, state, &subscription);
        };
        // Each move, then its undo and redo, as separate events.
        let moved =
            |mux: &mut Mux,
             state: &mut HierarchyState,
             change: Result<LayoutChange, MuxError>| {
                let change = change.unwrap();
                converge(mux, state, &subscription);
                for direction in [Undo, Redo] {
                    mux.restore_tabs(&change, direction).unwrap();
                    converge(mux, state, &subscription);
                }
                change
            };
        let first = mux.create_session(None).unwrap();
        let alpha = mux.create_workspace(first, None).unwrap();
        let beta = mux.create_workspace(first, None).unwrap();
        let second = mux.create_session(None).unwrap();
        let gamma = mux.create_workspace(second, None).unwrap();
        let one = open(&mut mux, alpha);
        let two = open(&mut mux, alpha);
        let a = mux.tab(one).unwrap().first_pane().id;
        let b = split(&mut mux, alpha, one, a, SplitDirection::Right);
        let c = split(&mut mux, alpha, one, b, SplitDirection::Down);
        step(&mut mux, &mut state);
        // Names ride along, and move between a pane and its tab.
        mux.rename_pane(a, Some("a")).unwrap();
        mux.rename_pane(c, Some("c")).unwrap();
        mux.rename_tab(two, Some("two")).unwrap();
        step(&mut mux, &mut state);
        let at = |mux: &Mux, pane: PaneId| {
            let target = mux.select_pane(pane).unwrap();
            PaneRef {
                workspace: target.workspace.unwrap(),
                tab: target.tab.unwrap(),
                pane,
            }
        };
        let beside = |mux: &Mux, pane: PaneId, side: SplitDirection| {
            MoveDestination::Beside(Beside {
                target: at(mux, pane),
                side,
            })
        };

        let change = mux.swap_panes(alpha, one, a, c);
        moved(&mut mux, &mut state, change);
        let change =
            mux.move_pane(at(&mux, c), beside(&mux, b, SplitDirection::Left));
        moved(&mut mux, &mut state, change);
        let lone = mux.tab(two).unwrap().first_pane().id;
        let change = mux
            .move_pane(at(&mux, b), beside(&mux, lone, SplitDirection::Right));
        moved(&mut mux, &mut state, change);
        assert_eq!(state.pane_tab(b), Some(two));
        let change = mux.move_pane(
            at(&mux, c),
            MoveDestination::NewTab {
                workspace: alpha,
                before: Some(one),
            },
        );
        let three = moved(&mut mux, &mut state, change).after[1].tab;
        assert_eq!(state.workspace_tabs(alpha).unwrap(), [three, one, two]);
        // Undo reopens the dropped tab with its own identity.
        let change = mux.move_tab_to_pane(
            alpha,
            two,
            Beside {
                target: at(&mux, a),
                side: SplitDirection::Down,
            },
        );
        moved(&mut mux, &mut state, change);
        assert!(state.tab(two).is_none());
        // A lone pane's extraction into another workspace moves its tab.
        let change = mux.move_pane(
            at(&mux, c),
            MoveDestination::NewTab {
                workspace: beta,
                before: None,
            },
        );
        moved(&mut mux, &mut state, change);
        assert_eq!(state.tab_workspace(three), Some(beta));
        let change =
            mux.move_pane(at(&mux, lone), beside(&mux, c, SplitDirection::Up));
        moved(&mut mux, &mut state, change);
        // Across sessions: pane a opens a tab in gamma, then pane b, tab
        // one's last pane, takes its whole tab there.
        for pane in [a, b] {
            let change = mux.move_pane(
                at(&mux, pane),
                MoveDestination::NewTab {
                    workspace: gamma,
                    before: None,
                },
            );
            moved(&mut mux, &mut state, change);
        }
        assert_eq!(state.tab_workspace(one), Some(gamma));
        assert_eq!(state.workspace_tabs(gamma).unwrap().len(), 2);
        mux.shutdown().unwrap();
        step(&mut mux, &mut state);
    }

    #[test]
    fn reducer_follows_every_arrangement_and_its_undo_and_redo() {
        use crate::mux::RestoreDirection::{Redo, Undo};
        use huterm_protocol::BUILT_IN_LAYOUTS;
        let mut mux = Mux::default();
        let (mut state, subscription) = mux.subscribe_hierarchy();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let tab = open(&mut mux, workspace);
        let a = mux.tab(tab).unwrap().first_pane().id;
        let b = split(&mut mux, workspace, tab, a, SplitDirection::Right);
        split(&mut mux, workspace, tab, b, SplitDirection::Down);
        mux.set_pane_zoom(workspace, tab, Some(b)).unwrap();
        converge(&mut mux, &mut state, &subscription);
        let names = BUILT_IN_LAYOUTS.iter().map(|layout| layout.name.as_str());
        // Tiled is balanced already; main left's 60/40 is not.
        for name in names.chain(["main_left", "balance"]) {
            let current = mux.tab(tab).unwrap().layout().clone();
            let panes: Vec<PaneId> =
                current.panes().iter().map(|pane| pane.id).collect();
            let arrangement = match huterm_protocol::NamedLayout::built_in(name)
            {
                Some(layout) => layout.template.expand(&panes).unwrap(),
                None => current.balanced(),
            };
            let change = mux
                .arrange_panes(workspace, tab, &current, &arrangement)
                .unwrap();
            assert!(!change.is_empty(), "{name} changed nothing");
            converge(&mut mux, &mut state, &subscription);
            assert_eq!(
                state.tab(tab).unwrap().layout,
                *mux.tab(tab).unwrap().layout()
            );
            // The change records what undo needs, a weights-only balance
            // included.
            for direction in [Undo, Redo] {
                mux.restore_tabs(&change, direction).unwrap();
                converge(&mut mux, &mut state, &subscription);
            }
        }
        mux.shutdown().unwrap();
        converge(&mut mux, &mut state, &subscription);
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
        let pane = mux.tab(tab).unwrap().first_pane().id;
        let stale_pane = PaneId::in_runtime(mux.runtime_id(), u64::MAX);
        assert!(matches!(
            mux.rename_pane(stale_pane, Some("x")),
            Err(crate::MuxError::UnknownPane(_))
        ));
        assert!(matches!(
            mux.rename_pane(pane, Some(" ")),
            Err(crate::MuxError::InvalidName)
        ));
        assert!(matches!(
            mux.rename_pane(PaneId::new(pane.get()), Some("x")),
            Err(crate::MuxError::ForeignRuntime(_))
        ));
        let right = SplitDirection::Right;
        assert!(
            mux.split_pane(workspace, tab, stale_pane, right, &command(""))
                .is_err()
        );
        // A failed spawn changes no structure and leaks no terminal.
        let terminals = mux.terminal_count();
        assert!(
            mux.split_pane(workspace, tab, pane, right, &failing)
                .is_err()
        );
        assert_eq!(mux.terminal_count(), terminals);
        assert_eq!(mux.tab(tab).unwrap().layout().pane_count(), 1);
        assert!(mux.close_pane(workspace, stale_tab, pane).is_err());
        assert!(mux.close_pane(workspace, tab, stale_pane).is_err());
        // A lone pane cannot be zoomed.
        assert!(mux.set_pane_zoom(workspace, tab, Some(pane)).is_err());
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
        mux.rename_pane(pane, None).unwrap();
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
    fn resizes_and_shared_zoom_emit_but_leave_close_tickets_valid() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let tab = open(&mut mux, workspace);
        let left = mux.tab(tab).unwrap().first_pane().id;
        let right =
            split(&mut mux, workspace, tab, left, SplitDirection::Right);
        let (root, children) = root_split(&mux, tab);
        let (revision, seq) = (mux.revision, mux.hierarchy_seq());

        mux.resize_split(workspace, tab, root, &children, &[400_000, 600_000])
            .unwrap();
        assert_eq!((mux.revision, mux.hierarchy_seq()), (revision, seq + 1));
        // The same weights again change nothing.
        mux.resize_split(workspace, tab, root, &children, &[400_000, 600_000])
            .unwrap();
        assert_eq!(mux.hierarchy_seq(), seq + 1);
        mux.set_pane_zoom(workspace, tab, Some(right)).unwrap();
        assert_eq!((mux.revision, mux.hierarchy_seq()), (revision, seq + 2));
        mux.set_pane_zoom(workspace, tab, Some(right)).unwrap();
        assert_eq!(mux.hierarchy_seq(), seq + 2);

        // A refused split changes nothing, not even the close revision.
        mux.resize_split(workspace, tab, root, &children, &[1, 999_999])
            .unwrap();
        let (seq, terminals) = (mux.hierarchy_seq(), mux.terminal_count());
        assert!(matches!(
            mux.split_pane(
                workspace,
                tab,
                left,
                SplitDirection::Right,
                &command("read value"),
            ),
            Err(crate::MuxError::PaneTooSmall(pane)) if pane == left
        ));
        assert_eq!(mux.revision, revision);
        assert_eq!(mux.hierarchy_seq(), seq);
        assert_eq!(mux.terminal_count(), terminals);

        // Splits and pane closes change what a close covers.
        let below =
            split(&mut mux, workspace, tab, right, SplitDirection::Down);
        assert_ne!(mux.revision, revision);
        let revision = mux.revision;
        mux.close_pane(workspace, tab, below).unwrap();
        assert_ne!(mux.revision, revision);
        mux.shutdown().unwrap();
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
        state = recovered;
        let workspace = mux.create_workspace(session, None).unwrap();
        mux.rename_workspace(workspace, Some("after")).unwrap();
        converge(&mut mux, &mut state, &third);
        assert_eq!(
            state.session(session).unwrap().display_name(),
            format!("name {HIERARCHY_QUEUE_CAPACITY}")
        );
        // A lagged subscription reports the end of the stream once the Mux
        // is gone, so its client stops resubscribing.
        drop(mux);
        assert_eq!(second.try_recv(), Err(HierarchyRecvError::Closed));
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
