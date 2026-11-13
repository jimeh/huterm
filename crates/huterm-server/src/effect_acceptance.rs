//! Slice 4's host-effect acceptance cases on the deterministic world: model
//! terminals behind the real server connection and client session, on a
//! virtual clock, so every deadline is exact and each case replays.

use huterm_client::EffectDelivery;
use huterm_protocol::{GridSize, HostEffectKind, ViewerId};
use huterm_wire::caps::CLIPBOARD_PAYLOAD_LIMIT;
use huterm_wire::effect::EffectPayload;
use huterm_wire::handshake::{
    HOST_EFFECT_DELIVERY_DEADLINE, HOST_EFFECT_MIN_THROUGHPUT,
};
use huterm_wire::stream::HOST_EFFECT_STREAM;

use crate::ServerConfig;
use crate::model::ModelLimits;
use crate::world::{LinkProfile, World};

const SETTLE: u64 = 20_000;

fn world(terminals: usize, size: GridSize) -> World {
    World::new(
        1,
        ServerConfig::default(),
        terminals,
        size,
        ModelLimits::default(),
    )
}

fn small() -> GridSize {
    GridSize {
        columns: 20,
        rows: 6,
    }
}

/// A 200-column line whose letters vary with `seed`, so its row travels
/// as text rather than as a fill.
fn varied(seed: usize) -> String {
    (0..200)
        .map(|column| {
            let letter = u8::try_from((column * 7 + seed) % 26).unwrap();
            char::from(b'a' + letter)
        })
        .collect()
}

fn text(delivery: &EffectDelivery) -> &str {
    match &delivery.payload {
        EffectPayload::ClipboardWrite { text, .. } => text,
        payload @ EffectPayload::Notification { .. } => {
            panic!("a clipboard write, not {payload:?}")
        }
    }
}

fn texts(world: &World, peer: usize) -> Vec<String> {
    world
        .effects(peer)
        .into_iter()
        .map(|delivery| text(delivery).to_owned())
        .collect()
}

/// `peer`'s viewer of terminal `index`, in the model terminal's scope.
fn model_viewer(world: &World, peer: usize, index: usize) -> ViewerId {
    let viewer = world.viewer(peer, index).expect("a live viewer");
    ViewerId::in_runtime(world.terminals[index].id().runtime(), viewer.get())
}

fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap()
}

/// Steps until `peer` received `count` effects, returning when the last
/// one arrived, or panics after `limit` steps.
fn until_effects(
    world: &mut World,
    peer: usize,
    count: usize,
    limit: u64,
) -> u64 {
    for _ in 0..limit {
        if world.effects(peer).len() >= count {
            return world.now;
        }
        world.step();
    }
    panic!(
        "{} of {count} effects after {limit} steps",
        world.effects(peer).len()
    );
}

#[test]
fn a_maximum_write_on_a_reading_client_completes_and_an_effect_behind_it_follows_without_a_wake()
 {
    let mut world = world(2, small());
    let peer = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want_effects(peer, 0, true);
    // A hidden viewer the client grants no credit still receives effects.
    world.want_effects(peer, 1, false);
    world.settle(SETTLE);
    // A writer that moves 8 MiB a second, so the maximum write takes 8 s.
    let rate = usize::try_from(HOST_EFFECT_MIN_THROUGHPUT / 1000).unwrap();
    world.peers[peer].batch = Some(256 << 10);
    world.peers[peer].write_rate = rate;
    let big = "x".repeat(CLIPBOARD_PAYLOAD_LIMIT);
    assert!(world.terminals[0].clipboard(&big));
    assert!(world.terminals[1].clipboard("small"));
    // Both are admitted and signalled now; nothing wakes either viewer
    // again before they arrive.
    let start = world.now;
    let arrived = until_effects(&mut world, peer, 1, 20_000);
    assert!(
        world
            .terminals
            .iter()
            .all(|terminal| terminal.take_signalled().is_empty()),
        "a viewer was woken again"
    );
    until_effects(&mut world, peer, 2, 1_000);
    let effects = world.effects(peer);
    assert_eq!(text(effects[0]).len(), CLIPBOARD_PAYLOAD_LIMIT);
    assert_eq!(text(effects[1]), "small");
    assert!(
        arrived - start >= 7_900,
        "the write took {} ms",
        arrived - start
    );
    let stats = world.peers[peer]
        .connection
        .as_ref()
        .unwrap()
        .effect_stats();
    assert_eq!((stats.sent, stats.expired), (2, 0));
    assert_eq!(world.terminals[0].effects_held(), (0, 0));
}

