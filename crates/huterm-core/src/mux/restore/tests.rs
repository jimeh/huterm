use super::*;
use crate::mux::live_fixture::{ECHO, Live, beside, opened_tab, pane_ref};
use crate::mux::tests::command;
use crate::mux::{Beside, MoveDestination};
use crate::test_support::TestClient;
use huterm_protocol::{PaneNode, SplitDirection, SplitId};

use RestoreDirection::{Redo, Undo};

/// The current records of the tabs `change` touched.
fn current(mux: &Mux, change: &LayoutChange) -> Vec<TabSnapshot> {
    change
        .before
        .iter()
        .map(|snapshot| mux.snapshot(snapshot.tab))
        .collect()
}

/// Undoes and redoes `change`, checking that each leaves the exact
/// recorded records and reports the change it made, then runs `check`.
fn round_trip(mux: &mut Mux, change: &LayoutChange, check: impl Fn(&Mux)) {
    assert!(!change.is_empty());
    assert_eq!(current(mux, change), change.after);
    let undone = mux.restore_tabs(change, Undo).unwrap();
    assert_eq!(current(mux, change), change.before, "undo");
    assert_eq!(undone.before, change.after);
    assert_eq!(undone.after, change.before);
    assert_eq!(undone.placed, change.placed);
    check(mux);
    let redone = mux.restore_tabs(change, Redo).unwrap();
    assert_eq!(current(mux, change), change.after, "redo");
    assert_eq!(redone.before, change.before);
    assert_eq!(redone.after, change.after);
    check(mux);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "walk four live shells through undo and redo of every move"
)]
fn undo_and_redo_of_every_move_keep_live_shells_and_exact_records() {
    let mut mux = Mux::default();
    let first = mux.create_session(None).unwrap();
    let alpha = mux.create_workspace(first, None).unwrap();
    let beta = mux.create_workspace(first, None).unwrap();
    let second = mux.create_session(None).unwrap();
    let gamma = mux.create_workspace(second, None).unwrap();
    let script = command(ECHO);

    // Tab one: [1 | (2 over 3)]; tab two: [4].
    let one = mux.open_test_tab(alpha, &script).unwrap();
    let one_id = one.tab.id;
    let p1 = Live::new("pa", one.tab.first_pane(), &one.client);
    let two = mux
        .split_test_pane(alpha, one_id, p1.pane, SplitDirection::Right, &script)
        .unwrap();
    let p2 = Live::new("pb", &two.pane, &two.client);
    let three = mux
        .split_test_pane(alpha, one_id, p2.pane, SplitDirection::Down, &script)
        .unwrap();
    let p3 = Live::new("pc", &three.pane, &three.client);
    let four = mux.open_test_tab(alpha, &script).unwrap();
    let p4 = Live::new("pd", four.tab.first_pane(), &four.client);
    let panes = [&p1, &p2, &p3, &p4];
    let step = std::cell::Cell::new(0);
    let check = |mux: &Mux| {
        step.set(step.get() + 1);
        assert_eq!(mux.terminal_count(), 4);
        for pane in panes {
            pane.answers(mux, step.get());
        }
    };
    check(&mux);

    let swap = mux.swap_panes(alpha, one_id, p1.pane, p3.pane).unwrap();
    round_trip(&mut mux, &swap, check);
    let within = mux
        .move_pane(
            pane_ref(&mux, p3.pane),
            beside(&mux, p2.pane, SplitDirection::Left),
        )
        .unwrap();
    round_trip(&mut mux, &within, check);
    let across = mux
        .move_pane(
            pane_ref(&mux, p2.pane),
            beside(&mux, p4.pane, SplitDirection::Right),
        )
        .unwrap();
    round_trip(&mut mux, &across, check);
    let extraction = mux
        .move_pane(
            pane_ref(&mux, p3.pane),
            MoveDestination::NewTab {
                workspace: alpha,
                before: Some(one_id),
            },
        )
        .unwrap();
    let extracted = opened_tab(&extraction);
    round_trip(&mut mux, &extraction, check);
    // Tab two, now [4 | 2], gives up pane 2 and then its last pane, which
    // removes it; undo reopens it with its own identity.
    let two_id = four.tab.id;
    mux.move_pane(
        pane_ref(&mux, p2.pane),
        beside(&mux, p1.pane, SplitDirection::Down),
    )
    .unwrap();
    let emptied = mux
        .move_pane(
            pane_ref(&mux, p4.pane),
            beside(&mux, p1.pane, SplitDirection::Up),
        )
        .unwrap();
    assert!(mux.tab(two_id).is_none());
    round_trip(&mut mux, &emptied, check);
    // A lone pane extracted into another workspace moves its whole tab,
    // the one change that places a tab.
    let lone = mux
        .move_pane(
            pane_ref(&mux, p3.pane),
            MoveDestination::NewTab {
                workspace: beta,
                before: None,
            },
        )
        .unwrap();
    assert_eq!(lone.placed, [extracted]);
    for change in [&swap, &within, &across, &extraction, &emptied] {
        assert_eq!(change.placed, []);
    }
    round_trip(&mut mux, &lone, check);
    // Undo of a move into another session revokes the pane's viewers, as
    // the move did.
    let into_gamma = mux
        .move_pane(
            pane_ref(&mux, p1.pane),
            MoveDestination::NewTab {
                workspace: gamma,
                before: None,
            },
        )
        .unwrap();
    let watcher = TestClient::new(&mux.terminals[&p1.terminal]);
    mux.restore_tabs(&into_gamma, Undo).unwrap();
    assert!(watcher.poll().revoked);
    assert_eq!(mux.select_pane(p1.pane).unwrap().session, first);
    mux.restore_tabs(&into_gamma, Redo).unwrap();
    round_trip(&mut mux, &into_gamma, check);
    // Tab one drops whole into the extracted tab, now in beta.
    let dropped = mux
        .move_tab_to_pane(
            alpha,
            one_id,
            Beside {
                target: pane_ref(&mux, p3.pane),
                side: SplitDirection::Down,
            },
        )
        .unwrap();
    assert!(mux.tab(one_id).is_none());
    round_trip(&mut mux, &dropped, check);
    mux.restore_tabs(&dropped, Undo).unwrap();
    assert_eq!(mux.select_tab(one_id).unwrap().workspace, Some(alpha));
    check(&mux);
    mux.shutdown().unwrap();
}

