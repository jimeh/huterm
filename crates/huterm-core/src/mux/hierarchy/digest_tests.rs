//! Digests at emission: [`Mux::hierarchy_digest`] and the digesting
//! subscription.

use std::time::{Duration, Instant};

use huterm_protocol::{
    ApplyOutcome, MAX_HIERARCHY_ITEMS, MAX_NAME_BYTES, PaneId, PaneInfo,
    PaneLayout, PaneNode, SPLIT_TOTAL, SessionId, SplitAxis, SplitChild,
    SplitDirection, SplitId, SplitNode, TabId, TerminalId,
};

use super::*;
use crate::mux::tests::command;

const BASE: DigestProfile = DigestProfile::Base;

/// Renames `session` until the hierarchy reaches sequence `seq`, each
/// rename to a name it has not held, so each emits one event.
fn rename_through(mux: &mut Mux, session: SessionId, seq: u64) {
    while mux.hierarchy_seq() < seq {
        let name = format!("renamed at {}", mux.hierarchy_seq());
        mux.rename_session(session, Some(&name)).unwrap();
    }
}

#[test]
fn the_mux_digest_equals_the_snapshot_digest_after_mixed_operations() {
    let mut mux = Mux::default();
    let check = |mux: &Mux| {
        assert_eq!(
            mux.hierarchy_digest(BASE),
            mux.hierarchy_state().digest_for(BASE),
            "at sequence {}",
            mux.hierarchy_seq()
        );
    };
    check(&mux);
    let first = mux.create_session(Some("main")).unwrap();
    let second = mux.create_session(None).unwrap();
    let alpha = mux.create_workspace(first, None).unwrap();
    let beta = mux.create_workspace(first, Some("beta")).unwrap();
    mux.attach_session(second).unwrap();
    let attachment = mux.attach_session(first).unwrap();
    check(&mux);
    let one = mux.open_tab(alpha, &command("read value")).unwrap().tab;
    let two = mux.open_tab(alpha, &command("read value")).unwrap().tab;
    let left = one.first_pane().id;
    let right = mux
        .split_pane(
            alpha,
            one.id,
            left,
            SplitDirection::Right,
            &command("read value"),
        )
        .unwrap()
        .pane
        .id;
    check(&mux);
    mux.rename_tab(one.id, Some("one")).unwrap();
    mux.rename_pane(right, Some("right")).unwrap();
    mux.set_workspace_pinned(beta, true).unwrap();
    mux.set_input_sync(alpha, one.id, true).unwrap();
    mux.set_pane_zoom(alpha, one.id, Some(right)).unwrap();
    check(&mux);
    mux.move_tab(alpha, two.id, beta, None).unwrap();
    mux.move_session(second, Some(first)).unwrap();
    mux.retarget_attachment(attachment, second).unwrap();
    mux.swap_panes(alpha, one.id, left, right).unwrap();
    check(&mux);
    mux.close_pane(alpha, one.id, left).unwrap();
    mux.move_workspace(first, beta, second, None).unwrap();
    mux.detach_session(attachment).unwrap();
    check(&mux);
    let (state, digest, _subscription) =
        mux.subscribe_hierarchy_with_digests(BASE);
    assert_eq!(digest, state.digest_for(BASE));
    assert_eq!(state.seq(), mux.hierarchy_seq());
    mux.shutdown().unwrap();
    check(&mux);
}

#[test]
fn digesting_subscriptions_carry_correct_digests_exactly_at_interval_multiples()
{
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let (mut state, digest, subscription) =
        mux.subscribe_hierarchy_with_digests(BASE);
    assert_eq!(digest, state.digest());
    let (_, plain) = mux.subscribe_hierarchy();
    let subscribed = state.seq();
    // Past two multiples of the interval, the first one after a few
    // structural changes besides the renames.
    mux.create_workspace(session, Some("work")).unwrap();
    mux.attach_session(session).unwrap();
    rename_through(&mut mux, session, 2 * HIERARCHY_DIGEST_INTERVAL + 3);
    let mut digested = Vec::new();
    loop {
        match subscription.try_recv() {
            Ok(DigestedEnvelope { envelope, digest }) => {
                let seq = envelope.seq;
                assert!(matches!(
                    state.apply(envelope),
                    ApplyOutcome::Applied(_)
                ));
                if seq.is_multiple_of(HIERARCHY_DIGEST_INTERVAL) {
                    assert_eq!(digest, Some(state.digest()), "at {seq}");
                    digested.push(seq);
                } else {
                    assert_eq!(digest, None, "at {seq}");
                }
            }
            Err(HierarchyRecvError::Empty) => break,
            Err(error) => panic!("unexpected {error}"),
        }
    }
    assert_eq!(state.seq(), mux.hierarchy_seq());
    assert_eq!(
        digested,
        [HIERARCHY_DIGEST_INTERVAL, 2 * HIERARCHY_DIGEST_INTERVAL]
    );
    // The plain subscription follows the same events.
    let mut plain_events = 0;
    while plain.try_recv().is_ok() {
        plain_events += 1;
    }
    assert_eq!(plain_events, mux.hierarchy_seq() - subscribed);
}

