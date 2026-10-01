use super::*;
use crate::desktop::windows::model::{TabEntry, WindowLayout};
use gpui::{Bounds, WindowBounds, point, px, size};
use huterm_core::Mux;
use huterm_protocol::{
    HierarchyEnvelope, HierarchyEvent, PaneId, RuntimeId, SessionId,
    SessionInfo, StreamId, TabInfo, TerminalId, WorkspaceInfo,
};

const RUNTIME: RuntimeId = RuntimeId::new(42);

fn session(value: u64) -> SessionId {
    SessionId::in_runtime(RUNTIME, value)
}

fn workspace(value: u64) -> WorkspaceId {
    WorkspaceId::in_runtime(RUNTIME, value)
}

fn tab(value: u64) -> TabId {
    TabId::in_runtime(RUNTIME, value)
}

fn window(value: u64) -> WindowId {
    WindowId::from(value)
}

/// A projection fed by hand-built events, as a client's drain applies them.
struct Feed {
    state: HierarchyState,
}

impl Feed {
    /// Session 1 holds workspaces 10 and 11, both empty.
    fn new() -> Self {
        let mut feed = Self {
            state: HierarchyState::new(StreamId::new(RUNTIME)),
        };
        feed.apply(HierarchyEvent::SessionCreated {
            session: SessionInfo {
                id: session(1),
                custom_name: None,
                automatic_name: "Session 1".into(),
            },
            index: 0,
        });
        for (index, value) in [10, 11].into_iter().enumerate() {
            feed.apply(HierarchyEvent::WorkspaceCreated {
                session: session(1),
                workspace: WorkspaceInfo {
                    id: workspace(value),
                    custom_name: None,
                    automatic_name: format!("Workspace {value}"),
                },
                index: u32::try_from(index).unwrap(),
            });
        }
        feed
    }

    fn apply(&mut self, event: HierarchyEvent) -> Touched {
        let envelope = HierarchyEnvelope {
            stream: self.state.stream(),
            seq: self.state.seq() + 1,
            event,
        };
        match self.state.apply(envelope) {
            ApplyOutcome::Applied(touched) => touched,
            other => panic!("event did not apply: {other:?}"),
        }
    }

    /// Opens tab `value` at the end of `workspace`; returns its sequence.
    fn open(&mut self, workspace_id: WorkspaceId, value: u64) -> u64 {
        let index = self.state.workspace_tabs(workspace_id).unwrap().len();
        self.apply(HierarchyEvent::TabOpened {
            workspace: workspace_id,
            tab: TabInfo {
                id: tab(value),
                pane_id: PaneId::new(value),
                terminal_id: TerminalId::new(value),
                custom_name: None,
                fallback_name: "sh".into(),
            },
            index: u32::try_from(index).unwrap(),
        });
        self.state.seq()
    }

    fn rename(&mut self, value: u64, name: Option<&str>) -> Touched {
        self.apply(HierarchyEvent::TabRenamed {
            tab: tab(value),
            custom_name: name.map(str::to_owned),
        })
    }

    fn move_tab(&mut self, value: u64, workspace_id: WorkspaceId, index: u32) {
        self.apply(HierarchyEvent::TabMoved {
            tab: tab(value),
            workspace: workspace_id,
            index,
        });
    }
}

/// A terminal view the fake window installed.
struct View {
    tab: TabId,
    committed: u64,
    terminal_title: String,
}

/// One window's views and model entries, with the label rule `TabView`
/// applies.
struct FakeWindow {
    id: WindowId,
    workspace: WorkspaceId,
    views: Vec<View>,
    busy: bool,
}

struct Target<'a> {
    window: &'a mut FakeWindow,
    model: &'a mut WindowModel,
    state: &'a HierarchyState,
}

