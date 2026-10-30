use super::*;
use crate::mux::live_fixture::{
    ECHO, Live, beside, opened_tab, pane_name, pane_ref,
};
use crate::mux::tests::command;
use crate::test_support::TestClient;
use crate::{CloseRequest, MuxError};

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "walk four live shells through every move path in one runtime"
)]
fn every_move_path_keeps_live_shells_their_identities_and_history() {
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
    let two_id = four.tab.id;
    let p4 = Live::new("pd", four.tab.first_pane(), &four.client);
    let panes = [&p1, &p2, &p3, &p4];
    let check = |mux: &Mux, step: usize| {
        assert_eq!(mux.terminal_count(), 4);
        for pane in panes {
            pane.answers(mux, step);
        }
    };
    check(&mux, 0);

    // A swap within the tab.
    let change = mux.swap_panes(alpha, one_id, p1.pane, p3.pane).unwrap();
    assert_eq!(change.before.len(), 1);
    check(&mux, 1);
    // A move within the tab.
    mux.move_pane(
        pane_ref(&mux, p3.pane),
        beside(&mux, p2.pane, SplitDirection::Left),
    )
    .unwrap();
    check(&mux, 2);
    // A move into another tab of the same session revokes no viewer.
    let watcher = TestClient::new(&mux.terminals[&p2.terminal]);
    mux.move_pane(
        pane_ref(&mux, p2.pane),
        beside(&mux, p4.pane, SplitDirection::Right),
    )
    .unwrap();
    assert!(!watcher.poll().revoked);
    assert_eq!(mux.select_pane(p2.pane).unwrap().tab, Some(two_id));
    check(&mux, 3);
    // An extraction into a new tab.
    let change = mux
        .move_pane(
            pane_ref(&mux, p3.pane),
            MoveDestination::NewTab {
                workspace: alpha,
                before: None,
            },
        )
        .unwrap();
    let three_id = opened_tab(&change);
    assert_eq!(mux.select_pane(p3.pane).unwrap().tab, Some(three_id));
    check(&mux, 4);
    // Tab two loses both panes; the last one's move removes it.
    mux.move_pane(
        pane_ref(&mux, p4.pane),
        beside(&mux, p1.pane, SplitDirection::Down),
    )
    .unwrap();
    let change = mux
        .move_pane(
            pane_ref(&mux, p2.pane),
            beside(&mux, p4.pane, SplitDirection::Right),
        )
        .unwrap();
    assert!(mux.tab(two_id).is_none());
    assert_eq!(
        change
            .before
            .iter()
            .map(|snapshot| (snapshot.tab, snapshot.record.is_some()))
            .collect::<Vec<_>>(),
        [(two_id, true), (one_id, true)]
    );
    assert_eq!(
        change
            .after
            .iter()
            .map(|snapshot| (snapshot.tab, snapshot.record.is_some()))
            .collect::<Vec<_>>(),
        [(two_id, false), (one_id, true)]
    );
    check(&mux, 5);
    // A lone pane extracted into another workspace moves its whole tab.
    mux.move_pane(
        pane_ref(&mux, p3.pane),
        MoveDestination::NewTab {
            workspace: beta,
            before: None,
        },
    )
    .unwrap();
    assert_eq!(mux.select_tab(three_id).unwrap().workspace, Some(beta));
    check(&mux, 6);
    // A move into another session revokes the pane's viewers.
    let watcher = TestClient::new(&mux.terminals[&p1.terminal]);
    mux.move_pane(
        pane_ref(&mux, p1.pane),
        MoveDestination::NewTab {
            workspace: gamma,
            before: None,
        },
    )
    .unwrap();
    assert!(watcher.poll().revoked);
    assert_eq!(mux.select_pane(p1.pane).unwrap().session, second);
    check(&mux, 7);
    // Tab one, now [4 | 2], drops whole into tab three beside pane 3.
    let change = mux
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
    assert_eq!(change.after[0].record, None);
    let layout = mux.tab(three_id).unwrap().layout();
    assert_eq!(layout.pane_count(), 3);
    for pane in [&p2, &p3, &p4] {
        assert!(layout.pane(pane.pane).is_some(), "{} left", pane.name);
    }
    check(&mux, 8);
    mux.shutdown().unwrap();
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "compare one runtime before and after every no-op and refusal"
)]
fn no_ops_and_refusals_change_nothing_and_swaps_keep_the_revision() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let alpha = mux.create_workspace(session, None).unwrap();
    let idle = command("read value");
    let tab = mux.open_tab(alpha, &idle).unwrap().tab;
    let left = tab.first_pane().id;
    let right = mux
        .split_pane(alpha, tab.id, left, SplitDirection::Right, &idle)
        .unwrap()
        .pane
        .id;
    let lone = mux.open_tab(alpha, &idle).unwrap().tab;
    let lone_pane = lone.first_pane().id;
    let unchanged = |mux: &Mux, seq: u64, revision: u64, next: u64| {
        assert_eq!(mux.hierarchy_seq(), seq);
        assert_eq!(mux.revision, revision);
        assert_eq!(mux.next_id, next);
    };

    // A swap emits its layout but leaves the close revision alone.
    let (seq, revision) = (mux.hierarchy_seq(), mux.revision);
    assert!(
        !mux.swap_panes(alpha, tab.id, left, right)
            .unwrap()
            .is_empty()
    );
    assert_eq!(mux.hierarchy_seq(), seq + 1);
    assert_eq!(mux.revision, revision);
    mux.swap_panes(alpha, tab.id, left, right).unwrap();

    let (seq, revision, next) =
        (mux.hierarchy_seq(), mux.revision, mux.next_id);
    let layout = mux.tab(tab.id).unwrap().clone();
    let no_ops = [
        mux.swap_panes(alpha, tab.id, left, left),
        mux.move_pane(
            pane_ref(&mux, left),
            beside(&mux, left, SplitDirection::Right),
        ),
        // Moving 1 to the left of 2 in [1 | 2] has the same shape.
        mux.move_pane(
            pane_ref(&mux, left),
            beside(&mux, right, SplitDirection::Left),
        ),
        mux.move_pane(
            pane_ref(&mux, lone_pane),
            MoveDestination::NewTab {
                workspace: alpha,
                before: None,
            },
        ),
    ];
    for result in no_ops {
        assert!(result.unwrap().is_empty());
    }
    unchanged(&mux, seq, revision, next);
    assert_eq!(*mux.tab(tab.id).unwrap(), layout);

    // A tab into one of its own panes.
    assert!(matches!(
        mux.move_tab_to_pane(
            alpha,
            tab.id,
            Beside {
                target: pane_ref(&mux, right),
                side: SplitDirection::Right,
            },
        ),
        Err(MuxError::InvalidLayout(LayoutError::SelfTarget))
    ));
    // A stale reference: the pane is in another tab.
    let stale = PaneRef {
        tab: lone.id,
        ..pane_ref(&mux, left)
    };
    assert!(matches!(
        mux.move_pane(stale, beside(&mux, right, SplitDirection::Down)),
        Err(MuxError::UnknownPane(_))
    ));
    // A destination in another runtime with the same socket name.
    let mut other = Mux::default();
    let foreign_session = other.create_session(None).unwrap();
    let foreign = other.create_workspace(foreign_session, None).unwrap();
    assert!(matches!(
        mux.move_pane(
            pane_ref(&mux, left),
            MoveDestination::NewTab {
                workspace: foreign,
                before: None,
            },
        ),
        Err(MuxError::ForeignRuntime(_))
    ));
    unchanged(&mux, seq, revision, next);

    // With no identity left, a move that needs one is refused before it
    // changes anything, the close revision included.
    mux.reserve_through(u64::MAX);
    let next = mux.next_id;
    assert!(matches!(
        mux.move_pane(
            pane_ref(&mux, left),
            MoveDestination::NewTab {
                workspace: alpha,
                before: None,
            },
        ),
        Err(MuxError::IdExhausted)
    ));
    // Pane 1 below the lone pane needs a new split.
    assert!(matches!(
        mux.move_pane(
            pane_ref(&mux, left),
            beside(&mux, lone_pane, SplitDirection::Down)
        ),
        Err(MuxError::IdExhausted)
    ));
    // So does a tab dropped below a pane of a column split.
    assert!(matches!(
        mux.move_tab_to_pane(
            alpha,
            lone.id,
            Beside {
                target: pane_ref(&mux, left),
                side: SplitDirection::Down,
            },
        ),
        Err(MuxError::IdExhausted)
    ));
    unchanged(&mux, seq, revision, next);
    assert_eq!(*mux.tab(tab.id).unwrap(), layout);
    assert_eq!(*mux.tab(lone.id).unwrap(), lone);
    // A move that joins an existing split needs no identity, so it still
    // works.
    mux.move_pane(
        pane_ref(&mux, lone_pane),
        beside(&mux, right, SplitDirection::Right),
    )
    .unwrap();
    assert_eq!(mux.tab(tab.id).unwrap().layout().pane_count(), 3);
    assert_eq!(mux.next_id, next);
    mux.shutdown().unwrap();
}

