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
    ApplyOutcome, HierarchyEvent, HierarchyState, PaneId, SessionId, TabId,
    TabInfo, Touched, WorkspaceId,
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
    /// Application teardown has committed, its `Reset` arrived, or the
    /// stream closed: nothing more applies, and every waiter was cancelled.
    /// The summary names what applied before the freeze; reconcile it.
    Frozen(Touched),
}

/// The outcome of installing a resync snapshot.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Install {
    /// The snapshot and its drained subscription are current; reconcile
    /// everything.
    Current,
    /// The new subscription lagged again; resubscribe without reconciling.
    Lagged,
    /// Application teardown has committed, its `Reset` arrived, or the
    /// stream closed; nothing more applies, and every waiter was cancelled.
    /// The summary names what changed; reconcile it.
    Frozen(Touched),
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
    /// Teardown froze the projection at its last pre-teardown state.
    frozen: bool,
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
            frozen: false,
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
        if terminating || self.frozen {
            self.freeze();
            Drained::Frozen(Touched::default())
        } else {
            self.drain()
        }
    }

    /// Stops following the stream after teardown: cancels every waiter and
    /// any pending resubscription, and applies nothing more.
    fn freeze(&mut self) {
        self.frozen = true;
        self.waiters.cancel_all();
        self.resyncing = false;
    }

    /// Applies every queued event in order. Only shutdown emits `Reset`,
    /// after teardown has committed, so a dequeued `Reset` freezes the
    /// projection instead of applying, even when the caller sampled the
    /// teardown flag before the worker set it.
    fn drain(&mut self) -> Drained {
        let mut touched = Touched::default();
        loop {
            match self.subscription.try_recv() {
                Ok(envelope)
                    if envelope.stream == self.state.stream()
                        && matches!(envelope.event, HierarchyEvent::Reset) =>
                {
                    self.freeze();
                    return Drained::Frozen(touched);
                }
                Ok(envelope) => match self.state.apply(envelope) {
                    ApplyOutcome::Applied(changed) => touched.merge(changed),
                    ApplyOutcome::Stale => {}
                    ApplyOutcome::ResyncRequired(_) => return Drained::Resync,
                },
                Err(HierarchyRecvError::Lagged) => return Drained::Resync,
                Err(HierarchyRecvError::Empty) => {
                    return Drained::Current(touched);
                }
                // A dropped Mux sends nothing more: keep what applied and
                // cancel waiters, which could otherwise park forever.
                Err(HierarchyRecvError::Closed) => {
                    self.freeze();
                    return Drained::Frozen(touched);
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
        if terminating || self.frozen {
            self.freeze();
            return Install::Frozen(Touched::default());
        }
        self.state = state;
        self.subscription = Rc::new(subscription);
        self.drain_task = None;
        match self.drain() {
            Drained::Current(_) => {
                self.resyncing = false;
                Install::Current
            }
            Drained::Resync => Install::Lagged,
            // The snapshot replaced the state before the freeze.
            Drained::Frozen(_) => Install::Frozen(Touched::everything()),
        }
    }

    /// Whether a resubscription is still pending.
    pub(super) fn resyncing(&self) -> bool {
        self.resyncing
    }

    /// Asks for `seq`: ready when applied, cancelled after teardown, and a
    /// one-shot waiter otherwise.
    pub(super) fn wait_for(&mut self, seq: u64, terminating: bool) -> Wait {
        if terminating || self.frozen {
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

/// The window's installed order. Installed tabs the projection does not
/// hold, such as a closed tab whose view a busy window keeps, stay in their
/// current slots; the projected installed tabs fill the remaining slots in
/// the projection's order for the workspace. Always a permutation of
/// `installed`, and a just-pushed tab the projection holds lands at its
/// projected position.
pub(super) fn installed_order(
    projected: &[TabId],
    installed: &[TabId],
) -> Vec<TabId> {
    let mut held = projected
        .iter()
        .copied()
        .filter(|tab| installed.contains(tab));
    installed
        .iter()
        .map(|tab| {
            if projected.contains(tab) {
                held.next().unwrap_or(*tab)
            } else {
                *tab
            }
        })
        .collect()
}

/// The views reconcile removes from one window: those whose tabs the
/// projection has applied and no longer holds, or holds in a workspace other
/// than the window's `workspace`, such as a tab moved elsewhere, or in a
/// workspace that now belongs to a session other than the window's attached
/// `session`. `installed` pairs each view's tab with the sequence it
/// committed at; a view the projection has not caught up with is never
/// removed. A busy window keeps its views: its structural completion drops
/// them and refocuses in one update, then reconciles. Nothing is removed
/// once teardown owns removal.
pub(super) fn removals(
    state: &HierarchyState,
    workspace: Option<WorkspaceId>,
    session: Option<SessionId>,
    installed: &[(TabId, u64)],
    busy: bool,
    terminating: bool,
) -> Vec<TabId> {
    if terminating || busy {
        return Vec::new();
    }
    installed
        .iter()
        .filter(|(tab, committed)| {
            state.seq() >= *committed
                && match state.tab_workspace(*tab) {
                    None => true,
                    Some(owner) => {
                        workspace.is_some_and(|own| own != owner)
                            || session.is_some_and(|own| {
                                state.workspace_session(owner) != Some(own)
                            })
                    }
                }
        })
        .map(|(tab, _)| *tab)
        .collect()
}

/// The tabs reconcile installs in one window: those the projection holds in
/// the window's `workspace` that have neither a view in `installed` nor an
/// entry in `pending`, in projected order. Nothing installs in a busy
/// window, whose own spawn holds `busy` until it pushes its view; in a
/// window without an attached `session`, or whose workspace now belongs to
/// another session; or once teardown has committed.
pub(super) fn installs(
    state: &HierarchyState,
    workspace: Option<WorkspaceId>,
    session: Option<SessionId>,
    installed: &[TabId],
    pending: &[TabId],
    busy: bool,
    terminating: bool,
) -> Vec<TabId> {
    let (Some(workspace), Some(session)) = (workspace, session) else {
        return Vec::new();
    };
    if terminating
        || busy
        || state.workspace_session(workspace) != Some(session)
    {
        return Vec::new();
    }
    state
        .workspace_tabs(workspace)
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|tab| !installed.contains(tab) && !pending.contains(tab))
        .collect()
}

/// The revoked views reconcile replaces in place in one window: those whose
/// tabs the projection still holds in the window's `workspace`, under the
/// same conditions as [`installs`], without an entry in `pending`. A
/// revoked view whose tab left is removed instead.
pub(super) fn replacements(
    state: &HierarchyState,
    workspace: Option<WorkspaceId>,
    session: Option<SessionId>,
    revoked: &[TabId],
    pending: &[TabId],
    busy: bool,
    terminating: bool,
) -> Vec<TabId> {
    let held =
        installs(state, workspace, session, &[], pending, busy, terminating);
    revoked
        .iter()
        .copied()
        .filter(|tab| held.contains(tab))
        .collect()
}

/// Whether the projection holds no tab that a window attached to `session`
/// would show in `workspace`, so a refused emptied close of that window may
/// run again.
pub(super) fn holds_no_tabs(
    state: &HierarchyState,
    workspace: Option<WorkspaceId>,
    session: Option<SessionId>,
) -> bool {
    installs(state, workspace, session, &[], &[], false, false).is_empty()
}

/// A window's installs and replacements that have started and not
/// finished, and those that failed. A failed one waits for a later batch
/// that touches the window's workspace before reconcile tries it again, so
/// a failure cannot loop.
#[derive(Debug, Default)]
pub(super) struct PendingInstalls {
    in_flight: Vec<TabId>,
    failed: Vec<TabId>,
}

impl PendingInstalls {
    fn begin(&mut self, tabs: &[TabId]) {
        self.in_flight.extend_from_slice(tabs);
    }

    /// Ends `tab`'s install; a `failed` one waits for the next batch that
    /// touches the workspace.
    pub(super) fn finish(&mut self, tab: TabId, failed: bool) {
        self.in_flight.retain(|pending| *pending != tab);
        if failed && !self.failed.contains(&tab) {
            self.failed.push(tab);
        }
    }

    /// Allows failed installs again once a batch touched `workspace`.
    fn touched(&mut self, touched: &Touched, workspace: Option<WorkspaceId>) {
        if workspace
            .is_some_and(|workspace| touched.contains_workspace(workspace))
        {
            self.failed.clear();
        }
    }

    /// The tabs reconcile must not install now.
    fn excluded(&self) -> Vec<TabId> {
        self.in_flight.iter().chain(&self.failed).copied().collect()
    }
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

/// The names a tab view last saw the projection hold for its tab. A view
/// keeps showing them after the projection drops the tab, such as while a
/// busy window's close commits, instead of falling back to the bare
/// terminal title.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct TabNames {
    custom: Option<String>,
    fallback: String,
}

impl TabNames {
    pub(super) fn from_info(info: &TabInfo) -> Self {
        Self {
            custom: info.custom_name.clone(),
            fallback: info.fallback_name().to_owned(),
        }
    }

    /// Copies the projection's names when they differ.
    pub(super) fn remember(&mut self, info: &TabInfo) {
        if self.custom != info.custom_name {
            self.custom.clone_from(&info.custom_name);
        }
        if self.fallback != info.fallback_name() {
            info.fallback_name().clone_into(&mut self.fallback);
        }
    }
}

/// The custom and fallback names a tab's label resolves from: the
/// projection's while it holds the tab, else the view's remembered names.
/// One hash lookup and no allocation.
pub(super) fn tab_names<'a>(
    state: &'a HierarchyState,
    tab: TabId,
    remembered: &'a TabNames,
) -> (Option<&'a str>, &'a str) {
    state.tab(tab).map_or(
        (remembered.custom.as_deref(), remembered.fallback.as_str()),
        |info| (info.custom_name.as_deref(), info.fallback_name()),
    )
}

/// One window's views and model entries as reconcile changes them.
pub(super) trait ReconcileTarget {
    fn projection(&self) -> &HierarchyState;
    fn workspace(&self) -> Option<WorkspaceId>;
    /// The session of the window's attachment; `None` without one.
    fn session(&self) -> Option<SessionId>;
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
    /// Remembers the names the projection holds for `tab` on its view.
    fn remember_names(&mut self, tab: TabId);
    /// The tab's current display title.
    fn title(&self, tab: TabId) -> String;
    /// Publishes a tab's title and its panes' titles; returns whether any
    /// changed.
    fn publish_title(&mut self, tab: TabId, title: &str) -> bool;
    fn pending_installs(&mut self) -> &mut PendingInstalls;
    /// Starts installing views for `tabs`, which reconcile has recorded as
    /// in flight.
    fn install(&mut self, tabs: &[TabId]);
    /// Installed tabs whose viewer reported revocation.
    fn revoked(&self) -> Vec<TabId>;
    /// Starts replacing the revoked viewers of `tabs`, which reconcile has
    /// recorded as in flight.
    fn replace(&mut self, tabs: &[TabId]);
    /// An installed tab's pane views with the sequences they were built at.
    fn installed_panes(&self, tab: TabId) -> Vec<(PaneId, u64)>;
    /// Drops pane views of `tab` without closing anything.
    fn drop_panes(&mut self, tab: TabId, panes: &[PaneId]);
    /// Moves the view of `pane` from `from` into `to`, building `to`'s
    /// view when the window has none and dropping `from`'s once it holds
    /// no pane. The view keeps its terminal view, viewer, and state.
    fn adopt_pane(&mut self, pane: PaneId, from: TabId, to: TabId);
    /// Puts `tab`'s pane views in the projected traversal order; returns
    /// whether any moved.
    fn order_panes(&mut self, tab: TabId) -> bool;
}

/// The pane views a window must move between its tabs: each view whose
/// pane the projection, at or past the view's sequence, now holds in
/// another tab of `workspace`, with the tab it leaves and the tab it
/// joins. A pane that left the workspace is not adopted; removal drops it.
pub(super) fn adoptions(
    state: &HierarchyState,
    workspace: Option<WorkspaceId>,
    installed: &[(TabId, Vec<(PaneId, u64)>)],
) -> Vec<(PaneId, TabId, TabId)> {
    let Some(workspace) = workspace else {
        return Vec::new();
    };
    installed
        .iter()
        .flat_map(|(tab, panes)| panes.iter().map(move |pane| (*tab, *pane)))
        .filter_map(|(from, (pane, committed))| {
            let to = state.pane_tab(pane)?;
            (committed <= state.seq()
                && to != from
                && state.tab_workspace(to) == Some(workspace))
            .then_some((pane, from, to))
        })
        .collect()
}

/// The pane views of an installed `tab` that reconcile removes: those whose
/// pane the projection no longer holds in the tab, once it has applied the
/// sequence each view was built at. A tab the projection no longer holds
/// in `workspace` leaves with its tab instead.
pub(super) fn pane_removals(
    state: &HierarchyState,
    workspace: Option<WorkspaceId>,
    tab: TabId,
    installed: &[(PaneId, u64)],
) -> Vec<PaneId> {
    let Some(info) = state.tab(tab).filter(|_| {
        workspace.is_some() && state.tab_workspace(tab) == workspace
    }) else {
        return Vec::new();
    };
    installed
        .iter()
        .filter(|(pane, committed)| {
            *committed <= state.seq() && info.layout.pane(*pane).is_none()
        })
        .map(|(pane, _)| *pane)
        .collect()
}

/// Whether the projection holds a pane of `tab` that has no view in
/// `installed`.
pub(super) fn missing_panes(
    state: &HierarchyState,
    tab: TabId,
    installed: &[(PaneId, u64)],
) -> bool {
    state.tab(tab).is_some_and(|info| {
        info.layout.panes().iter().any(|pane| {
            !installed.iter().any(|(installed, _)| *installed == pane.id)
        })
    })
}

/// The tab a window must activate after reconcile: its active tab when a
/// removal moved the active tab or that tab's focused pane, given each as
/// the active tab and its focused pane. The model or the tab view moved it,
/// but the view still has to show, snapshot, and focus it.
pub(super) fn reactivation(
    before: Option<(TabId, PaneId)>,
    after: Option<(TabId, PaneId)>,
) -> Option<TabId> {
    after.filter(|_| after != before).map(|(tab, _)| tab)
}

/// What reconciling one window changed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "redraws, titles, emptying and adoption are independent outcomes"
)]
pub(super) struct WindowChange {
    /// Something the window renders changed; notify it once.
    pub(super) changed: bool,
    /// A published title changed, so title consumers may need a refresh.
    pub(super) titles: bool,
    /// The removals took the window's last view.
    pub(super) emptied: bool,
    /// A pane view moved between tabs, so the window must reapply which
    /// views show even when its active tab and pane stay the same.
    pub(super) adopted: bool,
}

/// Drops the pane views whose panes left their tabs once the projection
/// caught up with each view; reports whether any went.
fn drop_left_panes(target: &mut impl ReconcileTarget) -> bool {
    let mut dropped = false;
    for (tab, _) in target.installed() {
        let gone = pane_removals(
            target.projection(),
            target.workspace(),
            tab,
            &target.installed_panes(tab),
        );
        if !gone.is_empty() {
            target.drop_panes(tab, &gone);
            dropped = true;
        }
    }
    dropped
}

/// Moves the views of panes that moved between tabs this window shows,
/// reporting whether any moved.
fn adopt_moved_panes(target: &mut impl ReconcileTarget) -> bool {
    let installed: Vec<_> = target
        .installed()
        .into_iter()
        .map(|(tab, _)| (tab, target.installed_panes(tab)))
        .collect();
    let moved = adoptions(target.projection(), target.workspace(), &installed);
    for &(pane, from, to) in &moved {
        target.adopt_pane(pane, from, to);
    }
    !moved.is_empty()
}

/// Republishes the titles of the `touched` tabs among `current` that the
/// projection still `held` in this window's workspace, reporting whether
/// any title changed.
fn publish_touched_titles(
    target: &mut impl ReconcileTarget,
    touched: &Touched,
    current: &[TabId],
    held: &[TabId],
) -> bool {
    let mut changed = false;
    for &tab in current {
        if !touched.contains_tab(tab) || !held.contains(&tab) {
            continue;
        }
        target.remember_names(tab);
        let title = target.title(tab);
        changed |= target.publish_title(tab, &title);
    }
    changed
}

/// Brings one window in line with the projection: pane order, adoptions,
/// removals, then tab order, then the titles of touched tabs, then installs
/// and replacements. Derived
/// from state and compared before every write, so running it again changes
/// nothing.
pub(super) fn reconcile_window(
    target: &mut impl ReconcileTarget,
    touched: &Touched,
    terminating: bool,
) -> WindowChange {
    let mut change = WindowChange::default();
    // A swap or a move within a tab changes no membership, only order. The
    // views follow it first, so a pane that adoption or removal takes away
    // hands focus to the first pane in the new order.
    for (tab, _) in target.installed() {
        change.changed |= target.order_panes(tab);
    }
    // Panes that moved between tabs this window shows keep their views,
    // before removal could drop them; never in a busy window, whose own
    // completion reconciles once it ends.
    if !target.busy() && !terminating && adopt_moved_panes(target) {
        change.changed = true;
        change.titles = true;
        change.adopted = true;
    }
    let installed = target.installed();
    let remove = removals(
        target.projection(),
        target.workspace(),
        target.session(),
        &installed,
        target.busy(),
        terminating,
    );
    if !remove.is_empty() {
        target.drop_tabs(&remove);
        // The removed tabs' published titles are gone too, so title
        // consumers such as a Quit dialog elsewhere must redraw.
        change.changed = true;
        change.titles = true;
        change.emptied = target.installed().is_empty();
    }
    // Panes leave their tab the way tabs leave the window: never in a busy
    // window, whose own completion owns the aftermath.
    if !target.busy() && !terminating && drop_left_panes(target) {
        change.changed = true;
        change.titles = true;
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
    // A tab the projection no longer holds in this window's workspace keeps
    // its last published title until its view is dropped; republishing it
    // now would lose its name while a busy window's close commits.
    let workspace = target.workspace();
    let held: Vec<TabId> = current
        .iter()
        .copied()
        .filter(|tab| {
            workspace.is_some()
                && target.projection().tab_workspace(*tab) == workspace
        })
        .collect();
    if publish_touched_titles(target, touched, &current, &held) {
        change.changed = true;
        change.titles = true;
    }
    target.pending_installs().touched(touched, workspace);
    let pending = target.pending_installs().excluded();
    let installed: Vec<TabId> =
        target.installed().into_iter().map(|(tab, _)| tab).collect();
    let mut install = installs(
        target.projection(),
        workspace,
        target.session(),
        &installed,
        &pending,
        target.busy(),
        terminating,
    );
    // A shown tab whose layout gained a pane installs the new pane's view.
    let held = installs(
        target.projection(),
        workspace,
        target.session(),
        &[],
        &pending,
        target.busy(),
        terminating,
    );
    install.extend(installed.iter().copied().filter(|tab| {
        held.contains(tab)
            && missing_panes(
                target.projection(),
                *tab,
                &target.installed_panes(*tab),
            )
    }));
    if !install.is_empty() {
        target.pending_installs().begin(&install);
        target.install(&install);
    }
    let pending = target.pending_installs().excluded();
    let replace = replacements(
        target.projection(),
        workspace,
        target.session(),
        &target.revoked(),
        &pending,
        target.busy(),
        terminating,
    );
    if !replace.is_empty() {
        target.pending_installs().begin(&replace);
        target.replace(&replace);
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