#[test]
fn an_effect_goes_out_ahead_of_queued_update_frames() {
    let size = GridSize {
        columns: 200,
        rows: 40,
    };
    let terminals = 6;
    let mut world = world(terminals, size);
    let peer = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    for index in 0..terminals {
        world.want_effects(peer, index, true);
    }
    world.settle(SETTLE);
    // A writer that takes about one update frame per batch, slowly.
    world.peers[peer].batch = Some(16 << 10);
    world.peers[peer].write_rate = 1_000;
    // Varied text, so rows travel as text rather than fills.
    for index in 0..terminals {
        for row in 0..40 {
            world.terminals[index].write_line(&varied(row * 13 + index));
        }
    }
    // Updates queue behind the batch being written.
    while world.peers[peer].writing.is_none() {
        world.step();
    }
    assert!(world.terminals[0].clipboard("ahead"));
    world.step();
    // The batch in the writer finishes; the next one opens with the
    // effect's frame, ahead of every queued update.
    let mut batches = 0;
    loop {
        let before = world.peers[peer].writing.as_ref().map(|(_, at)| *at);
        world.step();
        // A new batch: the writer had none, or its offset started over.
        let started = match (before, &world.peers[peer].writing) {
            (None, Some(_)) => true,
            (Some(before), Some((_, at))) => *at < before,
            _ => false,
        };
        if started {
            let (batch, _) = world.peers[peer].writing.as_ref().unwrap();
            // A frame header is a 4-byte length, the flags, and the
            // stream varint.
            assert_eq!(
                u32::from(batch[5]),
                HOST_EFFECT_STREAM,
                "the effect waited behind other frames"
            );
            break;
        }
        batches += 1;
        assert!(batches < 100_000, "the writer never took another batch");
    }
    world.peers[peer].write_rate = usize::MAX;
    world.settle(SETTLE);
    assert_eq!(texts(&world, peer), ["ahead"]);
    for index in 0..terminals {
        assert!(world.shows_current(peer, index));
    }
}

#[test]
fn an_effect_whose_client_trickles_mid_frame_is_dropped_at_its_deadline_and_releases_at_once()
 {
    let mut world = world(1, small());
    let other = world.add_peer(LinkProfile::INSTANT);
    let stuck = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want_effects(other, 0, true);
    world.want_effects(stuck, 0, true);
    world.settle(SETTLE);
    let terminal = world.terminals[0].clone();
    terminal.activate(model_viewer(&world, stuck, 0));
    // The stuck client takes one byte a step, so the effect's first frame
    // stays half written in its writer.
    world.peers[stuck].batch = Some(64 << 10);
    world.peers[stuck].write_rate = 1;
    let big = "y".repeat(CLIPBOARD_PAYLOAD_LIMIT);
    assert!(terminal.clipboard(&big));
    // The terminal's bytes are spoken for, so the other client's write
    // is refused while the stuck one holds them.
    terminal.activate(model_viewer(&world, other, 0));
    assert!(!terminal.clipboard("refused"));
    let start = world.now;
    let deadline = start + millis(HOST_EFFECT_DELIVERY_DEADLINE);
    while world.now < deadline {
        world.step();
        assert_eq!(world.effects(stuck).len(), 0);
    }
    // At the deadline the effect is dropped, and its reservation with it.
    world.step();
    let stats = world.peers[stuck]
        .connection
        .as_ref()
        .unwrap()
        .effect_stats();
    assert_eq!(stats.expired, 1);
    assert_eq!(terminal.effects_held(), (0, 0));
    assert!(terminal.clipboard("admitted"));
    until_effects(&mut world, other, 1, 1_000);
    assert_eq!(texts(&world, other), ["admitted"]);
    // Once the stuck client reads again, the frame in its writer ends,
    // the abort follows it, and its framing stays valid: a later effect
    // arrives and its view converges.
    world.peers[stuck].write_rate = usize::MAX;
    world.settle(SETTLE);
    terminal.activate(model_viewer(&world, stuck, 0));
    assert!(terminal.clipboard("later"));
    world.settle(SETTLE);
    assert_eq!(texts(&world, stuck), ["later"]);
    assert_eq!(world.peers[stuck].closes.len(), 0);
    assert!(world.shows_current(stuck, 0));
}