/// A runtime with one workspace of idle panes.
fn idle_runtime() -> (Mux, WorkspaceId) {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let alpha = mux.create_workspace(session, None).unwrap();
    (mux, alpha)
}

fn open(mux: &mut Mux, workspace: WorkspaceId) -> (TabId, PaneId) {
    let tab = mux.open_tab(workspace, &command("read value")).unwrap().tab;
    (tab.id, tab.first_pane().id)
}

fn split(
    mux: &mut Mux,
    workspace: WorkspaceId,
    tab: TabId,
    pane: PaneId,
) -> PaneId {
    mux.split_pane(
        workspace,
        tab,
        pane,
        SplitDirection::Right,
        &command("read value"),
    )
    .unwrap()
    .pane
    .id
}

#[test]
fn undo_keeps_later_names_reorders_and_the_removed_tab_identity() {
    let (mut mux, alpha) = idle_runtime();
    let (first, _) = open(&mut mux, alpha);
    let (dropped, _) = open(&mut mux, alpha);
    let (target, pane) = open(&mut mux, alpha);
    let lone = mux.tab(dropped).unwrap().first_pane().id;
    split(&mut mux, alpha, dropped, lone);
    mux.rename_tab(dropped, Some("dropped")).unwrap();
    let change = mux
        .move_tab_to_pane(
            alpha,
            dropped,
            Beside {
                target: pane_ref(&mux, pane),
                side: SplitDirection::Down,
            },
        )
        .unwrap();
    // Another client renames the receiving tab and moves it to the front,
    // which the guard ignores because the tab is not in `placed`.
    mux.rename_tab(target, Some("renamed")).unwrap();
    mux.reorder_tab(alpha, target, Some(first)).unwrap();
    mux.restore_tabs(&change, Undo).unwrap();
    let order: Vec<TabId> = mux
        .workspace(alpha)
        .unwrap()
        .tabs
        .iter()
        .map(|t| t.id)
        .collect();
    // The reopened tab returns to its recorded position, 1.
    assert_eq!(order, [target, dropped, first]);
    assert_eq!(mux.tab(target).unwrap().custom_name(), Some("renamed"));
    assert_eq!(mux.tab(dropped).unwrap().custom_name(), Some("dropped"));
    assert_eq!(mux.tab(target).unwrap().layout().pane_count(), 1);
    assert_eq!(mux.tab(dropped).unwrap().layout().pane_count(), 2);
    mux.shutdown().unwrap();
}

