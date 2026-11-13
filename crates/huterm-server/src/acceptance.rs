//! Slice 3b's acceptance cases on the deterministic world: model terminals
//! behind the real server connection and client session, on a virtual
//! clock, so each case runs the same way every time.

use huterm_client::ViewerEvent;
use huterm_protocol::GridSize;

use crate::ServerConfig;
use crate::model::ModelLimits;
use crate::world::{LinkProfile, World};

const SETTLE: u64 = 20_000;

fn grid(columns: u16, rows: u16) -> GridSize {
    GridSize { columns, rows }
}

fn world(terminals: usize) -> World {
    World::new(
        1,
        ServerConfig::default(),
        terminals,
        grid(20, 6),
        ModelLimits::default(),
    )
}

#[test]
fn a_viewer_converges_on_its_terminal_through_output() {
    let mut world = world(1);
    let peer = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want(peer, 0, true);
    world.settle(SETTLE);
    assert!(world.shows_current(peer, 0));
    for line in 0..30 {
        world.terminals[0].write_line(&format!("line {line}"));
        world.step();
    }
    world.settle(SETTLE);
    assert!(world.shows_current(peer, 0));
    let updates = world
        .viewer_events(peer, 0)
        .iter()
        .filter(|event| matches!(event, ViewerEvent::Updated { .. }))
        .count();
    assert!(updates > 1, "{updates} updates");
}

/// Updates `peer` applied, from its log.
fn applied(world: &World, peer: usize) -> usize {
    world
        .viewer_events(peer, 0)
        .iter()
        .filter(|event| matches!(event, ViewerEvent::Updated { .. }))
        .count()
}

#[test]
fn a_fast_and_a_slow_client_converge_while_the_slow_one_skips_publications() {
    let mut world = world(1);
    let fast = world.add_peer(LinkProfile::INSTANT);
    let slow = world.add_peer(LinkProfile {
        latency: 40,
        jitter: 20,
        chunk: 256,
        buffer: 2048,
    });
    world.settle(SETTLE);
    world.want(fast, 0, true);
    world.want(slow, 0, true);
    world.settle(SETTLE);
    let publications = 300;
    for line in 0..publications {
        world.terminals[0].write_line(&format!("output {line}"));
        world.step();
    }
    world.settle(SETTLE);
    assert!(world.shows_current(fast, 0));
    assert!(world.shows_current(slow, 0));
    let (fast, slow) = (applied(&world, fast), applied(&world, slow));
    assert!(
        slow < fast && slow < publications,
        "slow {slow}, fast {fast}, of {publications} publications"
    );
}

#[test]
fn a_client_that_stops_reading_stalls_nobody_and_is_disconnected_at_the_deadline()
 {
    let mut world = world(1);
    let reader = world.add_peer(LinkProfile::INSTANT);
    let buffer = 512;
    let stalled = world.add_peer(LinkProfile {
        latency: 0,
        jitter: 0,
        chunk: usize::MAX,
        buffer,
    });
    world.settle(SETTLE);
    world.want(reader, 0, true);
    world.want(stalled, 0, true);
    world.settle(SETTLE);
    world.peers[stalled].stalled_until = u64::MAX;
    let deadline = u64::try_from(
        huterm_wire::handshake::WRITER_STALL_DEADLINE.as_millis(),
    )
    .unwrap();
    // Output keeps flowing to the other viewer while the stalled client
    // reads nothing. Bells keep its status changing, which needs no
    // credit, so its connection always has something to write.
    let mut full_at = None;
    let mut line = 0;
    while world.peers[stalled].closes.is_empty() {
        world.terminals[0].write_line(&format!("line {line}"));
        world.terminals[0].bell();
        line += 1;
        for _ in 0..10 {
            world.step();
            if full_at.is_none() && world.peers[stalled].down.len() >= buffer {
                full_at = Some(world.now);
            }
        }
        // The reader keeps up the whole time.
        if line % 100 == 0 {
            world.settle(500);
            assert!(world.shows_current(reader, 0), "after {line} lines");
        }
        assert!(
            full_at.is_none_or(|full| world.now < full + deadline + 1000),
            "the stalled client was not disconnected"
        );
    }
    let full_at = full_at.expect("the stalled client's link filled");
    assert!(world.now >= full_at + deadline - 20, "closed too early");
    assert_eq!(world.peers[stalled].closes, [crate::CloseReason::Stalled]);
    world.settle(SETTLE);
    assert!(world.shows_current(reader, 0));
}