#[test]
fn a_recipient_that_stops_reading_does_not_hold_up_other_recipients() {
    let mut world = world(1, small());
    let reader = world.add_peer(LinkProfile::INSTANT);
    let stalled = world.add_peer(LinkProfile {
        latency: 0,
        jitter: 0,
        chunk: usize::MAX,
        buffer: 4096,
    });
    world.settle(SETTLE);
    world.want_effects(reader, 0, true);
    world.want_effects(stalled, 0, true);
    world.settle(SETTLE);
    let terminal = world.terminals[0].clone();
    world.peers[stalled].stalled_until = u64::MAX;
    terminal.activate(model_viewer(&world, stalled, 0));
    assert!(terminal.clipboard(&"z".repeat(1 << 20)));
    terminal.activate(model_viewer(&world, reader, 0));
    for n in 0..20 {
        // Each write is admitted while the stalled client holds one.
        assert!(terminal.clipboard(&format!("copy {n}")), "copy {n}");
        until_effects(&mut world, reader, n + 1, 1_000);
    }
    assert_eq!(texts(&world, reader).last().unwrap(), "copy 19");
}

#[test]
fn a_burst_of_writes_in_one_batch_is_admitted_and_the_client_ends_with_the_last()
 {
    let mut world = world(1, small());
    let peer = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want_effects(peer, 0, true);
    world.settle(SETTLE);
    for n in 0..5 {
        assert!(world.terminals[0].clipboard(&format!("set {n}")));
    }
    world.settle(SETTLE);
    assert_eq!(
        texts(&world, peer),
        ["set 0", "set 1", "set 2", "set 3", "set 4"]
    );
}

#[test]
fn an_effect_sent_before_a_revoke_is_never_executed_even_after_a_regrant() {
    let mut world = world(1, small());
    let peer = world.add_peer(LinkProfile {
        latency: 50,
        jitter: 0,
        chunk: usize::MAX,
        buffer: usize::MAX,
    });
    world.settle(SETTLE);
    world.want_effects(peer, 0, true);
    world.settle(SETTLE);
    let viewer = world.viewer(peer, 0).unwrap();
    assert!(world.terminals[0].clipboard("before"));
    while world.peers[peer]
        .connection
        .as_ref()
        .unwrap()
        .effect_stats()
        .sent
        == 0
    {
        world.step();
    }
    // The effect is on its way when the user revokes and re-grants.
    let session = &mut world.peers[peer].session;
    let revoked = session
        .set_host_effect_permission(viewer, HostEffectKind::Clipboard, false)
        .unwrap();
    session
        .set_host_effect_permission(viewer, HostEffectKind::Clipboard, true)
        .unwrap();
    assert!(revoked > huterm_wire::effect::INITIAL_EFFECT_GENERATION);
    world.settle(SETTLE);
    assert_eq!(world.effects(peer).len(), 0);
    assert_eq!(world.peers[peer].session.stats().effects_discarded, 1);
    // Later effects run normally after any number of changes, and each
    // carries the generation the client last numbered.
    let mut last = 0;
    for allowed in [false, true, false, false, true] {
        last = world.peers[peer]
            .session
            .set_host_effect_permission(
                viewer,
                HostEffectKind::Clipboard,
                allowed,
            )
            .unwrap();
    }
    world.settle(SETTLE);
    assert!(world.terminals[0].clipboard("after"));
    world.settle(SETTLE);
    let effects = world.effects(peer);
    assert_eq!(effects.len(), 1);
    assert_eq!(text(effects[0]), "after");
    assert_eq!(effects[0].generation, last);
    assert!(world.peers[peer].session.host_effect_current(
        viewer,
        HostEffectKind::Clipboard,
        last
    ));
    // A revoke that reached the server denies the next write at admission.
    world.peers[peer]
        .session
        .set_host_effect_permission(viewer, HostEffectKind::Clipboard, false)
        .unwrap();
    world.settle(SETTLE);
    assert!(!world.terminals[0].clipboard("denied"));
}

