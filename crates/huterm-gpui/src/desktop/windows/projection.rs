//! The client's projection of runtime structure: one `HierarchyState` fed by
//! a core subscription, the one-shot waiters completions park on until the
//! projection reaches their committed sequence, and the level-triggered rules
//! that bring window views in line with projection state.
//!
//! Everything here is free of GPUI so the rules can be tested without a
//! window; `windows.rs` supplies the application glue.

use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

use gpui::{Task, WindowId};
use huterm_core::{HierarchyRecvError, HierarchySubscription};
use huterm_protocol::{
    ApplyOutcome, HierarchyState, TabId, Touched, WorkspaceId,
};

use super::model::WindowModel;

/// How a sequence waiter resolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Resolution {
    /// The projection applied the awaited sequence.
    Ready,
    /// Teardown or a stream change ended the wait; the projection may never
    /// reach the sequence.
    Cancelled,
}

/// One-shot waiters keyed by the hierarchy sequence they need, released in
/// sequence order. Each resolves exactly once: resolution removes the waiter
/// before sending, and dropping the set cancels whatever remains.
#[derive(Debug, Default)]
pub(super) struct SequenceWaiters {
    next: u64,
    waiting: BTreeMap<(u64, u64), async_channel::Sender<Resolution>>,
}

impl SequenceWaiters {
    /// Registers a waiter for `seq`.
    pub(super) fn register(
        &mut self,
        seq: u64,
    ) -> async_channel::Receiver<Resolution> {
        let (sender, receiver) = async_channel::bounded(1);
        self.next = self.next.wrapping_add(1);
        self.waiting.insert((seq, self.next), sender);
        receiver
    }

    /// Resolves every waiter whose sequence is at most `applied` as ready.
    pub(super) fn release(&mut self, applied: u64) {
        while let Some(entry) = self.waiting.first_entry() {
            if entry.key().0 > applied {
                break;
            }
            let _ = entry.remove().try_send(Resolution::Ready);
        }
    }

    /// Resolves every remaining waiter as cancelled.
    pub(super) fn cancel_all(&mut self) {
        while let Some((_, sender)) = self.waiting.pop_first() {
            let _ = sender.try_send(Resolution::Cancelled);
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.waiting.len()
    }
}

impl Drop for SequenceWaiters {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

/// The outcome of draining the subscription.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Drained {
    /// Every queued event applied or was stale; the summary names what the
    /// applied ones changed.
    Current(Touched),
    /// Events were lost or the stream diverged; replace the state from a
    /// fresh snapshot before reconciling anything.
    Resync,
    /// Application teardown has committed: nothing applied, and every
    /// waiter was cancelled.
    Frozen,
}

/// The outcome of installing a resync snapshot.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Install {
    /// The snapshot and its drained subscription are current; reconcile
    /// everything.
    Current,
    /// The new subscription lagged again; resubscribe without reconciling.
    Lagged,
    /// Application teardown has committed; nothing was installed, and every
    /// waiter was cancelled.
    Frozen,
}

/// What a completion learns when it asks for its committed sequence.
#[derive(Debug)]
pub(super) enum Wait {
    /// The projection already holds the sequence.
    Ready,
    /// Teardown has committed; the wait is cancelled at once.
    Cancelled,
    /// The projection is behind; the receiver resolves exactly once.
    Pending(async_channel::Receiver<Resolution>),
}

/// The projection, its subscription, and the waiters parked on it.
pub(super) struct Projection {
    state: HierarchyState,
    subscription: Rc<HierarchySubscription>,
    waiters: SequenceWaiters,
    /// A resubscription worker is running; drains defer to it.
    resyncing: bool,
    /// The application task woken by the current subscription. Replacing
    /// it drops, and so cancels, the previous subscription's task.
    drain_task: Option<Task<()>>,
}

impl Projection {
    pub(super) fn new(
        state: HierarchyState,
        subscription: HierarchySubscription,
    ) -> Self {
        Self {
            state,
            subscription: Rc::new(subscription),
            waiters: SequenceWaiters::default(),
            resyncing: false,
            drain_task: None,
        }
    }

    pub(super) fn state(&self) -> &HierarchyState {
        &self.state
    }

    /// The subscription the drain task waits on.
    pub(super) fn subscription(&self) -> Rc<HierarchySubscription> {
        Rc::clone(&self.subscription)
    }

    pub(super) fn set_drain_task(&mut self, task: Task<()>) {
        self.drain_task = Some(task);
    }