#[test]
fn moves_keep_unrelated_zoom_and_clear_zoom_they_disturb() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let alpha = mux.create_workspace(session, None).unwrap();
    let idle = command("read value");
    let split = |mux: &mut Mux, tab: TabId, pane: PaneId| {
        mux.split_pane(alpha, tab, pane, SplitDirection::Right, &idle)
            .unwrap()
            .pane
            .id
    };
    // Tab one: [pa | pb | pc]; tab two: [pd | pe].
    let one = mux.open_tab(alpha, &idle).unwrap().tab.id;
    let pa = mux.tab(one).unwrap().first_pane().id;
    let pb = split(&mut mux, one, pa);
    let pc = split(&mut mux, one, pb);
    let two = mux.open_tab(alpha, &idle).unwrap().tab.id;
    let pd = mux.tab(two).unwrap().first_pane().id;
    let pe = split(&mut mux, two, pd);
    let zoom = |mux: &Mux, tab: TabId| mux.tab(tab).unwrap().zoomed();

    // A swap keeps the zoom on its pane.
    mux.set_pane_zoom(alpha, one, Some(pa)).unwrap();
    mux.swap_panes(alpha, one, pa, pb).unwrap();
    assert_eq!(zoom(&mux, one), Some(pa));
    // Pane pc leaves: tab one keeps its unrelated zoom, and tab two, which
    // takes pc, loses its own.
    mux.set_pane_zoom(alpha, two, Some(pd)).unwrap();
    mux.move_pane(pane_ref(&mux, pc), beside(&mux, pe, SplitDirection::Right))
        .unwrap();
    assert_eq!(zoom(&mux, one), Some(pa));
    assert_eq!(zoom(&mux, two), None);
    // An extraction that leaves two panes keeps an unrelated zoom.
    mux.set_pane_zoom(alpha, two, Some(pd)).unwrap();
    mux.move_pane(
        pane_ref(&mux, pe),
        MoveDestination::NewTab {
            workspace: alpha,
            before: None,
        },
    )
    .unwrap();
    assert_eq!(zoom(&mux, two), Some(pd));
    // The zoomed pane leaving clears its tab's zoom, and so does pa tab
    // left with one pane.
    mux.move_pane(pane_ref(&mux, pd), beside(&mux, pa, SplitDirection::Left))
        .unwrap();
    assert_eq!(zoom(&mux, two), None);
    mux.move_pane(pane_ref(&mux, pb), beside(&mux, pc, SplitDirection::Right))
        .unwrap();
    assert_eq!(mux.tab(one).unwrap().layout().pane_count(), 2);
    mux.set_pane_zoom(alpha, two, Some(pb)).unwrap();
    mux.move_pane(pane_ref(&mux, pc), beside(&mux, pa, SplitDirection::Right))
        .unwrap();
    assert_eq!(mux.tab(two).unwrap().layout().pane_count(), 1);
    assert_eq!(zoom(&mux, two), None);
    // A move within pa tab clears its zoom, as pa split does.
    mux.set_pane_zoom(alpha, one, Some(pa)).unwrap();
    mux.move_pane(pane_ref(&mux, pd), beside(&mux, pc, SplitDirection::Right))
        .unwrap();
    assert_eq!(zoom(&mux, one), None);
    mux.shutdown().unwrap();
}