impl ReconcileTarget for Target<'_> {
    fn projection(&self) -> &HierarchyState {
        self.state
    }

    fn workspace(&self) -> Option<WorkspaceId> {
        Some(self.window.workspace)
    }

    fn installed(&self) -> Vec<(TabId, u64)> {
        self.window
            .views
            .iter()
            .map(|view| (view.tab, view.committed))
            .collect()
    }

    fn busy(&self) -> bool {
        self.window.busy
    }

    fn drop_tabs(&mut self, tabs: &[TabId]) {
        self.window.views.retain(|view| !tabs.contains(&view.tab));
        self.model.close_tabs(self.window.id, tabs);
    }

    fn apply_order(&mut self, order: &[TabId]) -> bool {
        assert!(self.model.apply_order(self.window.id, order));
        self.window.views.sort_by_key(|view| {
            order.iter().position(|tab| *tab == view.tab).unwrap()
        });
        true
    }

    fn title(&self, tab: TabId) -> String {
        let view = self
            .window
            .views
            .iter()
            .find(|view| view.tab == tab)
            .unwrap();
        self.state.tab(tab).map_or_else(
            || view.terminal_title.clone(),
            |info| info.display_name(&view.terminal_title).to_owned(),
        )
    }

    fn publish_title(&mut self, tab: TabId, title: &str) -> bool {
        self.model.set_tab_title(self.window.id, tab, title)
    }
}

fn layout() -> WindowLayout {
    WindowLayout {
        bounds: WindowBounds::Windowed(Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(800.0), px(600.0)),
        }),
        sidebar_width: px(180.0),
    }
}

fn open_window(
    model: &mut WindowModel,
    id: u64,
    workspace_id: WorkspaceId,
) -> FakeWindow {
    model.open(window(id), None, layout());
    model.attach(window(id), None, session(1), workspace_id);
    FakeWindow {
        id: window(id),
        workspace: workspace_id,
        views: Vec::new(),
        busy: false,
    }
}

/// Installs a spawned view as `push_tab_view` does: append, then apply the
/// installed order.
fn push(
    fake: &mut FakeWindow,
    model: &mut WindowModel,
    state: &HierarchyState,
    value: u64,
    committed: u64,
) {
    fake.views.push(View {
        tab: tab(value),
        committed,
        terminal_title: format!("title {value}"),
    });
    model.open_tab(
        fake.id,
        TabEntry {
            id: tab(value),
            terminal: TerminalId::new(value),
            title: format!("title {value}"),
        },
    );
    let current: Vec<TabId> = fake.views.iter().map(|view| view.tab).collect();
    let order = installed_order(
        state.workspace_tabs(fake.workspace).unwrap_or_default(),
        &current,
    );
    if order != current {
        Target {
            window: fake,
            model,
            state,
        }
        .apply_order(&order);
    }
}

fn reconcile(
    fake: &mut FakeWindow,
    model: &mut WindowModel,
    state: &HierarchyState,
    touched: &Touched,
) -> WindowChange {
    reconcile_window(
        &mut Target {
            window: fake,
            model,
            state,
        },
        touched,
        false,
    )
}

fn view_order(fake: &FakeWindow) -> Vec<TabId> {
    fake.views.iter().map(|view| view.tab).collect()
}

fn model_order(model: &WindowModel, fake: &FakeWindow) -> Vec<TabId> {
    model
        .record(fake.id)
        .unwrap()
        .tabs
        .iter()
        .map(|entry| entry.id)
        .collect()
}

fn model_title(model: &WindowModel, fake: &FakeWindow, value: u64) -> String {
    model
        .record(fake.id)
        .unwrap()
        .tabs
        .iter()
        .find(|entry| entry.id == tab(value))
        .unwrap()
        .title
        .clone()
}

#[test]
fn an_empty_summary_visits_no_window() {
    let mut feed = Feed::new();
    let mut model = WindowModel::default();
    let mut fake = open_window(&mut model, 1, workspace(10));
    let seq = feed.open(workspace(10), 1);
    push(&mut fake, &mut model, &feed.state, 1, seq);
    assert!(windows_to_visit(&model, &Touched::default()).is_empty());
    assert_eq!(
        windows_to_visit(&model, &Touched::everything()),
        [window(1)]
    );
}

