use super::*;
use crate::mux::HierarchyRecvError;
use crate::mux::live_fixture::{ECHO, Live};
use crate::mux::tests::command;
use huterm_protocol::{
    BUILT_IN_LAYOUTS, HierarchyEvent, NodeId, PaneId, PaneNode, SplitDirection,
    TerminalCommand,
};

/// A tab of three panes, `[1 | (2 over 3)]`, running `script`.
fn three_panes(
    mux: &mut Mux,
    script: &TerminalCommand,
) -> (WorkspaceId, TabId, [PaneId; 3]) {
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let tab = mux.open_tab(workspace, script).unwrap().tab.id;
    let first = mux.tab(tab).unwrap().first_pane().id;
    let split = |mux: &mut Mux, pane, direction| {
        mux.split_pane(workspace, tab, pane, direction, script)
            .unwrap()
            .pane
            .id
    };
    let second = split(mux, first, SplitDirection::Right);
    let third = split(mux, second, SplitDirection::Down);
    (workspace, tab, [first, second, third])
}

fn layout(mux: &Mux, tab: TabId) -> PaneLayout {
    mux.tab(tab).unwrap().layout().clone()
}

/// The built-in layout `name` for the tab's panes.
fn built_in(mux: &Mux, tab: TabId, name: &str) -> Arrangement {
    let panes: Vec<PaneId> = layout(mux, tab)
        .panes()
        .iter()
        .map(|pane| pane.id)
        .collect();
    huterm_protocol::NamedLayout::built_in(name)
        .unwrap()
        .template
        .expand(&panes)
        .unwrap()
}

/// Asserts that the tab's layout is exactly `arrangement`, weights
/// included.
fn assert_holds(mux: &Mux, tab: TabId, arrangement: &Arrangement) {
    let current = layout(mux, tab);
    assert_eq!(
        current.arranged(arrangement, || panic!("the shape differs")),
        Ok(current)
    );
}

/// The root split's identity and children.
fn root(mux: &Mux, tab: TabId) -> (SplitId, Vec<NodeId>) {
    let PaneNode::Split(split) = mux.tab(tab).unwrap().layout().root() else {
        panic!("tab {tab:?} is not split");
    };
    (
        split.id,
        split.children.iter().map(|child| child.node.id()).collect(),
    )
}

#[test]
fn every_built_in_and_a_balance_keep_live_shells_and_their_identities() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let workspace = mux.create_workspace(session, None).unwrap();
    let script = command(ECHO);
    // [1 | (2 over 3)]
    let opened = mux.open_test_tab(workspace, &script).unwrap();
    let tab = opened.tab.id;
    let p1 = Live::new("pa", opened.tab.first_pane(), &opened.client);
    let two = mux
        .split_test_pane(
            workspace,
            tab,
            p1.pane,
            SplitDirection::Right,
            &script,
        )
        .unwrap();
    let p2 = Live::new("pb", &two.pane, &two.client);
    let three = mux
        .split_test_pane(workspace, tab, p2.pane, SplitDirection::Down, &script)
        .unwrap();
    let p3 = Live::new("pc", &three.pane, &three.client);
    let panes = [&p1, &p2, &p3];
    let order = [p1.pane, p2.pane, p3.pane];
    for pane in panes {
        pane.answers(&mux, 0);
    }

    let names = BUILT_IN_LAYOUTS.iter().map(|layout| layout.name.as_str());
    let steps: Vec<&str> = names.chain(["main_left", "balance"]).collect();
    for (step, name) in steps.into_iter().enumerate() {
        let expected = layout(&mux, tab);
        let arrangement = if name == "balance" {
            expected.balanced()
        } else {
            built_in(&mux, tab, name)
        };
        let change = mux
            .arrange_panes(workspace, tab, &expected, &arrangement)
            .unwrap();
        assert!(!change.is_empty(), "{name} changed nothing");
        assert_holds(&mux, tab, &arrangement);
        let ids: Vec<PaneId> = layout(&mux, tab)
            .panes()
            .iter()
            .map(|pane| pane.id)
            .collect();
        assert_eq!(ids, order, "{name} reordered the panes");
        assert_eq!(mux.terminal_count(), 3);
        for pane in panes {
            pane.answers(&mux, step + 1);
        }
    }
    mux.shutdown().unwrap();
}