#[test]
fn cross_session_moves_revoke_only_the_moved_terminals_viewers() {
    let mut mux = Mux::default();
    let first = mux.create_session(None).unwrap();
    let alpha = mux.create_workspace(first, None).unwrap();
    let second = mux.create_session(None).unwrap();
    let beta = mux.create_workspace(second, None).unwrap();
    let idle = command("read value");
    let one = mux.open_tab(alpha, &idle).unwrap().tab;
    let p = one.first_pane().clone();
    let q = mux
        .split_pane(alpha, one.id, p.id, SplitDirection::Right, &idle)
        .unwrap()
        .pane;
    let lone = mux.open_tab(alpha, &idle).unwrap().tab;
    let s = lone.first_pane().clone();
    let other = mux.open_tab(beta, &idle).unwrap().tab;
    let r = other.first_pane().clone();
    let watch = |mux: &Mux, pane: &PaneInfo| {
        TestClient::new(&mux.terminals[&pane.terminal_id])
    };
    let (wp, wq, wr, ws) = (
        watch(&mux, &p),
        watch(&mux, &q),
        watch(&mux, &r),
        watch(&mux, &s),
    );
    // A pane moved beside a pane of another session.
    mux.move_pane(
        pane_ref(&mux, p.id),
        beside(&mux, r.id, SplitDirection::Down),
    )
    .unwrap();
    assert!(wp.poll().revoked);
    assert!(!wq.poll().revoked && !wr.poll().revoked && !ws.poll().revoked);
    // A lone pane's tab placed in another session's workspace.
    mux.move_pane(
        pane_ref(&mux, s.id),
        MoveDestination::NewTab {
            workspace: beta,
            before: None,
        },
    )
    .unwrap();
    assert!(ws.poll().revoked);
    assert!(!wq.poll().revoked && !wr.poll().revoked);
    // A whole tab dropped into another session's tab.
    mux.move_tab_to_pane(
        alpha,
        one.id,
        Beside {
            target: pane_ref(&mux, r.id),
            side: SplitDirection::Right,
        },
    )
    .unwrap();
    assert!(wq.poll().revoked);
    assert!(!wr.poll().revoked);
    mux.shutdown().unwrap();
}