#[test]
fn several_renames_notify_their_window_once_and_skip_other_windows() {
    let mut feed = Feed::new();
    let mut model = WindowModel::default();
    let mut first = open_window(&mut model, 1, workspace(10));
    let mut second = open_window(&mut model, 2, workspace(11));
    for value in [1, 2, 3] {
        let seq = feed.open(workspace(10), value);
        push(&mut first, &mut model, &feed.state, value, seq);
    }
    let seq = feed.open(workspace(11), 4);
    push(&mut second, &mut model, &feed.state, 4, seq);

    let mut touched = Touched::default();
    for (value, name) in [(1, "one"), (2, "two"), (3, "three")] {
        touched.merge(feed.rename(value, Some(name)));
    }
    assert_eq!(windows_to_visit(&model, &touched), [window(1)]);
    let mut notifications = 0;
    for id in windows_to_visit(&model, &touched) {
        assert_eq!(id, first.id);
        if reconcile(&mut first, &mut model, &feed.state, &touched).changed {
            notifications += 1;
        }
    }
    assert_eq!(notifications, 1);
    assert_eq!(model_title(&model, &first, 2), "two");
    assert_eq!(model_title(&model, &second, 4), "title 4");

    // Reconciling again, as a completion would after the drain already
    // did, writes and notifies nothing.
    let again = reconcile(&mut first, &mut model, &feed.state, &touched);
    assert_eq!(again, WindowChange::default());
}

#[test]
fn a_view_the_projection_has_not_caught_up_with_is_never_dropped() {
    let mut feed = Feed::new();
    let mut model = WindowModel::default();
    let mut fake = open_window(&mut model, 1, workspace(10));
    // The view committed at a sequence the projection has not applied, so
    // the projection not holding its tab proves nothing yet.
    let ahead = feed.state.seq() + 2;
    push(&mut fake, &mut model, &feed.state, 1, ahead);
    let change =
        reconcile(&mut fake, &mut model, &feed.state, &Touched::everything());
    assert!(!change.changed);
    assert_eq!(view_order(&fake), [tab(1)]);

    // Once applied and gone, the view goes from views and model together.
    feed.open(workspace(10), 1);
    feed.apply(HierarchyEvent::TabClosed { tab: tab(1) });
    assert!(feed.state.seq() >= ahead);
    let change =
        reconcile(&mut fake, &mut model, &feed.state, &Touched::everything());
    assert!(change.changed);
    assert!(fake.views.is_empty());
    assert!(model_order(&model, &fake).is_empty());
}

#[test]
fn a_projected_tab_without_a_view_is_tolerated_while_its_spawn_is_in_flight() {
    let mut feed = Feed::new();
    let mut model = WindowModel::default();
    let mut fake = open_window(&mut model, 1, workspace(10));
    fake.busy = true;
    let seq = feed.open(workspace(10), 1);
    // The completion drains before `push_tab_view`, while `busy` is set.
    reconcile(&mut fake, &mut model, &feed.state, &Touched::everything());
    fake.busy = false;
    push(&mut fake, &mut model, &feed.state, 1, seq);
    let change =
        reconcile(&mut fake, &mut model, &feed.state, &Touched::everything());
    assert!(!change.changed);
}

#[test]
fn a_projected_tab_without_a_view_is_tolerated_in_an_idle_window() {
    // Destroying a view only detaches it; the tab lives on in the
    // projection, and reconcile neither asserts nor installs a view.
    let mut feed = Feed::new();
    let mut model = WindowModel::default();
    let mut fake = open_window(&mut model, 1, workspace(10));
    let seq = feed.open(workspace(10), 1);
    push(&mut fake, &mut model, &feed.state, 1, seq);
    feed.open(workspace(10), 2);
    assert!(!fake.busy);
    let change =
        reconcile(&mut fake, &mut model, &feed.state, &Touched::everything());
    assert!(!change.changed);
    assert_eq!(view_order(&fake), [tab(1)]);
    assert_eq!(model_order(&model, &fake), [tab(1)]);
}

#[test]
fn removal_waits_for_teardown_to_own_it() {
    let mut feed = Feed::new();
    let seq = feed.open(workspace(10), 1);
    feed.apply(HierarchyEvent::TabClosed { tab: tab(1) });
    let installed = [(tab(1), seq)];
    // Quit pending, assessing, confirming, or cancelled leaves `terminating`
    // clear, so reconcile still removes; only committed teardown skips.
    assert_eq!(
        removals(&feed.state, Some(workspace(10)), &installed, false, false),
        [tab(1)]
    );
    assert!(
        removals(&feed.state, Some(workspace(10)), &installed, false, true)
            .is_empty()
    );
}