#[test]
fn layouts_take_fresh_split_ids_and_weights_keep_them_without_the_revision() {
    let mut mux = Mux::default();
    let (workspace, tab, _) = three_panes(&mut mux, &command("read value"));
    let (revision, seq, next) =
        (mux.revision, mux.hierarchy_seq(), mux.next_id);

    // Rows of three panes: one new split, the next identity.
    let rows = built_in(&mux, tab, "rows");
    let before = layout(&mux, tab);
    let change = mux.arrange_panes(workspace, tab, &before, &rows).unwrap();
    assert_eq!(change.before[0].record.as_ref().unwrap().layout, before);
    assert_eq!(change.placed, Vec::new());
    let after = layout(&mux, tab);
    assert_eq!(
        after.split_ids(),
        [SplitId::in_runtime(mux.runtime_id, next + 1)]
    );
    assert_eq!(mux.next_id, next + 1);
    // Main top has two splits, numbered in traversal order.
    let main_top = built_in(&mux, tab, "main_top");
    mux.arrange_panes(workspace, tab, &after, &main_top)
        .unwrap();
    assert_eq!(
        layout(&mux, tab).split_ids(),
        [next + 2, next + 3].map(|id| SplitId::in_runtime(mux.runtime_id, id))
    );
    assert_eq!(mux.next_id, next + 3);

    // A balance keeps the shape: no identity, only weights.
    let (split, children) = root(&mux, tab);
    mux.resize_split(workspace, tab, split, &children, &[300_000, 700_000])
        .unwrap();
    let resized = layout(&mux, tab);
    mux.arrange_panes(workspace, tab, &resized, &resized.balanced())
        .unwrap();
    let balanced = layout(&mux, tab);
    assert_eq!(balanced.split_ids(), resized.split_ids());
    assert!(balanced.same_structure(&resized));
    assert_ne!(balanced, resized);
    assert_eq!(mux.next_id, next + 3);
    // One event per change, and none touched the close revision.
    assert_eq!(mux.hierarchy_seq(), seq + 4);
    assert_eq!(mux.revision, revision);
    mux.shutdown().unwrap();
}

#[test]
fn no_ops_and_refusals_change_nothing() {
    let mut mux = Mux::default();
    let idle = command("read value");
    let (workspace, tab, [first, second, _]) = three_panes(&mut mux, &idle);
    let lone = mux.open_tab(workspace, &idle).unwrap().tab;
    let lone_pane = lone.first_pane().id;
    mux.set_pane_zoom(workspace, tab, Some(second)).unwrap();
    let (_, subscription) = mux.subscribe_hierarchy();
    let current = layout(&mux, tab);
    let unchanged = |mux: &Mux, seq: u64, revision: u64, next: u64| {
        assert_eq!(mux.hierarchy_seq(), seq);
        assert_eq!(mux.revision, revision);
        assert_eq!(mux.next_id, next);
        assert_eq!(*mux.tab(tab).unwrap().layout(), current);
        assert_eq!(mux.tab(tab).unwrap().zoomed(), Some(second));
        assert_eq!(subscription.try_recv(), Err(HierarchyRecvError::Empty));
    };
    let (seq, revision, next) =
        (mux.hierarchy_seq(), mux.revision, mux.next_id);

    // The split's halves are already balanced, so balancing is a no-op and
    // the shared zoom stays.
    let no_op = mux
        .arrange_panes(workspace, tab, &current, &current.balanced())
        .unwrap();
    assert!(no_op.is_empty());
    // A tab of one pane: every layout of one pane is that pane.
    for name in ["columns", "tiled"] {
        let arrangement = built_in(&mux, lone.id, name);
        assert_eq!(arrangement, Arrangement::Pane(lone_pane));
        assert!(
            mux.arrange_panes(workspace, lone.id, lone.layout(), &arrangement)
                .unwrap()
                .is_empty()
        );
    }
    unchanged(&mux, seq, revision, next);

    // The tab no longer has the expected structure.
    let rows = built_in(&mux, tab, "rows");
    assert!(matches!(
        mux.arrange_panes(workspace, tab, lone.layout(), &rows),
        Err(MuxError::InvalidLayout(LayoutError::ChildrenChanged))
    ));
    // Arrangements that add, drop, or repeat a pane.
    let columns = |panes: &[PaneId]| Arrangement::Split {
        axis: huterm_protocol::SplitAxis::Columns,
        children: panes
            .iter()
            .zip([400_000, 300_000, 300_000])
            .map(|(pane, weight)| (weight, Arrangement::Pane(*pane)))
            .collect(),
    };
    for (arrangement, error) in [
        (
            columns(&[first, second, lone_pane]),
            LayoutError::UnknownPane,
        ),
        (columns(&[first, second, second]), LayoutError::DuplicateId),
        (Arrangement::Pane(first), LayoutError::MissingPane),
    ] {
        assert!(matches!(
            mux.arrange_panes(workspace, tab, &current, &arrangement),
            Err(MuxError::InvalidLayout(found)) if found == error
        ));
    }
    // Targets of another runtime.
    let mut other = Mux::default();
    let (foreign_workspace, foreign_tab, _) = three_panes(&mut other, &idle);
    assert!(matches!(
        mux.arrange_panes(foreign_workspace, tab, &current, &rows),
        Err(MuxError::ForeignRuntime(_))
    ));
    assert!(matches!(
        mux.arrange_panes(workspace, foreign_tab, &current, &rows),
        Err(MuxError::ForeignRuntime(_))
    ));
    unchanged(&mux, seq, revision, next);

    // With no identity left, a new shape is refused before any change,
    // while a same-shape arrangement still applies.
    mux.reserve_through(u64::MAX);
    let next = mux.next_id;
    assert!(matches!(
        mux.arrange_panes(workspace, tab, &current, &rows),
        Err(MuxError::IdExhausted)
    ));
    unchanged(&mux, seq, revision, next);
    let (split, children) = root(&mux, tab);
    mux.resize_split(workspace, tab, split, &children, &[300_000, 700_000])
        .unwrap();
    let resized = layout(&mux, tab);
    mux.arrange_panes(workspace, tab, &resized, &resized.balanced())
        .unwrap();
    assert_eq!(layout(&mux, tab), current);
    assert_eq!(mux.next_id, next);
    other.shutdown().unwrap();
    mux.shutdown().unwrap();
}