#[test]
fn undo_keeps_a_later_zoom_and_restores_one_the_change_cleared() {
    let (mut mux, alpha) = idle_runtime();
    let (tab, a) = open(&mut mux, alpha);
    let b = split(&mut mux, alpha, tab, a);
    let c = split(&mut mux, alpha, tab, b);
    let zoom = |mux: &Mux, tab: TabId| mux.tab(tab).unwrap().zoomed();

    // The move clears the zoom on a; undo brings it back.
    mux.set_pane_zoom(alpha, tab, Some(a)).unwrap();
    let change = mux
        .move_pane(pane_ref(&mux, c), beside(&mux, a, SplitDirection::Left))
        .unwrap();
    assert_eq!(zoom(&mux, tab), None);
    mux.restore_tabs(&change, Undo).unwrap();
    assert_eq!(zoom(&mux, tab), Some(a));
    // A zoom another client set after the move survives the undo.
    mux.restore_tabs(&change, Redo).unwrap();
    mux.set_pane_zoom(alpha, tab, Some(b)).unwrap();
    mux.restore_tabs(&change, Undo).unwrap();
    assert_eq!(zoom(&mux, tab), Some(b));

    // A pane moved beside lone pane d, then a zoom on d: undo leaves d's
    // tab with one pane, so the zoom clears.
    let (lone, d) = open(&mut mux, alpha);
    let change = mux
        .move_pane(pane_ref(&mux, c), beside(&mux, d, SplitDirection::Right))
        .unwrap();
    mux.set_pane_zoom(alpha, lone, Some(d)).unwrap();
    mux.restore_tabs(&change, Undo).unwrap();
    assert_eq!(mux.tab(lone).unwrap().layout().pane_count(), 1);
    assert_eq!(zoom(&mux, lone), None);
    mux.shutdown().unwrap();
}

/// The root split's identity and children.
fn root_split(
    mux: &Mux,
    tab: TabId,
) -> (SplitId, Vec<huterm_protocol::NodeId>) {
    let PaneNode::Split(split) = mux.tab(tab).unwrap().layout().root() else {
        panic!("tab {tab:?} is not split");
    };
    (
        split.id,
        split.children.iter().map(|child| child.node.id()).collect(),
    )
}

#[test]
fn a_later_resize_does_not_block_undo_which_restores_earlier_weights() {
    let (mut mux, alpha) = idle_runtime();
    let (tab, a) = open(&mut mux, alpha);
    let b = split(&mut mux, alpha, tab, a);
    let c = split(&mut mux, alpha, tab, b);
    let (root, children) = root_split(&mux, tab);
    mux.resize_split(alpha, tab, root, &children, &[200_000, 300_000, 500_000])
        .unwrap();
    let weighted = mux.tab(tab).unwrap().layout().clone();
    let change = mux.swap_panes(alpha, tab, a, c).unwrap();
    let (root, children) = root_split(&mux, tab);
    mux.resize_split(alpha, tab, root, &children, &[600_000, 300_000, 100_000])
        .unwrap();
    mux.restore_tabs(&change, Undo).unwrap();
    assert_eq!(*mux.tab(tab).unwrap().layout(), weighted);
    mux.shutdown().unwrap();
}

#[test]
fn a_change_of_weights_alone_undoes_and_redoes() {
    let (mut mux, alpha) = idle_runtime();
    let (tab, a) = open(&mut mux, alpha);
    split(&mut mux, alpha, tab, a);
    // A change that keeps the shape and moves only the divider, as
    // balancing a tab does.
    let before = mux.snapshot(tab);
    let (root, children) = root_split(&mux, tab);
    mux.resize_split(alpha, tab, root, &children, &[200_000, 800_000])
        .unwrap();
    let change = LayoutChange {
        before: vec![before],
        after: vec![mux.snapshot(tab)],
        placed: Vec::new(),
    };
    let revision = mux.revision;
    round_trip(&mut mux, &change, |_| {});
    assert_eq!(mux.revision, revision);
    mux.shutdown().unwrap();
}