#[test]
fn unheld_tabs_keep_their_slots_while_held_tabs_follow_the_projection() {
    let (a, b, c) = (tab(1), tab(2), tab(3));
    // A just-pushed tab lands at its projected position.
    assert_eq!(installed_order(&[a, c, b], &[a, b, c]), [a, c, b]);
    // A closed tab the window still shows keeps its slot.
    assert_eq!(installed_order(&[a, c], &[a, b, c]), [a, b, c]);
    assert_eq!(installed_order(&[c, a], &[a, b, c]), [c, b, a]);
    // Projected tabs without views do not take slots.
    assert_eq!(installed_order(&[b, tab(4), a], &[a, b]), [b, a]);
}

#[test]
fn a_projected_order_ahead_of_installed_views_converges_in_either_arrival_order()
 {
    // The projection already holds C between B and A when C's view arrives.
    let mut feed = Feed::new();
    let mut model = WindowModel::default();
    let mut fake = open_window(&mut model, 1, workspace(10));
    let a = feed.open(workspace(10), 1);
    push(&mut fake, &mut model, &feed.state, 1, a);
    let b = feed.open(workspace(10), 2);
    push(&mut fake, &mut model, &feed.state, 2, b);
    let c = feed.open(workspace(10), 3);
    feed.move_tab(2, workspace(10), 0);
    feed.move_tab(3, workspace(10), 1);
    fake.busy = true;
    reconcile(&mut fake, &mut model, &feed.state, &Touched::everything());
    // B before A applies to the installed views; C has no view yet.
    assert_eq!(view_order(&fake), [tab(2), tab(1)]);
    fake.busy = false;
    push(&mut fake, &mut model, &feed.state, 3, c);
    assert_eq!(view_order(&fake), [tab(2), tab(3), tab(1)]);
    assert_eq!(model_order(&model, &fake), view_order(&fake));

    // The view arrives first; the projection then places it mid-order.
    let mut feed = Feed::new();
    let mut model = WindowModel::default();
    let mut fake = open_window(&mut model, 1, workspace(10));
    let a = feed.open(workspace(10), 1);
    push(&mut fake, &mut model, &feed.state, 1, a);
    let b = feed.open(workspace(10), 2);
    push(&mut fake, &mut model, &feed.state, 2, b);
    let early = feed.state.clone();
    let c = feed.state.seq() + 1;
    push(&mut fake, &mut model, &early, 3, c);
    assert_eq!(view_order(&fake), [tab(1), tab(2), tab(3)]);
    feed.open(workspace(10), 3);
    feed.move_tab(3, workspace(10), 1);
    reconcile(&mut fake, &mut model, &feed.state, &Touched::everything());
    assert_eq!(view_order(&fake), [tab(1), tab(3), tab(2)]);
    assert_eq!(model_order(&model, &fake), view_order(&fake));
}

#[test]
fn a_close_after_a_missed_reorder_converges() {
    let mut feed = Feed::new();
    let mut model = WindowModel::default();
    let mut fake = open_window(&mut model, 1, workspace(10));
    for value in [1, 2, 3] {
        let seq = feed.open(workspace(10), value);
        push(&mut fake, &mut model, &feed.state, value, seq);
    }
    // A reorder the window never reconciled, then a close of B.
    feed.move_tab(3, workspace(10), 0);
    feed.apply(HierarchyEvent::TabClosed { tab: tab(2) });
    reconcile(&mut fake, &mut model, &feed.state, &Touched::everything());
    assert_eq!(view_order(&fake), [tab(3), tab(1)]);
    assert_eq!(model_order(&model, &fake), view_order(&fake));
}