#[test]
fn moves_make_close_assessments_stale_and_swaps_do_not() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let alpha = mux.create_workspace(session, None).unwrap();
    let idle = command("read value");
    let tab = mux.open_tab(alpha, &idle).unwrap().tab.id;
    let left = mux.tab(tab).unwrap().first_pane().id;
    let right = mux
        .split_pane(alpha, tab, left, SplitDirection::Right, &idle)
        .unwrap()
        .pane
        .id;
    let other = mux.open_tab(alpha, &idle).unwrap().tab;
    let close = CloseRequest::Tab {
        workspace: alpha,
        tab,
    };
    let assessment = mux.prepare_close(close.clone()).unwrap().check_jobs();
    mux.swap_panes(alpha, tab, left, right).unwrap();
    // The swap changed no terminal the close covers.
    assert!(!matches!(
        mux.validate_close(&assessment, &assessment.recheck(), true),
        Err(MuxError::StaleClose)
    ));
    mux.move_pane(
        pane_ref(&mux, left),
        beside(&mux, right, SplitDirection::Down),
    )
    .unwrap();
    assert!(matches!(
        mux.validate_close(&assessment, &assessment.recheck(), true),
        Err(MuxError::StaleClose)
    ));
    // A pane that moves in widens the close; the stale commit is refused
    // and a new assessment covers it.
    let assessment = mux.prepare_close(close.clone()).unwrap().check_jobs();
    mux.move_pane(
        pane_ref(&mux, other.first_pane().id),
        beside(&mux, left, SplitDirection::Right),
    )
    .unwrap();
    assert!(matches!(
        mux.validate_close(&assessment, &assessment.recheck(), true),
        Err(MuxError::StaleClose)
    ));
    let assessment = mux.prepare_close(close).unwrap().check_jobs();
    assert!(
        assessment
            .terminals()
            .contains(&other.first_pane().terminal_id)
    );
    mux.shutdown().unwrap();
}