#[test]
fn restores_of_swaps_keep_the_revision_and_of_moves_advance_it() {
    let (mut mux, alpha) = idle_runtime();
    let (tab, a) = open(&mut mux, alpha);
    let b = split(&mut mux, alpha, tab, a);
    let (other, c) = open(&mut mux, alpha);
    let swap = mux.swap_panes(alpha, tab, a, b).unwrap();
    let revision = mux.revision;
    mux.restore_tabs(&swap, Undo).unwrap();
    mux.restore_tabs(&swap, Redo).unwrap();
    assert_eq!(mux.revision, revision);
    let across = mux
        .move_pane(pane_ref(&mux, c), beside(&mux, a, SplitDirection::Down))
        .unwrap();
    assert!(mux.tab(other).is_none());
    let revision = mux.revision;
    mux.restore_tabs(&across, Undo).unwrap();
    assert!(mux.revision > revision);
    mux.shutdown().unwrap();
}

#[test]
fn restores_of_a_tab_placed_in_another_session_advance_and_revoke() {
    let (mut mux, alpha) = idle_runtime();
    let other = mux.create_session(None).unwrap();
    let beta = mux.create_workspace(other, None).unwrap();
    let (tab, pane) = open(&mut mux, alpha);
    let placed = mux
        .move_pane(
            pane_ref(&mux, pane),
            MoveDestination::NewTab {
                workspace: beta,
                before: None,
            },
        )
        .unwrap();
    assert_eq!(placed.placed, [tab]);
    let terminal = mux.tab(tab).unwrap().first_pane().terminal_id;
    for (direction, workspace) in [(Undo, alpha), (Redo, beta)] {
        let watcher = TestClient::new(&mux.terminals[&terminal]);
        let revision = mux.revision;
        mux.restore_tabs(&placed, direction).unwrap();
        assert_eq!(mux.select_tab(tab).unwrap().workspace, Some(workspace));
        assert!(mux.revision > revision, "{direction:?} kept the revision");
        assert!(watcher.poll().revoked, "{direction:?} kept the viewer");
    }
    mux.shutdown().unwrap();
}

/// `layout` with its root split's identity replaced.
fn with_root_split(layout: &PaneLayout, id: SplitId) -> PaneLayout {
    let PaneNode::Split(mut split) = layout.root().clone() else {
        panic!("layout is not split");
    };
    split.id = id;
    PaneLayout::new(PaneNode::Split(split)).unwrap()
}