#[test]
fn a_reset_and_a_title_change_reach_the_same_title_in_either_order() {
    for reset_first in [true, false] {
        let mut feed = Feed::new();
        let mut model = WindowModel::default();
        let mut fake = open_window(&mut model, 1, workspace(10));
        let seq = feed.open(workspace(10), 1);
        push(&mut fake, &mut model, &feed.state, 1, seq);
        let touched = feed.rename(1, Some("custom"));
        reconcile(&mut fake, &mut model, &feed.state, &touched);
        assert_eq!(model_title(&model, &fake, 1), "custom");

        let retitle = |fake: &mut FakeWindow,
                       model: &mut WindowModel,
                       state: &HierarchyState| {
            // The terminal drain republishes its tab's title.
            fake.views[0].terminal_title = "vim".into();
            let mut target = Target {
                window: fake,
                model,
                state,
            };
            let title = target.title(tab(1));
            target.publish_title(tab(1), &title);
        };
        if reset_first {
            let touched = feed.rename(1, None);
            reconcile(&mut fake, &mut model, &feed.state, &touched);
            retitle(&mut fake, &mut model, &feed.state);
        } else {
            retitle(&mut fake, &mut model, &feed.state);
            assert_eq!(model_title(&model, &fake, 1), "custom");
            let touched = feed.rename(1, None);
            reconcile(&mut fake, &mut model, &feed.state, &touched);
        }
        assert_eq!(
            model_title(&model, &fake, 1),
            "vim",
            "reset first: {reset_first}"
        );
    }
}

#[test]
fn title_refreshes_are_scheduled_only_for_open_consumers_and_coalesce() {
    let mut consumers = TitleConsumers::default();
    assert!(!consumers.mark(window(1)), "no consumer, no refresh");
    assert!(consumers.take_dirty().is_empty());
    consumers.set_host(window(2), true);
    assert!(consumers.mark(window(1)));
    assert!(!consumers.mark(window(3)), "one refresh per turn");
    assert_eq!(
        consumers.take_dirty(),
        HashSet::from([window(1), window(3)])
    );
    assert!(consumers.mark(window(1)), "the next change schedules again");
    consumers.set_host(window(2), false);
    assert!(consumers.hosts().is_empty());
}

#[test]
fn waiters_resolve_once_in_sequence_order() {
    let mut waiters = SequenceWaiters::default();
    let late = waiters.register(5);
    let early = waiters.register(3);
    waiters.release(4);
    assert_eq!(early.try_recv(), Ok(Resolution::Ready));
    assert!(late.try_recv().is_err());
    // A cancellation racing the release resolves only what is left.
    waiters.release(5);
    waiters.cancel_all();
    assert_eq!(late.try_recv(), Ok(Resolution::Ready));
    assert!(late.try_recv().is_err(), "resolved exactly once");
    assert!(late.is_closed());
    assert_eq!(waiters.len(), 0);

    let dropped = SequenceWaiters::default().register(1);
    assert_eq!(dropped.try_recv(), Ok(Resolution::Cancelled));
}

fn rename_workspace(
    mux: &mut Mux,
    workspace_id: WorkspaceId,
    name: &str,
) -> u64 {
    mux.rename_workspace(workspace_id, Some(name)).unwrap();
    mux.hierarchy_seq()
}

#[test]
fn a_completion_proceeds_at_once_or_waits_for_the_next_drain() {
    let mut mux = Mux::default();
    let session_id = mux.create_session(None).unwrap();
    let workspace_id = mux.create_workspace(session_id, None).unwrap();
    let (state, subscription) = mux.subscribe_hierarchy();
    let mut projection = Projection::new(state, subscription);

    // Already applied: the completion continues on the same turn.
    let applied = rename_workspace(&mut mux, workspace_id, "one");
    assert!(matches!(projection.sync(false), Drained::Current(_)));
    assert!(matches!(projection.wait_for(applied, false), Wait::Ready));

    // A result that arrives before its event waits; the next ordinary
    // drain releases it.
    let ahead = rename_workspace(&mut mux, workspace_id, "two");
    let Wait::Pending(waiter) = projection.wait_for(ahead, false) else {
        panic!("a result ahead of the projection must wait");
    };
    assert!(waiter.try_recv().is_err());
    assert!(matches!(projection.sync(false), Drained::Current(_)));
    projection.settle_waiters(false);
    assert_eq!(waiter.try_recv(), Ok(Resolution::Ready));
    assert_eq!(
        projection
            .state()
            .workspace(workspace_id)
            .unwrap()
            .display_name(),
        "two"
    );
}