    /// Applies every queued event unless application teardown has
    /// committed. From then on the projection keeps its last pre-teardown
    /// state, so the `Reset` that teardown emits never empties it and labels
    /// keep their names while Quit tears down.
    pub(super) fn sync(&mut self, terminating: bool) -> Drained {
        if terminating {
            self.freeze();
            Drained::Frozen
        } else {
            self.drain()
        }
    }

    /// Stops following the stream after teardown: cancels every waiter and
    /// any pending resubscription.
    fn freeze(&mut self) {
        self.waiters.cancel_all();
        self.resyncing = false;
    }

    /// Applies every queued event in order.
    fn drain(&mut self) -> Drained {
        let mut touched = Touched::default();
        loop {
            match self.subscription.try_recv() {
                Ok(envelope) => match self.state.apply(envelope) {
                    ApplyOutcome::Applied(changed) => touched.merge(changed),
                    ApplyOutcome::Stale => {}
                    ApplyOutcome::ResyncRequired(_) => return Drained::Resync,
                },
                Err(HierarchyRecvError::Lagged) => return Drained::Resync,
                // A dropped Mux sends nothing more; what applied stands.
                Err(HierarchyRecvError::Empty | HierarchyRecvError::Closed) => {
                    return Drained::Current(touched);
                }
            }
        }
    }

    /// Marks a resubscription as started; false when one already runs.
    pub(super) fn begin_resync(&mut self) -> bool {
        !std::mem::replace(&mut self.resyncing, true)
    }

    /// Replaces the state and subscription together, then drains the new
    /// subscription so events committed after the snapshot apply before
    /// anything reconciles. After teardown it installs nothing.
    pub(super) fn install(
        &mut self,
        state: HierarchyState,
        subscription: HierarchySubscription,
        terminating: bool,
    ) -> Install {
        if terminating {
            self.freeze();
            return Install::Frozen;
        }
        self.state = state;
        self.subscription = Rc::new(subscription);
        self.drain_task = None;
        if matches!(self.drain(), Drained::Current(_)) {
            self.resyncing = false;
            Install::Current
        } else {
            Install::Lagged
        }
    }

    /// Whether a resubscription is still pending.
    pub(super) fn resyncing(&self) -> bool {
        self.resyncing
    }

    /// Asks for `seq`: ready when applied, cancelled after teardown, and a
    /// one-shot waiter otherwise.
    pub(super) fn wait_for(&mut self, seq: u64, terminating: bool) -> Wait {
        if terminating {
            Wait::Cancelled
        } else if self.state.seq() >= seq {
            Wait::Ready
        } else {
            Wait::Pending(self.waiters.register(seq))
        }
    }

    /// Releases the waiters the projection satisfies, or cancels all of
    /// them once teardown has committed.
    pub(super) fn settle_waiters(&mut self, terminating: bool) {
        if terminating {
            self.waiters.cancel_all();
        } else {
            self.waiters.release(self.state.seq());
        }
    }

    #[cfg(test)]
    pub(super) fn waiting(&self) -> usize {
        self.waiters.len()
    }
}

/// The window's installed order: the projection's order for its workspace,
/// filtered to installed tabs, then any installed tab the projection does
/// not hold, in current order. Always a permutation of `installed`.
pub(super) fn installed_order(
    projected: &[TabId],
    installed: &[TabId],
) -> Vec<TabId> {
    let mut order: Vec<TabId> = projected
        .iter()
        .copied()
        .filter(|tab| installed.contains(tab))
        .collect();
    order.extend(installed.iter().filter(|tab| !projected.contains(tab)));
    order
}

/// How one window's installed views differ from the projection.
#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct Membership {
    /// Views whose tabs the projection has applied and no longer holds.
    /// Empty while the window is busy: its own completion removes them.
    pub(super) remove: Vec<TabId>,
    /// A projected tab with no view while the window has no spawn in flight:
    /// the unsupported external-open flow.
    pub(super) unexpected: Option<TabId>,
}

/// Decides membership for one window. `installed` pairs each view's tab
/// with the sequence it committed at; a view the projection has not caught
/// up with is never removed. A busy window keeps its views: its structural
/// completion drops them and refocuses in one update, then reconciles. Nothing
/// changes once teardown owns removal.
pub(super) fn membership(
    state: &HierarchyState,
    workspace: Option<WorkspaceId>,
    installed: &[(TabId, u64)],
    busy: bool,
    terminating: bool,
) -> Membership {
    if terminating {
        return Membership::default();
    }
    let remove = if busy {
        Vec::new()
    } else {
        installed
            .iter()
            .filter(|(tab, committed)| {
                state.seq() >= *committed && state.tab(*tab).is_none()
            })
            .map(|(tab, _)| *tab)
            .collect()
    };
    let unexpected = if busy {
        None
    } else {
        workspace
            .and_then(|workspace| state.workspace_tabs(workspace))
            .and_then(|projected| {
                projected
                    .iter()
                    .copied()
                    .find(|tab| !installed.iter().any(|(id, _)| id == tab))
            })
    };
    Membership { remove, unexpected }
}