/// `change` with the record of the tab at `index` on both sides edited.
fn edited(
    change: &LayoutChange,
    index: usize,
    edit: impl Fn(&mut TabSnapshot),
) -> LayoutChange {
    let mut change = change.clone();
    edit(&mut change.before[index]);
    edit(&mut change.after[index]);
    change
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "compare one runtime before and after every refused restore"
)]
fn refused_restores_change_nothing() {
    let mut mux = Mux::default();
    let first = mux.create_session(None).unwrap();
    let alpha = mux.create_workspace(first, None).unwrap();
    let beta = mux.create_workspace(first, None).unwrap();
    let second = mux.create_session(None).unwrap();
    let gamma = mux.create_workspace(second, None).unwrap();
    let (tab, a) = open(&mut mux, alpha);
    let b = split(&mut mux, alpha, tab, a);
    let (moved, c) = open(&mut mux, alpha);
    let swap = mux.swap_panes(alpha, tab, a, b).unwrap();
    // Tab `dropped` from beta nests into `tab`; then beta closes.
    let (dropped, _) = open(&mut mux, beta);
    let drop = mux
        .move_tab_to_pane(
            beta,
            dropped,
            Beside {
                target: pane_ref(&mux, a),
                side: SplitDirection::Down,
            },
        )
        .unwrap();
    mux.close_workspace(beta).unwrap();
    // Another client moves tab `moved` into another session after a move
    // inside it.
    let d = split(&mut mux, alpha, moved, c);
    let within = mux
        .move_pane(pane_ref(&mux, d), beside(&mux, c, SplitDirection::Left))
        .unwrap();
    mux.move_tab(alpha, moved, gamma, None).unwrap();
    let state = mux.capture_hierarchy();
    let marks = (mux.hierarchy_seq(), mux.revision, mux.next_id);
    let refused = |mux: &mut Mux, change: &LayoutChange, expected: &str| {
        let error = mux.restore_tabs(change, Undo).unwrap_err();
        assert_eq!(format!("{error:?}"), expected);
        assert_eq!(mux.capture_hierarchy(), state);
        assert_eq!((mux.hierarchy_seq(), mux.revision, mux.next_id), marks);
    };

    // The swapped tab changed shape when the tab dropped into it.
    refused(&mut mux, &swap, "LayoutChanged");
    // Undoing the drop would reopen a tab in a closed workspace.
    let current = mux.snapshot(tab);
    let drop_now = LayoutChange {
        after: vec![mux.snapshot(dropped), current.clone()],
        ..drop.clone()
    };
    refused(&mut mux, &drop_now, "LayoutChanged");
    // The tab moved to another session.
    refused(&mut mux, &within, "LayoutChanged");
    let unknown = mux.next_id + 1;
    // A tab identity never allocated.
    let forged = edited(&within, 0, |snapshot| {
        snapshot.tab = TabId::in_runtime(mux.runtime_id, unknown);
    });
    refused(&mut mux, &forged, "InvalidRestore");
    // A split identity never allocated, in a change that matches now.
    let record = current.record.clone().unwrap();
    let now = LayoutChange {
        before: vec![TabSnapshot {
            tab,
            record: Some(TabRecord {
                layout: with_root_split(
                    &record.layout,
                    SplitId::in_runtime(mux.runtime_id, unknown),
                ),
                ..record.clone()
            }),
        }],
        after: vec![current.clone()],
        placed: Vec::new(),
    };
    refused(&mut mux, &now, "InvalidRestore");
    // A pane identity never allocated, which also breaks conservation.
    let pane = record.layout.first_pane().clone();
    let lone = LayoutChange {
        before: vec![TabSnapshot {
            tab,
            record: Some(TabRecord {
                layout: PaneLayout::single(huterm_protocol::PaneInfo {
                    id: PaneId::in_runtime(mux.runtime_id, unknown),
                    ..pane.clone()
                }),
                ..record.clone()
            }),
        }],
        ..now.clone()
    };
    refused(&mut mux, &lone, "InvalidRestore");
    // Panes that leave the touched tabs break conservation.
    let shrunk = LayoutChange {
        before: vec![TabSnapshot {
            tab,
            record: Some(TabRecord {
                layout: PaneLayout::single(pane),
                ..record.clone()
            }),
        }],
        ..now.clone()
    };
    refused(&mut mux, &shrunk, "InvalidRestore");
    // Reopening a tab whose identity is in use: the guard sees it present.
    let reopened = LayoutChange {
        before: vec![TabSnapshot {
            tab: moved,
            record: drop.before[0].record.clone(),
        }],
        after: vec![TabSnapshot {
            tab: moved,
            record: None,
        }],
        placed: Vec::new(),
    };
    refused(&mut mux, &reopened, "LayoutChanged");
    // Sides that name different tabs, or one tab twice.
    let mismatched = LayoutChange {
        after: Vec::new(),
        ..now.clone()
    };
    refused(&mut mux, &mismatched, "InvalidRestore");
    let doubled = LayoutChange {
        before: vec![current.clone(), current.clone()],
        after: vec![current.clone(), current.clone()],
        placed: Vec::new(),
    };
    refused(&mut mux, &doubled, "InvalidRestore");
    // A restored split that a tab the restore does not touch holds.
    let (held, _) = root_split(&mux, moved);
    let collides = LayoutChange {
        before: vec![TabSnapshot {
            tab,
            record: Some(TabRecord {
                layout: with_root_split(&record.layout, held),
                ..record.clone()
            }),
        }],
        ..now.clone()
    };
    refused(&mut mux, &collides, "LayoutChanged");
    // A change from another runtime.
    let mut other = Mux::default();
    let session = other.create_session(None).unwrap();
    let workspace = other.create_workspace(session, None).unwrap();
    let (foreign, _) = open(&mut other, workspace);
    let foreign = LayoutChange {
        before: vec![other.snapshot(foreign)],
        after: vec![other.snapshot(foreign)],
        placed: Vec::new(),
    };
    refused(
        &mut mux,
        &foreign,
        &format!("ForeignRuntime({:?})", other.runtime_id),
    );
    other.shutdown().unwrap();
    mux.shutdown().unwrap();
}