#[test]
fn teardown_cancels_waiters_and_later_waits() {
    let mut mux = Mux::default();
    let session_id = mux.create_session(None).unwrap();
    let workspace_id = mux.create_workspace(session_id, None).unwrap();
    let (state, subscription) = mux.subscribe_hierarchy();
    let mut projection = Projection::new(state, subscription);
    let ahead = rename_workspace(&mut mux, workspace_id, "one");
    let Wait::Pending(waiter) = projection.wait_for(ahead, false) else {
        panic!("a result ahead of the projection must wait");
    };
    projection.settle_waiters(true);
    assert_eq!(waiter.try_recv(), Ok(Resolution::Cancelled));
    assert_eq!(projection.waiting(), 0);
    assert!(matches!(projection.wait_for(ahead, true), Wait::Cancelled));
}

/// Renames until the subscription's bounded queue overflows.
fn overflow(mux: &mut Mux, workspace_id: WorkspaceId) {
    for index in 0..2_000 {
        rename_workspace(mux, workspace_id, &format!("name {index}"));
    }
}

#[test]
fn a_waiter_behind_a_resync_is_released_by_installation() {
    let mut mux = Mux::default();
    let session_id = mux.create_session(None).unwrap();
    let workspace_id = mux.create_workspace(session_id, None).unwrap();
    let (state, subscription) = mux.subscribe_hierarchy();
    let mut projection = Projection::new(state, subscription);
    overflow(&mut mux, workspace_id);
    assert_eq!(projection.sync(false), Drained::Resync);
    assert!(projection.begin_resync());
    assert!(!projection.begin_resync(), "one resubscription at a time");
    let committed = mux.hierarchy_seq();
    let Wait::Pending(waiter) = projection.wait_for(committed, false) else {
        panic!("a lagged projection must wait");
    };

    // A lagged new subscription installs nothing reconcilable.
    let (state, subscription) = mux.subscribe_hierarchy();
    overflow(&mut mux, workspace_id);
    assert_eq!(
        projection.install(state, subscription, false),
        Install::Lagged
    );
    assert!(projection.resyncing());
    assert!(waiter.try_recv().is_err());

    let (state, subscription) = mux.subscribe_hierarchy();
    assert_eq!(
        projection.install(state, subscription, false),
        Install::Current
    );
    assert!(!projection.resyncing());
    projection.settle_waiters(false);
    assert_eq!(waiter.try_recv(), Ok(Resolution::Ready));
    assert_eq!(projection.state().seq(), mux.hierarchy_seq());
}

fn idle_shell() -> huterm_protocol::TerminalCommand {
    huterm_protocol::TerminalCommand {
        program: "/bin/sh".into(),
        arguments: vec!["-c".into(), "read value".into()],
        working_directory: std::env::current_dir().unwrap(),
        environment: Vec::new(),
        grid_size: huterm_protocol::GridSize::clamped(40, 8),
        cell_size: huterm_protocol::CellSize {
            width: 8,
            height: 16,
        },
        presentation: huterm_protocol::TerminalPresentation::default(),
    }
}