#[test]
fn an_effect_pending_at_disconnect_reaches_neither_the_reconnected_client_nor_another()
 {
    let mut world = world(1, small());
    let keep = world.add_peer(LinkProfile::INSTANT);
    let gone = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want_effects(keep, 0, true);
    world.want_effects(gone, 0, true);
    world.settle(SETTLE);
    let terminal = world.terminals[0].clone();
    terminal.activate(model_viewer(&world, gone, 0));
    world.peers[gone].stalled_until = u64::MAX;
    assert!(terminal.clipboard("pending"));
    assert!(terminal.clipboard("queued"));
    for _ in 0..10 {
        world.step();
    }
    world.cut(gone);
    assert_eq!(terminal.effects_held(), (0, 0));
    world.peers[gone].stalled_until = 0;
    world.connect(gone);
    world.settle(SETTLE);
    assert_eq!(texts(&world, keep).len(), 0);
    assert_eq!(texts(&world, gone).len(), 0);
    // The reconnected client's new viewer receives what comes next.
    terminal.activate(model_viewer(&world, gone, 0));
    assert!(terminal.clipboard("fresh"));
    world.settle(SETTLE);
    assert_eq!(texts(&world, gone), ["fresh"]);
    assert_eq!(texts(&world, keep).len(), 0);
}

#[test]
fn a_text_client_is_never_selected_and_a_notification_reaches_the_desktop() {
    let mut world = world(1, small());
    let desktop = world.add_peer(LinkProfile::INSTANT);
    let text_client = world.add_peer_as(
        LinkProfile::INSTANT,
        huterm_wire::handshake::ClientKind::Text,
    );
    world.settle(SETTLE);
    world.want_effects(desktop, 0, true);
    world.want_effects(text_client, 0, true);
    world.settle(SETTLE);
    let terminal = world.terminals[0].clone();
    terminal.activate(model_viewer(&world, text_client, 0));
    assert!(terminal.clipboard("copy"));
    assert!(terminal.notify("build", "done"));
    world.settle(SETTLE);
    assert_eq!(world.effects(text_client).len(), 0);
    let effects = world.effects(desktop);
    assert_eq!(effects.len(), 2);
    assert_eq!(text(effects[0]), "copy");
    assert_eq!(
        effects[1].payload,
        EffectPayload::Notification {
            title: "build".into(),
            body: "done".into(),
        }
    );
}

#[test]
fn an_effect_admitted_before_its_viewer_is_announced_follows_the_subscription_result()
 {
    let mut world = world(
        2,
        GridSize {
            columns: 200,
            rows: 40,
        },
    );
    let peer = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want(peer, 1, true);
    world.settle(SETTLE);
    // Another viewer's large update holds a slow writer, so the new
    // viewer's result waits in the queue when the terminal asks for a
    // write.
    world.peers[peer].batch = Some(4 << 10);
    world.peers[peer].write_rate = 1;
    for row in 0..40 {
        world.terminals[1].write_line(&varied(row * 13));
    }
    while world.peers[peer].writing.is_none() {
        world.step();
    }
    world.want_effects(peer, 0, true);
    let terminal = world.terminals[0].clone();
    for _ in 0..1_000 {
        if terminal.viewers() > 0 {
            break;
        }
        world.step();
    }
    assert!(terminal.clipboard("early"));
    world.peers[peer].write_rate = usize::MAX;
    world.settle(SETTLE);
    assert_eq!(texts(&world, peer), ["early"]);
    assert_eq!(world.peers[peer].session.stats().effects_discarded, 0);
}