#[test]
fn a_weights_only_change_by_another_writer_passes_the_guard() {
    let mut mux = Mux::default();
    let (workspace, tab, _) = three_panes(&mut mux, &command("read value"));
    let expected = layout(&mux, tab);
    let tiled = built_in(&mux, tab, "tiled");
    // Another window drags the root divider after this one read the
    // layout.
    let (split, children) = root(&mux, tab);
    mux.resize_split(workspace, tab, split, &children, &[200_000, 800_000])
        .unwrap();
    let resized = layout(&mux, tab);
    let change = mux
        .arrange_panes(workspace, tab, &expected, &tiled)
        .unwrap();
    assert_eq!(change.before[0].record.as_ref().unwrap().layout, resized);
    assert_holds(&mux, tab, &tiled);
    mux.shutdown().unwrap();
}

#[test]
fn a_change_clears_the_shared_zoom_in_its_only_event() {
    let mut mux = Mux::default();
    let (workspace, tab, [_, second, _]) =
        three_panes(&mut mux, &command("read value"));
    let (_, subscription) = mux.subscribe_hierarchy();
    let only_event = |mux: &Mux| {
        let envelope = subscription.try_recv().unwrap();
        assert_eq!(envelope.seq, mux.hierarchy_seq());
        assert_eq!(
            envelope.event,
            HierarchyEvent::PaneLayoutChanged {
                tab,
                layout: layout(mux, tab),
                zoomed: None,
            }
        );
        assert_eq!(subscription.try_recv(), Err(HierarchyRecvError::Empty));
    };
    for name in ["main_top", "balance"] {
        mux.set_pane_zoom(workspace, tab, Some(second)).unwrap();
        subscription.try_recv().unwrap();
        let expected = layout(&mux, tab);
        let arrangement = if name == "balance" {
            expected.balanced()
        } else {
            built_in(&mux, tab, name)
        };
        let change = mux
            .arrange_panes(workspace, tab, &expected, &arrangement)
            .unwrap();
        // The recorded before keeps the zoom for undo.
        assert_eq!(
            change.before[0].record.as_ref().unwrap().zoomed,
            Some(second)
        );
        assert_eq!(change.after[0].record.as_ref().unwrap().zoomed, None);
        assert_eq!(mux.tab(tab).unwrap().zoomed(), None);
        only_event(&mux);
    }
    mux.shutdown().unwrap();
}