#[test]
fn emission_computes_one_digest_per_profile_and_none_without_a_digesting_subscription()
 {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let (_, _plain) = mux.subscribe_hierarchy();
    rename_through(&mut mux, session, HIERARCHY_DIGEST_INTERVAL + 1);
    assert_eq!(mux.hierarchy.digested, 0);
    let (_, _, first) = mux.subscribe_hierarchy_with_digests(BASE);
    let (_, _, second) = mux.subscribe_hierarchy_with_digests(BASE);
    // Stop at the next multiple, so the digest it carries is current.
    rename_through(&mut mux, session, 2 * HIERARCHY_DIGEST_INTERVAL);
    assert_eq!(mux.hierarchy.digested, 1);
    let digest = |subscription: &DigestedHierarchySubscription| {
        let mut digests = Vec::new();
        while let Ok(event) = subscription.try_recv() {
            digests.extend(event.digest);
        }
        digests
    };
    assert_eq!(digest(&first), [mux.hierarchy_digest(BASE)]);
    assert_eq!(digest(&second), [mux.hierarchy_digest(BASE)]);
}

/// A Mux with `sessions` sessions of `workspaces` workspaces of `tabs` tabs,
/// each tab four panes side by side, and one attachment per session. Every
/// custom and fallback name is `name_bytes` long. Tabs go into the
/// structure directly, with terminal identities no runtime backs.
fn fixture(
    sessions: usize,
    workspaces: usize,
    tabs: usize,
    name_bytes: usize,
) -> Mux {
    let name = "n".repeat(name_bytes);
    let mut mux = Mux::default();
    let runtime = mux.runtime_id();
    for _ in 0..sessions {
        let session = mux.create_session(Some(&name)).unwrap();
        mux.attach_session(session).unwrap();
        for _ in 0..workspaces {
            let workspace = mux.create_workspace(session, Some(&name)).unwrap();
            for _ in 0..tabs {
                let children = (0..4)
                    .map(|_| {
                        let id = mux.allocate().unwrap();
                        SplitChild {
                            weight: SPLIT_TOTAL / 4,
                            node: PaneNode::Pane(PaneInfo {
                                id: PaneId::in_runtime(runtime, id),
                                terminal_id: TerminalId::in_runtime(
                                    runtime, id,
                                ),
                                fallback_name: name.clone(),
                                custom_name: Some(name.clone()),
                            }),
                        }
                    })
                    .collect();
                let layout = PaneLayout::new(PaneNode::Split(SplitNode {
                    id: SplitId::in_runtime(runtime, mux.allocate().unwrap()),
                    axis: SplitAxis::Columns,
                    children,
                }))
                .unwrap();
                let tab = Tab {
                    id: TabId::in_runtime(runtime, mux.allocate().unwrap()),
                    custom_name: Some(name.clone()),
                    layout,
                    zoomed: None,
                    input_sync: false,
                };
                mux.workspaces.get_mut(&workspace).unwrap().tabs.push(tab);
            }
        }
    }
    mux
}

/// The median of `runs` timings of `work`.
fn median(runs: usize, mut work: impl FnMut() -> u64) -> Duration {
    let mut times: Vec<Duration> = (0..runs)
        .map(|_| {
            let start = Instant::now();
            std::hint::black_box(work());
            start.elapsed()
        })
        .collect();
    times.sort_unstable();
    times[runs / 2]
}

/// Measures [`Mux::hierarchy_digest`] against building a snapshot and
/// digesting it. Run with
/// `cargo test --release -p huterm-core hierarchy_digest_cost -- --ignored --nocapture`.
#[test]
#[ignore = "a release-mode measurement"]
fn hierarchy_digest_cost() {
    for (label, mux) in [
        ("small, 8-byte names", fixture(1, 1, 3, 8)),
        ("small, longest names", fixture(1, 1, 3, MAX_NAME_BYTES)),
        ("large, 8-byte names", fixture(32, 4, 20, 8)),
        ("large, longest names", fixture(32, 4, 20, MAX_NAME_BYTES)),
    ] {
        let items = mux.hierarchy_items();
        assert!(items <= MAX_HIERARCHY_ITEMS);
        assert_eq!(
            mux.hierarchy_digest(BASE),
            mux.hierarchy_state().digest_for(BASE)
        );
        let runs = if items < 100 { 2001 } else { 101 };
        let walk = median(runs, || mux.hierarchy_digest(BASE));
        let built = median(runs, || mux.hierarchy_state().digest_for(BASE));
        let state = mux.hierarchy_state();
        let digest_only = median(runs, || state.digest_for(BASE));
        println!(
            "{label}: {items} items, Mux::hierarchy_digest {walk:?}, \
             build and digest {built:?} (digest of a built state \
             {digest_only:?})"
        );
    }
}