#[test]
fn effects_back_to_back_still_let_updates_through() {
    let size = GridSize {
        columns: 200,
        rows: 40,
    };
    let mut world = world(1, size);
    let peer = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want_effects(peer, 0, true);
    world.settle(SETTLE);
    // A writer that takes one frame a batch and a batch a millisecond, so
    // an effect's frame leaves no room for another. A write of this size
    // takes about half a second, within its deadline.
    world.peers[peer].batch = Some(1 << 10);
    let terminal = world.terminals[0].clone();
    let write = "q".repeat(512 << 10);
    let updated = |world: &World| {
        world
            .viewer_events(peer, 0)
            .iter()
            .filter(|event| {
                matches!(event, huterm_client::ViewerEvent::Updated { .. })
            })
            .count()
    };
    // Forty seconds of writes back to back, with output all the while;
    // updates keep arriving in every ten of them.
    let mut windows = Vec::new();
    for window in 0..4 {
        let before = updated(&world);
        for ms in 0..10_000 {
            let _ = terminal.clipboard(&write);
            if ms % 100 == 0 {
                terminal.write_line(&varied(ms + window));
            }
            world.step();
        }
        windows.push(updated(&world) - before);
    }
    assert_eq!(world.peers[peer].closes.len(), 0);
    assert!(world.peers[peer].session.is_connected());
    assert!(
        windows.iter().all(|updates| *updates > 0),
        "updates per ten seconds: {windows:?}"
    );
    assert!(world.effects(peer).len() >= 40);
    world.peers[peer].write_rate = usize::MAX;
    // Settling sees no event while a large write streams; step until every
    // queued write is out.
    while terminal.effects_held() != (0, 0) {
        world.step();
    }
    world.settle(SETTLE);
    assert!(world.shows_current(peer, 0));
}

#[test]
fn a_streaming_effect_lets_a_started_update_finish() {
    let size = GridSize {
        columns: 200,
        rows: 40,
    };
    let mut world = world(1, size);
    let peer = world.add_peer(LinkProfile::INSTANT);
    world.settle(SETTLE);
    world.want_effects(peer, 0, true);
    world.settle(SETTLE);
    // A writer that takes 2 KiB batches at 1 KiB a millisecond, so an
    // update of the varied screen takes several batches.
    world.peers[peer].batch = Some(2 << 10);
    world.peers[peer].write_rate = 1 << 10;
    let terminal = world.terminals[0].clone();
    let updated = |world: &World| {
        world
            .viewer_events(peer, 0)
            .iter()
            .filter(|event| {
                matches!(event, huterm_client::ViewerEvent::Updated { .. })
            })
            .count()
    };
    let before = updated(&world);
    for row in 0..40 {
        terminal.write_line(&varied(row * 13));
    }
    // The update's first frame is out when the terminal asks for a write
    // that takes far longer than the rest of the update.
    for _ in 0..1_000 {
        if world.peers[peer].writing.is_some() {
            break;
        }
        world.step();
    }
    assert!(world.peers[peer].writing.is_some(), "nothing was written");
    assert!(terminal.clipboard(&"c".repeat(1 << 20)));
    let mut steps = 0;
    while updated(&world) == before {
        world.step();
        steps += 1;
        assert!(steps < 5_000, "the update never applied");
    }
    assert_eq!(
        world.effects(peer).len(),
        0,
        "the update waited {steps} ms for the whole write"
    );
    until_effects(&mut world, peer, 1, 5_000);
    world.settle(SETTLE);
    assert!(world.shows_current(peer, 0));
}