#[test]
fn a_resync_snapshot_older_than_an_installed_view_drains_before_reconciling() {
    let mut mux = Mux::default();
    let session_id = mux.create_session(None).unwrap();
    let workspace_id = mux.create_workspace(session_id, None).unwrap();
    let (state, subscription) = mux.subscribe_hierarchy();
    let mut projection = Projection::new(state, subscription);
    overflow(&mut mux, workspace_id);
    assert_eq!(projection.sync(false), Drained::Resync);
    // The worker takes its snapshot before a spawn commits; the spawn's
    // view is installed before the snapshot is.
    let (snapshot, resubscription) = mux.subscribe_hierarchy();
    let opened = mux.open_tab(workspace_id, &idle_shell()).unwrap();
    let committed = mux.hierarchy_seq();
    let mut model = WindowModel::default();
    model.open(window(1), None, layout());
    model.attach(window(1), None, session_id, workspace_id);
    let mut fake = FakeWindow {
        id: window(1),
        workspace: workspace_id,
        views: Vec::new(),
        busy: false,
    };
    fake.views.push(View {
        tab: opened.tab.id,
        committed,
        terminal_title: "sh".into(),
    });
    model.open_tab(
        window(1),
        TabEntry {
            id: opened.tab.id,
            terminal: opened.tab.terminal_id,
            title: "sh".into(),
        },
    );
    assert!(snapshot.tab(opened.tab.id).is_none());

    assert_eq!(
        projection.install(snapshot, resubscription, false),
        Install::Current
    );
    // The view is kept because the drained subscription added its tab.
    assert!(projection.state().tab(opened.tab.id).is_some());
    assert_eq!(projection.state().seq(), committed);
    let change = reconcile_window(
        &mut Target {
            window: &mut fake,
            model: &mut model,
            state: projection.state(),
        },
        &Touched::everything(),
        false,
    );
    assert!(!change.changed);
    assert_eq!(view_order(&fake), [opened.tab.id]);
    mux.shutdown().unwrap();
}

#[test]
fn a_reset_queued_after_teardown_is_not_applied_and_labels_keep_custom_names() {
    let mut mux = Mux::default();
    let session_id = mux.create_session(None).unwrap();
    let workspace_id = mux.create_workspace(session_id, None).unwrap();
    let opened = mux.open_tab(workspace_id, &idle_shell()).unwrap();
    let committed = mux.hierarchy_seq();
    let (state, subscription) = mux.subscribe_hierarchy();
    let mut projection = Projection::new(state, subscription);
    mux.rename_tab(opened.tab.id, Some("custom")).unwrap();
    assert!(matches!(projection.sync(false), Drained::Current(_)));
    let before = projection.state().clone();
    let Wait::Pending(waiter) = projection.wait_for(before.seq() + 1, false)
    else {
        panic!("a later sequence must wait");
    };

    // Teardown commits first, then shutdown queues `Reset`.
    mux.shutdown().unwrap();
    assert_eq!(projection.sync(true), Drained::Frozen);
    assert_eq!(*projection.state(), before);
    assert_eq!(waiter.try_recv(), Ok(Resolution::Cancelled));
    let (empty, resubscription) = mux.subscribe_hierarchy();
    assert_eq!(
        projection.install(empty, resubscription, true),
        Install::Frozen
    );
    assert_eq!(*projection.state(), before);

    let mut model = WindowModel::default();
    model.open(window(1), None, layout());
    model.attach(window(1), None, session_id, workspace_id);
    let mut fake = FakeWindow {
        id: window(1),
        workspace: workspace_id,
        views: vec![View {
            tab: opened.tab.id,
            committed,
            terminal_title: "sh".into(),
        }],
        busy: false,
    };
    let target = Target {
        window: &mut fake,
        model: &mut model,
        state: projection.state(),
    };
    assert_eq!(target.title(opened.tab.id), "custom");
}

#[test]
fn a_busy_window_keeps_removed_views_until_its_completion_reconciles() {
    let mut feed = Feed::new();
    let mut model = WindowModel::default();
    let mut fake = open_window(&mut model, 1, workspace(10));
    for value in [1, 2, 3] {
        let seq = feed.open(workspace(10), value);
        push(&mut fake, &mut model, &feed.state, value, seq);
    }
    feed.apply(HierarchyEvent::TabClosed { tab: tab(2) });
    // The wake-driven drain reconciles while the window's own close is in
    // flight: the closed view, which may hold focus, stays in its slot for
    // the completion to drop and refocus in one update.
    fake.busy = true;
    let change =
        reconcile(&mut fake, &mut model, &feed.state, &Touched::everything());
    assert!(!change.changed);
    assert_eq!(view_order(&fake), [tab(1), tab(2), tab(3)]);
    // Order still applies around it.
    feed.move_tab(3, workspace(10), 0);
    reconcile(&mut fake, &mut model, &feed.state, &Touched::everything());
    assert_eq!(view_order(&fake), [tab(3), tab(2), tab(1)]);
    assert_eq!(model_order(&model, &fake), view_order(&fake));

    // The completion clears `busy` and reconciles its own window with no
    // summary; the deferred removal applies then.
    fake.busy = false;
    let change =
        reconcile(&mut fake, &mut model, &feed.state, &Touched::default());
    assert!(change.changed);
    assert!(change.titles, "a removed title must redraw title consumers");
    assert_eq!(view_order(&fake), [tab(3), tab(1)]);
    assert_eq!(model_order(&model, &fake), view_order(&fake));
}