#[test]
fn custom_names_move_between_a_pane_and_its_tab() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let alpha = mux.create_workspace(session, None).unwrap();
    let script = command("read value");
    let open = |mux: &mut Mux| mux.open_tab(alpha, &script).unwrap().tab;
    let split = |mux: &mut Mux, tab: TabId, pane: PaneId| {
        mux.split_pane(alpha, tab, pane, SplitDirection::Right, &script)
            .unwrap()
            .pane
            .id
    };
    let tab_name = |mux: &Mux, tab: TabId| {
        mux.tab(tab).unwrap().custom_name().map(str::to_owned)
    };
    let new_tab = MoveDestination::NewTab {
        workspace: alpha,
        before: None,
    };
    // Tab one: [anchor | named | plain].
    let one = open(&mut mux).id;
    let anchor = mux.tab(one).unwrap().first_pane().id;
    let named = split(&mut mux, one, anchor);
    let plain = split(&mut mux, one, named);
    mux.rename_pane(named, Some("bee")).unwrap();

    // A pane extracted into a new tab gives the tab its name.
    let change = mux.move_pane(pane_ref(&mux, named), new_tab).unwrap();
    let extracted = opened_tab(&change);
    assert_eq!(tab_name(&mux, extracted).as_deref(), Some("bee"));
    assert_eq!(pane_name(&mux, named), None);
    // An unnamed pane gives its new tab no name.
    let change = mux.move_pane(pane_ref(&mux, plain), new_tab).unwrap();
    assert_eq!(tab_name(&mux, opened_tab(&change)), None);

    // A single-pane tab nested into a pane gives the pane its name.
    mux.move_tab_to_pane(
        alpha,
        extracted,
        Beside {
            target: pane_ref(&mux, anchor),
            side: SplitDirection::Right,
        },
    )
    .unwrap();
    assert!(mux.tab(extracted).is_none());
    assert_eq!(pane_name(&mux, named).as_deref(), Some("bee"));

    // A tab's name wins over its only pane's; without one the pane keeps
    // its own. Both hold for a nest and for a move of that pane.
    let two = open(&mut mux);
    let tabbed = two.first_pane().id;
    mux.rename_pane(tabbed, Some("own")).unwrap();
    mux.rename_tab(two.id, Some("two")).unwrap();
    mux.move_tab_to_pane(
        alpha,
        two.id,
        Beside {
            target: pane_ref(&mux, anchor),
            side: SplitDirection::Down,
        },
    )
    .unwrap();
    assert_eq!(pane_name(&mux, tabbed).as_deref(), Some("two"));
    let three = open(&mut mux);
    let kept = three.first_pane().id;
    mux.rename_pane(kept, Some("own")).unwrap();
    mux.move_pane(
        pane_ref(&mux, kept),
        beside(&mux, anchor, SplitDirection::Up),
    )
    .unwrap();
    assert!(mux.tab(three.id).is_none());
    assert_eq!(pane_name(&mux, kept).as_deref(), Some("own"));
    let four = open(&mut mux);
    let inherited = four.first_pane().id;
    mux.rename_tab(four.id, Some("four")).unwrap();
    mux.move_pane(
        pane_ref(&mux, inherited),
        beside(&mux, anchor, SplitDirection::Left),
    )
    .unwrap();
    assert_eq!(pane_name(&mux, inherited).as_deref(), Some("four"));

    // A multi-pane tab nested into a pane drops its own name; its panes
    // keep theirs.
    let five = open(&mut mux).id;
    let eff = mux.tab(five).unwrap().first_pane().id;
    let sibling = split(&mut mux, five, eff);
    mux.rename_pane(eff, Some("eff")).unwrap();
    mux.rename_tab(five, Some("five")).unwrap();
    mux.move_tab_to_pane(
        alpha,
        five,
        Beside {
            target: pane_ref(&mux, anchor),
            side: SplitDirection::Right,
        },
    )
    .unwrap();
    assert_eq!(pane_name(&mux, eff).as_deref(), Some("eff"));
    assert_eq!(pane_name(&mux, sibling), None);
    assert!(
        mux.tab(one)
            .unwrap()
            .layout()
            .panes()
            .iter()
            .all(|pane| pane.custom_name.as_deref() != Some("five"))
    );
    mux.shutdown().unwrap();
}