#[test]
fn staged_input_is_discarded_at_a_disconnect_and_admitted_input_still_runs() {
    let mut world = World::new(
        1,
        ServerConfig::default(),
        1,
        grid(20, 6),
        ModelLimits {
            queue: 2,
            ..ModelLimits::default()
        },
    );
    let peer = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want(peer, 0, true);
    world.settle(SETTLE);
    for index in 0..5 {
        world.type_text(peer, 0, &format!("key{index}")).unwrap();
    }
    world.settle(SETTLE);
    // The model's queue holds two; the server stages the other three.
    assert_eq!(world.terminals[0].queued(), 2);
    world.cut(peer);
    assert_eq!(world.terminals[0].drain(usize::MAX), 2);
    let written: Vec<String> = world.terminals[0]
        .written()
        .into_iter()
        .map(|written| format!("{:?}", written.input))
        .collect();
    assert_eq!(written, ["Text(\"key0\")", "Text(\"key1\")"]);
    // Nothing staged before the disconnect arrives later.
    world.connect(peer);
    world.settle(SETTLE);
    world.terminals[0].drain(usize::MAX);
    assert_eq!(world.terminals[0].written().len(), 2);
}

#[test]
fn a_hidden_viewer_gets_no_updates_during_output_but_still_hears_exit_and_bells()
 {
    let mut world = world(1);
    let peer = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want(peer, 0, false);
    world.settle(SETTLE);
    let before = applied(&world, peer);
    assert_eq!(before, 1, "the first update spends no credit");
    for line in 0..50 {
        world.terminals[0].write_line(&format!("hidden {line}"));
        world.step();
    }
    world.terminals[0].bell();
    world.terminals[0].exit(3);
    world.settle(SETTLE);
    assert_eq!(applied(&world, peer), before);
    let changes: Vec<_> = world
        .viewer_events(peer, 0)
        .into_iter()
        .filter_map(|event| match event {
            ViewerEvent::Status { changes, .. } => Some(changes.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(changes.iter().map(|changes| changes.bells).sum::<u64>(), 1);
    assert!(changes.iter().any(|changes| {
        changes.exited.is_some_and(|exit| exit.code == Some(3))
    }));
}

#[test]
fn a_credit_grant_to_a_notified_viewer_sends_its_update_without_more_output() {
    let mut world = world(1);
    let peer = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want(peer, 0, false);
    world.settle(SETTLE);
    world.terminals[0].write_line("while hidden");
    world.settle(SETTLE);
    assert!(!world.shows_current(peer, 0));
    world.peers[peer].wanted[0].visible = true;
    world.settle(SETTLE);
    assert!(world.shows_current(peer, 0));
}

#[test]
fn deltas_discarded_as_stale_still_return_credit_and_the_resync_snapshot_arrives()
 {
    let mut world = world(1);
    let peer = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want(peer, 0, true);
    world.settle(SETTLE);
    world.tamper(peer, 1000);
    // One delta with a full row, damaged on its way.
    world.terminals[0].set_row(1, "damaged on the way");
    world.settle(SETTLE);
    assert_eq!(world.peers[peer].tamper.as_ref().unwrap().damaged, 1);
    world.peers[peer].tamper.as_mut().unwrap().per_mille = 0;
    // Cursor moves keep the damaged row and reach the next digest, which
    // finds the damage; deltas after the resync request are discarded
    // as stale while the complete snapshot is on its way.
    for column in 0..2 * huterm_wire::update::UPDATE_DIGEST_INTERVAL {
        world.terminals[0].move_cursor(u16::try_from(column % 20).unwrap());
        world.step();
    }
    world.settle(SETTLE);
    assert_eq!(world.peers[peer].session.stats().viewer_resyncs, 1);
    assert!(world.shows_current(peer, 0));
    // Normal updates resume.
    world.terminals[0].write_line("after the resync");
    world.settle(SETTLE);
    assert!(world.shows_current(peer, 0));
}

/// What `peer` heard about terminal `terminal` through status since its
/// log's `since`: bells, the exit code, and failure messages.
fn heard(
    world: &World,
    peer: usize,
    since: usize,
) -> (u64, Option<u32>, Vec<String>) {
    let mut bells = 0;
    let mut exit = None;
    let mut failures = Vec::new();
    for event in world.viewer_events(peer, since) {
        if let ViewerEvent::Status { changes, .. } = event {
            bells += changes.bells;
            if let Some(exited) = changes.exited {
                exit = exited.code;
            }
            failures.extend(
                changes
                    .failures
                    .iter()
                    .map(|failure| failure.message.clone()),
            );
        }
    }
    (bells, exit, failures)
}

#[test]
fn a_reconnect_to_the_same_incarnation_reports_what_happened_while_away() {
    let mut world = world(2);
    let peer = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want(peer, 0, true);
    world.want(peer, 1, true);
    world.settle(SETTLE);
    world.cut(peer);
    let since = world.peers[peer].log.len();
    // While the client is away: bells and an exit on one terminal, and a
    // fatal failure that stops the other's runtime.
    world.terminals[0].bell();
    world.terminals[0].bell();
    world.terminals[0].exit(7);
    world.terminals[1].fail("reader failed");
    world.terminals[1].stop();
    world.connect(peer);
    world.settle(SETTLE);
    let (bells, exit, failures) = heard(&world, peer, since);
    assert_eq!(bells, 2);
    assert_eq!(exit, Some(7));
    assert_eq!(failures, ["reader failed"]);
    // The stopped terminal answered with its final status, and no viewer.
    assert_eq!(world.viewer(peer, 1), None);
    assert!(world.viewer(peer, 0).is_some());
    assert!(world.shows_current(peer, 0));
}

#[test]
fn a_reconnect_to_a_new_incarnation_starts_fresh_status_baselines() {
    let mut world = world(1);
    let peer = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want(peer, 0, true);
    world.settle(SETTLE);
    world.restart_server();
    // The new incarnation's terminal rang and exited before the client
    // subscribed to it.
    world.terminals[0].bell();
    world.terminals[0].exit(1);
    let since = world.peers[peer].log.len();
    world.connect(peer);
    world.settle(SETTLE);
    assert!(world.viewer(peer, 0).is_some());
    assert_eq!(heard(&world, peer, since), (0, None, Vec::new()));
    assert!(world.shows_current(peer, 0));
}

#[test]
fn many_viewers_on_large_grids_keep_encoded_output_within_the_budget() {
    // Updates encode only as the writer takes them, so a writer batch
    // larger than the budget is what tests the budget's gate.
    let budget = 128 << 10;
    let mut world = World::new(
        3,
        ServerConfig {
            outbound_budget: budget,
            ..ServerConfig::default()
        },
        8,
        grid(200, 60),
        ModelLimits::default(),
    );
    let peer = world.add_peer(LinkProfile {
        latency: 5,
        jitter: 5,
        chunk: 4096,
        buffer: 64 << 10,
    });
    // A native-like writer: a batch at a time, written slowly.
    world.peers[peer].batch = Some(256 << 10);
    world.peers[peer].write_rate = 4 << 10;
    world.settle(SETTLE);
    for terminal in 0..8 {
        for _ in 0..32 {
            world.want(peer, terminal, true);
        }
    }
    let frame = usize::try_from(huterm_wire::frame::DEFAULT_MAX_FRAME).unwrap();
    let mut most = 0;
    for round in 0..60 {
        // Heavy output: every row of every screen changes each round.
        for terminal in 0..8 {
            for row in 0..60 {
                world.terminals[terminal]
                    .set_row(row, &format!("{round}:{row:0>196}"));
            }
        }
        for _ in 0..5 {
            world.step();
            let connection = world.peers[0].connection.as_ref().unwrap();
            most = most.max(connection.retained());
            assert!(
                connection.retained() <= budget + frame,
                "{} retained, over {budget} plus a frame",
                connection.retained()
            );
        }
    }
    let connection = world.peers[0].connection.as_ref().unwrap();
    assert_eq!(connection.viewers(), 256);
    assert!(most > budget / 2, "the budget was never approached: {most}");
    world.settle(SETTLE);
    for terminal in 0..8 {
        assert!(world.shows_current(peer, terminal));
    }
}

#[test]
fn a_refused_scroll_leaves_the_clients_numbers_in_core_unchanged() {
    let mut world = world(1);
    let peer = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want(peer, 0, true);
    world.settle(SETTLE);
    for line in 0..30 {
        world.terminals[0].write_line(&format!("history {line}"));
    }
    world.settle(SETTLE);
    let viewer = world.viewer(peer, 0).unwrap();
    let now = world.time();
    world.terminals[0]
        .refuse_snapshots(Some(huterm_core::RuntimeError::NotPermitted));
    let refused = world.peers[peer].session.scroll(
        viewer,
        huterm_protocol::ScrollCommand::Relative(5),
        now,
    );
    world.settle(SETTLE);
    world.terminals[0].refuse_snapshots(None);
    let applied = world.peers[peer].session.scroll(
        viewer,
        huterm_protocol::ScrollCommand::Relative(3),
        now,
    );
    world.settle(SETTLE);
    world.type_text(peer, 0, "typed after the scroll").unwrap();
    world.settle(SETTLE);
    world.terminals[0].drain(usize::MAX);
    let outcome = |call| {
        world.peers[peer].log.iter().find_map(|event| match event {
            huterm_client::ClientEvent::Result { call: c, outcome }
                if *c == call =>
            {
                Some(outcome.clone())
            }
            _ => None,
        })
    };
    assert!(outcome(refused).unwrap().is_err());
    assert!(outcome(applied).unwrap().is_ok());
    // Core applied the client's second scroll under the client's number,
    // and the typing after it carries that number, so it cannot undo it.
    assert_eq!(
        world.terminals[0].scrolls(),
        [(viewer_in_model(&world, 0), 2)]
    );
    let written = world.terminals[0].written();
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].stamp.scrolls, 2);
    assert!(world.shows_current(peer, 0));
}

/// The model terminal's identity for its one viewer.
fn viewer_in_model(
    world: &World,
    terminal: usize,
) -> huterm_protocol::ViewerId {
    let viewer = world.viewer(0, terminal).unwrap();
    huterm_protocol::ViewerId::in_runtime(
        world.terminals[terminal].id().runtime(),
        viewer.get(),
    )
}

#[test]
fn a_scroll_during_a_multi_frame_update_arrives_after_it_and_both_apply() {
    let mut world = World::new(
        4,
        ServerConfig::default(),
        1,
        grid(1000, 120),
        ModelLimits::default(),
    );
    let peer = world.add_peer(LinkProfile {
        latency: 1,
        jitter: 0,
        chunk: 8 << 10,
        buffer: 16 << 10,
    });
    world.peers[peer].session = huterm_client::ClientSession::new({
        let mut config = huterm_client::ClientConfig::local(
            huterm_wire::handshake::ClientKind::Desktop,
            "world",
            "test",
        );
        config.hello.max_frame = huterm_wire::frame::MIN_MAX_FRAME;
        config
    });
    world.connect(peer);
    world.settle(SETTLE);
    world.want(peer, 0, true);
    world.settle(SETTLE);
    for line in 0..40 {
        world.terminals[0].write_line(&format!("{line:x<999}"));
    }
    world.settle(SETTLE);
    // A screen of new rows: one update of several frames.
    for row in 0..120 {
        world.terminals[0].set_row(row, &format!("{row:y<999}"));
    }
    let before = applied(&world, peer);
    // Step until the update's first frames are on their way.
    while world.peers[peer].down.len() == 0 {
        world.step();
    }
    let viewer = world.viewer(peer, 0).unwrap();
    let now = world.time();
    let scroll = world.peers[peer].session.scroll(
        viewer,
        huterm_protocol::ScrollCommand::Relative(10),
        now,
    );
    world.settle(SETTLE);
    let seqs: Vec<u64> = world
        .viewer_events(peer, 0)
        .into_iter()
        .filter_map(|event| match event {
            ViewerEvent::Updated { seq, .. } => Some(*seq),
            _ => None,
        })
        .collect();
    assert!(seqs.len() >= before + 2, "{seqs:?}");
    assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]), "{seqs:?}");
    assert!(world.peers[peer].log.iter().any(|event| matches!(
        event,
        huterm_client::ClientEvent::Result { call, outcome: Ok(_) } if *call == scroll
    )));
    assert_eq!(world.terminals[0].snapshot().viewport.bottom_offset, 10);
    assert!(world.shows_current(peer, 0));
}