/// Windows a summary concerns: those showing a touched workspace or tab.
pub(super) fn windows_to_visit(
    model: &WindowModel,
    touched: &Touched,
) -> Vec<WindowId> {
    if touched.is_empty() {
        return Vec::new();
    }
    model
        .records()
        .filter(|record| {
            touched.is_everything()
                || record.workspace.is_some_and(|workspace| {
                    touched.contains_workspace(workspace)
                })
                || record
                    .tabs
                    .iter()
                    .any(|entry| touched.contains_tab(entry.id))
        })
        .map(|record| record.id)
        .collect()
}

/// One window's views and model entries as reconcile changes them.
pub(super) trait ReconcileTarget {
    fn projection(&self) -> &HierarchyState;
    fn workspace(&self) -> Option<WorkspaceId>;
    /// Installed tabs in display order with their committed sequences.
    fn installed(&self) -> Vec<(TabId, u64)>;
    /// Whether the window has a structural operation, such as its own
    /// spawn, in flight.
    fn busy(&self) -> bool;
    /// Drops views and model entries without closing anything.
    fn drop_tabs(&mut self, tabs: &[TabId]);
    /// Applies an installed order to views and model entries; returns
    /// whether anything moved.
    fn apply_order(&mut self, order: &[TabId]) -> bool;
    /// The tab's current display title.
    fn title(&self, tab: TabId) -> String;
    /// Publishes a title; returns whether it changed.
    fn publish_title(&mut self, tab: TabId, title: &str) -> bool;
}

/// What reconciling one window changed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct WindowChange {
    /// Something the window renders changed; notify it once.
    pub(super) changed: bool,
    /// A published title changed, so title consumers may need a refresh.
    pub(super) titles: bool,
}

/// Brings one window in line with the projection: membership, then order,
/// then the titles of touched tabs. Derived from state and compared before
/// every write, so running it again changes nothing.
pub(super) fn reconcile_window(
    target: &mut impl ReconcileTarget,
    touched: &Touched,
    terminating: bool,
) -> WindowChange {
    let installed = target.installed();
    let plan = membership(
        target.projection(),
        target.workspace(),
        &installed,
        target.busy(),
        terminating,
    );
    debug_assert!(
        plan.unexpected.is_none(),
        "projected tab {:?} has no view in a window with no spawn in flight",
        plan.unexpected
    );
    let mut change = WindowChange::default();
    if !plan.remove.is_empty() {
        target.drop_tabs(&plan.remove);
        change.changed = true;
    }
    let current: Vec<TabId> =
        target.installed().into_iter().map(|(tab, _)| tab).collect();
    let projected = target
        .workspace()
        .and_then(|workspace| target.projection().workspace_tabs(workspace))
        .unwrap_or(&[]);
    let order = installed_order(projected, &current);
    if order != current {
        change.changed |= target.apply_order(&order);
    }
    for tab in current {
        if touched.contains_tab(tab) {
            let title = target.title(tab);
            if target.publish_title(tab, &title) {
                change.changed = true;
                change.titles = true;
            }
        }
    }
    change
}

/// Windows whose title consumers, open palettes and close confirmations,
/// read names they do not own, and the windows whose published titles
/// changed since the last refresh.
#[derive(Debug, Default)]
pub(super) struct TitleConsumers {
    hosts: HashSet<WindowId>,
    dirty: HashSet<WindowId>,
}

impl TitleConsumers {
    /// Records whether `window` shows a title consumer.
    pub(super) fn set_host(&mut self, window: WindowId, host: bool) {
        if host {
            self.hosts.insert(window);
        } else {
            self.hosts.remove(&window);
        }
    }

    pub(super) fn hosts(&self) -> Vec<WindowId> {
        self.hosts.iter().copied().collect()
    }

    /// Notes a terminal-driven title change in `window`. Returns whether the
    /// caller must schedule a refresh: only when a consumer exists and none
    /// is scheduled yet.
    pub(super) fn mark(&mut self, window: WindowId) -> bool {
        if self.hosts.is_empty() {
            return false;
        }
        let schedule = self.dirty.is_empty();
        self.dirty.insert(window);
        schedule
    }

    /// Takes the windows marked since the last refresh.
    pub(super) fn take_dirty(&mut self) -> HashSet<WindowId> {
        std::mem::take(&mut self.dirty)
    }
}

#[cfg(test)]
mod tests;