#[test]
fn a_reset_drained_before_the_teardown_flag_is_seen_freezes_the_projection() {
    let mut mux = Mux::default();
    let session_id = mux.create_session(None).unwrap();
    let workspace_id = mux.create_workspace(session_id, None).unwrap();
    let opened = mux.open_tab(workspace_id, &idle_shell()).unwrap();
    let (state, subscription) = mux.subscribe_hierarchy();
    let mut projection = Projection::new(state, subscription);
    mux.rename_tab(opened.tab.id, Some("custom")).unwrap();
    assert!(matches!(projection.sync(false), Drained::Current(_)));
    let before = projection.state().clone();

    // The drain sampled `terminating` before a worker set it and queued
    // `Reset`; the `Reset` itself freezes the projection.
    mux.rename_tab(opened.tab.id, Some("late")).unwrap();
    mux.shutdown().unwrap();
    assert_eq!(projection.sync(false), Drained::Frozen);
    assert_eq!(
        projection
            .state()
            .tab(opened.tab.id)
            .and_then(|tab| tab.custom_name.as_deref()),
        Some("late"),
        "events before the Reset still apply"
    );
    assert_eq!(projection.state().sessions(), before.sessions());
    let frozen = projection.state().clone();
    assert_eq!(projection.sync(false), Drained::Frozen);
    assert_eq!(*projection.state(), frozen);
    assert!(matches!(
        projection.wait_for(frozen.seq() + 1, false),
        Wait::Cancelled
    ));
}

#[test]
fn a_tab_moved_to_another_workspace_leaves_the_old_window() {
    let mut feed = Feed::new();
    let mut model = WindowModel::default();
    let mut first = open_window(&mut model, 1, workspace(10));
    let second = open_window(&mut model, 2, workspace(11));
    for value in [1, 2] {
        let seq = feed.open(workspace(10), value);
        push(&mut first, &mut model, &feed.state, value, seq);
    }
    feed.move_tab(2, workspace(11), 0);
    let change =
        reconcile(&mut first, &mut model, &feed.state, &Touched::everything());
    assert!(change.changed && change.titles);
    assert_eq!(view_order(&first), [tab(1)]);
    assert_eq!(model_order(&model, &first), [tab(1)]);
    // The destination window installs nothing: creating views for tabs it
    // did not spawn belongs to #22.
    assert!(model_order(&model, &second).is_empty());
    assert_eq!(
        removals(
            &feed.state,
            Some(workspace(11)),
            &[(tab(2), feed.state.seq())],
            false,
            false
        ),
        Vec::<TabId>::new()
    );
}

#[test]
fn a_closed_stream_cancels_waiters_and_freezes_the_projection() {
    let mut mux = Mux::default();
    let session_id = mux.create_session(None).unwrap();
    let workspace_id = mux.create_workspace(session_id, None).unwrap();
    let (state, subscription) = mux.subscribe_hierarchy();
    let mut projection = Projection::new(state, subscription);
    let applied = rename_workspace(&mut mux, workspace_id, "last");
    let Wait::Pending(waiter) = projection.wait_for(applied + 1, false) else {
        panic!("a sequence the stream never reaches must wait");
    };
    // The stream ends without teardown committing.
    drop(mux);
    assert_eq!(projection.sync(false), Drained::Frozen);
    assert_eq!(waiter.try_recv(), Ok(Resolution::Cancelled));
    assert_eq!(
        projection.state().seq(),
        applied,
        "queued events still apply"
    );
    assert_eq!(
        projection
            .state()
            .workspace(workspace_id)
            .unwrap()
            .display_name(),
        "last"
    );
    assert!(matches!(
        projection.wait_for(applied + 1, false),
        Wait::Cancelled
    ));
    assert_eq!(projection.sync(false), Drained::Frozen);
}
