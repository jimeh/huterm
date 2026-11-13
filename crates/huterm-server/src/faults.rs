//! Seeded fault injection over the real server and client.
//!
//! `huterm-ipc-sim` proves the design with its own codec and nothing may
//! depend on it, so it never runs `huterm-wire`, the server's connection,
//! or the client's session. This harness does: each run joins
//! [`Connection`](crate::Connection)s and
//! [`ClientSession`](huterm_client::ClientSession)s through the in-memory
//! links of the deterministic [`World`], behind model terminals, and draws
//! everything from one seed, so a run replays exactly. Its scenarios take
//! the simulation's broker profiles as their specification: latency,
//! jitter, fragmentation, bounded buffers, stalled and trickling readers,
//! connection cuts, work held and completed out of order, and decodable
//! damage to deltas. Every viewer also receives host effects: terminals
//! ask for clipboard writes, large ones on faulty links so writes miss
//! their deadlines, and clients on faulty links toggle their permission
//! while effects are on their way.
//!
//! After the faults stop, each run makes every viewer visible, lets the
//! terminals publish past the next update digest, waits for quiescence,
//! and checks the invariants the simulation checks: every visible viewer
//! shows its terminal's latest snapshot; damage is caught by digests and
//! recovered by resync; without damage no client resyncs or faults, so no
//! lane reordered a message and no credit went wrong; no input reaches a
//! terminal twice or out of order; on a healthy link every input arrives;
//! and no connection ever retains more than its bound. For host effects:
//! each effect reaches at most one client, the one whose viewer core chose,
//! in order per viewer; on a healthy link every effect arrives; and no
//! terminal holds a reservation once its effects are done.
//!
//! The normal suite runs every scenario over a few seeds. `mise run
//! faults:ipc` runs many more in release, and `mise run faults:ipc --
//! SCENARIO SEED` replays one.

use std::collections::{BTreeMap, BTreeSet};

use huterm_client::{ClientEvent, CloseReason, ViewerEvent};
use huterm_protocol::{GridSize, HostEffectKind, TerminalInput, ViewerId};
use huterm_wire::effect::EffectPayload;
use huterm_wire::response::{Response, Success};

use crate::ServerConfig;
use crate::model::ModelLimits;
use crate::world::{LinkProfile, Rng, World};

/// One scenario's link and fault rates. Rates are chances per thousand
/// per virtual millisecond, per peer.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Scenario {
    pub(crate) name: &'static str,
    link: LinkProfile,
    peers: usize,
    terminals: usize,
    cut: u64,
    stall: u64,
    /// Chance per thousand that a stall outlasts the writer stall
    /// deadline.
    long_stall: u64,
    /// Chance per thousand, per viewer and millisecond, that the terminal
    /// revokes it or the client closes it; both discard what it had
    /// queued, so only faulty scenarios end viewers.
    end: u64,
    damage: u64,
    hold_work: bool,
    /// Every input must arrive: no cut, stall past the deadline, or
    /// damage.
    healthy: bool,
    duration: u64,
    limits: ModelLimits,
}

const LOCAL: LinkProfile = LinkProfile {
    latency: 1,
    jitter: 1,
    chunk: 64,
    buffer: 4096,
};

const REMOTE: LinkProfile = LinkProfile {
    latency: 80,
    jitter: 120,
    chunk: 5,
    buffer: 2048,
};

/// Every fixed scenario.
pub(crate) fn scenarios() -> Vec<Scenario> {
    let base = Scenario {
        name: "clean",
        link: LOCAL,
        peers: 3,
        terminals: 2,
        cut: 0,
        stall: 0,
        long_stall: 10,
        end: 0,
        damage: 0,
        hold_work: true,
        healthy: true,
        duration: 3_000,
        limits: ModelLimits {
            queue: 8,
            viewer_input: 256,
        },
    };
    vec![
        base,
        Scenario {
            name: "local-faults",
            link: LinkProfile {
                chunk: 7,
                buffer: 512,
                ..LOCAL
            },
            cut: 2,
            stall: 2,
            end: 3,
            healthy: false,
            ..base
        },
        Scenario {
            name: "remote",
            link: REMOTE,
            cut: 1,
            stall: 1,
            end: 3,
            healthy: false,
            duration: 8_000,
            ..base
        },
        Scenario {
            name: "remote-healthy",
            link: REMOTE,
            duration: 6_000,
            ..base
        },
        // Readers that stall past the writer stall deadline, so the server
        // disconnects them and they reconnect.
        Scenario {
            name: "stalled-readers",
            link: LinkProfile {
                buffer: 512,
                ..LOCAL
            },
            stall: 1,
            long_stall: 400,
            healthy: false,
            duration: 45_000,
            ..base
        },
        Scenario {
            name: "state-corruption",
            link: LOCAL,
            damage: 200,
            healthy: false,
            ..base
        },
    ]
}

/// A scenario whose link, rates, and limits the seed draws.
pub(crate) fn swarm(seed: u64) -> Scenario {
    let mut rng = Rng::new(seed ^ 0x5157_4a52_4d00_0000);
    let mut pick = |values: &[u64]| values[rng.index(values.len())];
    let link = LinkProfile {
        latency: pick(&[0, 1, 5, 40, 120]),
        jitter: pick(&[0, 1, 20, 80]),
        chunk: usize::try_from(pick(&[1, 5, 64, 4096])).unwrap_or(64),
        buffer: usize::try_from(pick(&[256, 2048, 65_536])).unwrap_or(2048),
    };
    let cut = pick(&[0, 0, 1, 3]);
    let stall = pick(&[0, 0, 1, 3]);
    let damage = pick(&[0, 0, 0, 100]);
    Scenario {
        name: "swarm",
        link,
        peers: usize::try_from(pick(&[1, 2, 3])).unwrap_or(2),
        terminals: usize::try_from(pick(&[1, 2, 3])).unwrap_or(2),
        cut,
        stall,
        long_stall: 10,
        end: u64::from(cut != 0 || stall != 0 || damage != 0),
        damage,
        hold_work: pick(&[0, 1]) == 1,
        healthy: cut == 0 && stall == 0 && damage == 0,
        duration: pick(&[1_000, 3_000]),
        limits: ModelLimits {
            queue: usize::try_from(pick(&[1, 4, 64])).unwrap_or(4),
            viewer_input: usize::try_from(pick(&[16, 256, 1 << 20]))
                .unwrap_or(256),
        },
    }
}

/// What a run checked and which rare paths it reached.
#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) views_compared: usize,
    pub(crate) inputs_written: usize,
    pub(crate) cuts: usize,
    pub(crate) stalls: usize,
    pub(crate) stall_disconnects: usize,
    pub(crate) damaged: u64,
    pub(crate) resyncs: u64,
    pub(crate) revoked: usize,
    pub(crate) closed: usize,
    pub(crate) effects_admitted: usize,
    pub(crate) effects_delivered: usize,
    pub(crate) effects_discarded: u64,
    pub(crate) effects_expired: u64,
    pub(crate) permission_changes: usize,
}

/// The text of terminal `terminal`'s `n`th clipboard write, padded with
/// `padding` bytes so a large write misses its deadline on a slow link.
fn effect_token(terminal: usize, n: u64, padding: usize) -> String {
    format!("e{terminal}.{n}~{}", "~".repeat(padding))
}

fn parse_effect(text: &str) -> Option<(usize, u64)> {
    let (head, _) = text.split_once('~')?;
    let (terminal, n) = head.strip_prefix('e')?.split_once('.')?;
    Some((terminal.parse().ok()?, n.parse().ok()?))
}

/// The token peer `peer` types into terminal `terminal` as its `n`th input.
fn token(peer: usize, terminal: usize, n: u64) -> String {
    format!("<{peer}.{terminal}.{n}>")
}

fn parse(text: &str) -> Option<(usize, usize, u64)> {
    let inner = text.strip_prefix('<')?.strip_suffix('>')?;
    let mut parts = inner.split('.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

/// Runs `scenario` on `seed` and checks its invariants.
///
/// # Errors
/// Returns the first invariant that failed, with what it saw.
#[expect(clippy::too_many_lines, reason = "one run, phase by phase")]
pub(crate) fn run(scenario: &Scenario, seed: u64) -> Result<Report, String> {
    let mut world = World::new(
        seed,
        ServerConfig::default(),
        scenario.terminals,
        GridSize {
            columns: 16,
            rows: 4,
        },
        scenario.limits,
    );
    world.hold_work = scenario.hold_work;
    let mut report = Report::default();
    for _ in 0..scenario.peers {
        world.add_peer(scenario.link);
    }
    world.settle(5_000);
    for peer in 0..scenario.peers {
        for terminal in 0..scenario.terminals {
            let visible = world.rng.chance(700);
            world.want_effects(peer, terminal, visible);
        }
        if scenario.damage > 0 {
            world.tamper(peer, scenario.damage);
        }
    }
    let mut typed: BTreeMap<(usize, usize), u64> = BTreeMap::new();
    // Each terminal's writes so far, and the viewer core chose for each.
    let mut written: BTreeMap<usize, u64> = BTreeMap::new();
    let mut admitted: BTreeMap<(usize, u64), ViewerId> = BTreeMap::new();
    // Viewers whose client revoked the clipboard.
    let mut revoked_clipboard: BTreeSet<ViewerId> = BTreeSet::new();
    let mut reconnect_at: BTreeMap<usize, u64> = BTreeMap::new();
    let start = world.now;
    let deadline = u64::try_from(
        huterm_wire::handshake::WRITER_STALL_DEADLINE.as_millis(),
    )
    .unwrap_or(u64::MAX);
    while world.now < start + scenario.duration {
        // Terminals change.
        for terminal in 0..scenario.terminals {
            let roll = world.rng.below(1000);
            let model = &world.terminals[terminal];
            match roll {
                0..30 => model.write_line(&format!("{}:{}", world.now, roll)),
                30..45 => model.set_row(
                    usize::try_from(roll).unwrap_or(0),
                    &format!("r{}", world.now),
                ),
                45..60 => model.move_cursor(u16::try_from(roll).unwrap_or(0)),
                60..62 => model.bell(),
                62..67 => {
                    // Now and then a large write, on a faulty link.
                    let large = roll == 66 && !scenario.healthy;
                    let n = written.get(&terminal).copied().unwrap_or(0) + 1;
                    let padding = if large { 256 << 10 } else { 0 };
                    let text = effect_token(terminal, n, padding);
                    if let Some(viewer) = model.admit_clipboard(&text) {
                        written.insert(terminal, n);
                        admitted.insert((terminal, n), viewer);
                        report.effects_admitted += 1;
                    }
                }
                _ => {}
            }
            // The PTY drains its queue at its own pace.
            let drain = world.rng.index(3);
            model.drain(drain);
        }
        // Clients act.
        for peer in 0..scenario.peers {
            for terminal in 0..scenario.terminals {
                if world.viewer(peer, terminal).is_none() {
                    continue;
                }
                let roll = world.rng.below(1000);
                match roll {
                    0..20 => {
                        let n =
                            typed.get(&(peer, terminal)).copied().unwrap_or(0)
                                + 1;
                        let text = token(peer, terminal, n);
                        if world.type_text(peer, terminal, &text).is_some() {
                            typed.insert((peer, terminal), n);
                            // Typing makes the viewer the most recently
                            // active, as in core.
                            if let Some(viewer) = world.viewer(peer, terminal) {
                                let model = &world.terminals[terminal];
                                model.activate(ViewerId::in_runtime(
                                    model.id().runtime(),
                                    viewer.get(),
                                ));
                            }
                        }
                    }
                    20..23 => {
                        let viewer = world.viewer(peer, terminal);
                        let now = world.time();
                        if let Some(viewer) = viewer {
                            let rows = i64::try_from(world.rng.below(7))
                                .unwrap_or(0)
                                - 3;
                            world.peers[peer].session.scroll(
                                viewer,
                                huterm_protocol::ScrollCommand::Relative(rows),
                                now,
                            );
                        }
                    }
                    23..26 => {
                        let wanted = &mut world.peers[peer].wanted[terminal];
                        wanted.visible = !wanted.visible;
                    }
                    26..28 => {
                        if let Some(viewer) = world.viewer(peer, terminal) {
                            let focused = world.rng.chance(500);
                            world.peers[peer]
                                .session
                                .send_focus(viewer, focused);
                        }
                    }
                    // Faulty links only: on a healthy one every effect
                    // must arrive.
                    28..33 if !scenario.healthy => {
                        if let Some(viewer) = world.viewer(peer, terminal) {
                            let allowed = revoked_clipboard.contains(&viewer);
                            if world.peers[peer]
                                .session
                                .set_host_effect_permission(
                                    viewer,
                                    HostEffectKind::Clipboard,
                                    allowed,
                                )
                                .is_ok()
                            {
                                if allowed {
                                    revoked_clipboard.remove(&viewer);
                                } else {
                                    revoked_clipboard.insert(viewer);
                                }
                                report.permission_changes += 1;
                            }
                        }
                    }
                    _ => {}
                }
                if world.rng.chance(scenario.end) {
                    if world.rng.chance(500) {
                        if world.revoke(peer, terminal) {
                            report.revoked += 1;
                        }
                    } else if world.close_viewer(peer, terminal) {
                        report.closed += 1;
                    }
                }
            }
            // Faults.
            if world.peers[peer].connection.is_some()
                && world.rng.chance(scenario.cut)
            {
                world.cut(peer);
                report.cuts += 1;
            }
            if world.rng.chance(scenario.stall) {
                // Mostly shorter than the stall deadline; now and then
                // past it, which ends the connection.
                let length = if world.rng.chance(scenario.long_stall) {
                    deadline + 2_000
                } else {
                    world.rng.below(2_000)
                };
                // A stall never shortens one already under way.
                let until = world.now + length;
                let peer = &mut world.peers[peer];
                if world.rng.chance(500) {
                    peer.stalled_until = peer.stalled_until.max(until);
                } else {
                    peer.trickle_until = peer.trickle_until.max(until);
                }
                report.stalls += 1;
            }
            // A client whose connection ended reconnects after a while.
            if world.peers[peer].connection.is_none() {
                let delay = world.rng.below(500);
                let at = *reconnect_at.entry(peer).or_insert(world.now + delay);
                if world.now >= at {
                    reconnect_at.remove(&peer);
                    world.connect(peer);
                }
            }
        }
        world.step();
        check_retained(&world)?;
    }
    // Quiescence: no more faults, every viewer visible, every peer
    // connected.
    for peer in 0..scenario.peers {
        world.peers[peer].stalled_until = 0;
        world.peers[peer].trickle_until = 0;
        // The tamper keeps the frames of messages it has not finished, so
        // it stays, with damage off.
        if let Some(tamper) = world.peers[peer].tamper.as_mut() {
            report.damaged += tamper.damaged;
            tamper.per_mille = 0;
        }
        if world.peers[peer].connection.is_none() {
            world.connect(peer);
        }
        for wanted in &mut world.peers[peer].wanted {
            wanted.visible = true;
        }
    }
    world.hold_work = false;
    world.settle(60_000);
    // Every live viewer permits the clipboard again.
    for peer in 0..scenario.peers {
        for terminal in 0..scenario.terminals {
            if let Some(viewer) = world.viewer(peer, terminal)
                && revoked_clipboard.remove(&viewer)
            {
                let _ = world.peers[peer].session.set_host_effect_permission(
                    viewer,
                    HostEffectKind::Clipboard,
                    true,
                );
            }
        }
    }
    world.settle(60_000);
    // Publish past the next update digest, so damage a kept row still
    // carries is found and repaired.
    let interval = huterm_wire::update::UPDATE_DIGEST_INTERVAL;
    for column in 0..2 * interval {
        for terminal in &world.terminals {
            terminal.move_cursor(u16::try_from(column % 16).unwrap_or(0));
            terminal.drain(usize::MAX);
        }
        world.settle(2_000);
    }
    // The PTYs drain until staged input stops arriving.
    loop {
        let drained: usize = world
            .terminals
            .iter()
            .map(|terminal| terminal.drain(usize::MAX))
            .sum();
        world.settle(60_000);
        if drained == 0 && world.terminals.iter().all(|t| t.queued() == 0) {
            break;
        }
    }
    check_retained(&world)?;
    // Every visible viewer shows its terminal's latest snapshot.
    for peer in 0..scenario.peers {
        for terminal in 0..scenario.terminals {
            if world.viewer(peer, terminal).is_none() {
                return Err(format!(
                    "peer {peer} holds no viewer of terminal {terminal}"
                ));
            }
            if !world.shows_current(peer, terminal) {
                return Err(format!(
                    "peer {peer}'s view of terminal {terminal} diverged:\n\
                     view    {:?}\nlatest  {:?}",
                    world
                        .view(peer, terminal)
                        .map(|view| crate::world::rows_text(&view)),
                    crate::world::rows_text(
                        &world.terminals[terminal].snapshot()
                    ),
                ));
            }
            report.views_compared += 1;
        }
    }
    // Contract failures never happen; without damage no client resyncs.
    for (index, peer) in world.peers.iter().enumerate() {
        for event in &peer.log {
            if let ClientEvent::Closed(
                reason @ (CloseReason::Violation(_)
                | CloseReason::ContractFault(_)),
            ) = event
            {
                return Err(format!("peer {index} closed: {reason:?}"));
            }
            if let ClientEvent::Viewer(huterm_client::ViewerEvent::Fault {
                ..
            }) = event
            {
                return Err(format!("peer {index} faulted a viewer"));
            }
        }
        for reason in &peer.closes {
            match reason {
                crate::CloseReason::Stalled => report.stall_disconnects += 1,
                crate::CloseReason::Disconnected => {}
                other => {
                    return Err(format!(
                        "the server closed peer {index}: {other:?}"
                    ));
                }
            }
        }
        report.resyncs += peer.session.stats().viewer_resyncs;
    }
    if report.damaged == 0 && report.resyncs > 0 {
        return Err(format!("{} resyncs without damage", report.resyncs));
    }
    check_effects(&world, scenario, &admitted, &mut report)?;
    // Input reaches each terminal at most once and in order, and on a
    // healthy link every input arrives.
    for (terminal, model) in world.terminals.iter().enumerate() {
        let mut last: BTreeMap<usize, u64> = BTreeMap::new();
        for written in model.written() {
            let TerminalInput::Text(text) = &written.input else {
                continue;
            };
            let Some((peer, to, n)) = parse(text) else {
                return Err(format!("terminal {terminal} got {text:?}"));
            };
            if to != terminal {
                return Err(format!("terminal {terminal} got {text:?}"));
            }
            let previous = last.insert(peer, n).unwrap_or(0);
            if n <= previous {
                return Err(format!(
                    "terminal {terminal} got peer {peer}'s input {n} after {previous}"
                ));
            }
            if scenario.healthy && n != previous + 1 {
                return Err(format!(
                    "terminal {terminal} lost peer {peer}'s input {}",
                    previous + 1
                ));
            }
            report.inputs_written += 1;
        }
        if scenario.healthy {
            for peer in 0..scenario.peers {
                let sent = typed.get(&(peer, terminal)).copied().unwrap_or(0);
                let arrived = last.get(&peer).copied().unwrap_or(0);
                if arrived != sent {
                    return Err(format!(
                        "terminal {terminal} got {arrived} of peer {peer}'s {sent} inputs"
                    ));
                }
            }
        }
    }
    Ok(report)
}

/// Checks the host effects every peer received against what each terminal
/// admitted and to which viewer.
fn check_effects(
    world: &World,
    scenario: &Scenario,
    admitted: &BTreeMap<(usize, u64), ViewerId>,
    report: &mut Report,
) -> Result<(), String> {
    // Viewer numbers are unique across the world, so each names its peer.
    let mut owners: BTreeMap<u64, usize> = BTreeMap::new();
    for (index, peer) in world.peers.iter().enumerate() {
        for event in &peer.log {
            if let ClientEvent::Result {
                outcome:
                    Ok(Success {
                        response: Response::Viewer { viewer, .. },
                        ..
                    }),
                ..
            } = event
            {
                owners.insert(viewer.get(), index);
            }
        }
    }
    let mut delivered: BTreeSet<(usize, u64)> = BTreeSet::new();
    let mut last: BTreeMap<u64, u64> = BTreeMap::new();
    for (index, peer) in world.peers.iter().enumerate() {
        report.effects_discarded += peer.session.stats().effects_discarded;
        report.effects_expired += peer.effects_expired
            + peer
                .connection
                .as_ref()
                .map_or(0, |connection| connection.effect_stats().expired);
        for event in &peer.log {
            let ClientEvent::Viewer(ViewerEvent::HostEffect(delivery)) = event
            else {
                continue;
            };
            let EffectPayload::ClipboardWrite { text, .. } = &delivery.payload
            else {
                return Err(format!("peer {index} got {:?}", delivery.payload));
            };
            let Some(key) = parse_effect(text) else {
                return Err(format!("peer {index} got a write {text:.32?}"));
            };
            if !delivered.insert(key) {
                return Err(format!("effect {key:?} arrived twice"));
            }
            let Some(chosen) = admitted.get(&key) else {
                return Err(format!("effect {key:?} was never admitted"));
            };
            if chosen.get() != delivery.viewer.get()
                || owners.get(&chosen.get()) != Some(&index)
            {
                return Err(format!(
                    "effect {key:?} for viewer {} reached peer {index}'s viewer {}",
                    chosen.get(),
                    delivery.viewer.get()
                ));
            }
            let previous = last.insert(chosen.get(), key.1).unwrap_or(0);
            if key.1 <= previous {
                return Err(format!(
                    "viewer {} got effect {key:?} after {previous}",
                    chosen.get()
                ));
            }
            report.effects_delivered += 1;
        }
    }
    if scenario.healthy
        && let Some(lost) = admitted.keys().find(|key| !delivered.contains(key))
    {
        return Err(format!("effect {lost:?} never arrived on a healthy link"));
    }
    for (index, terminal) in world.terminals.iter().enumerate() {
        if terminal.effects_held() != (0, 0) {
            return Err(format!(
                "terminal {index} still holds {:?} for effects",
                terminal.effects_held()
            ));
        }
    }
    Ok(())
}

fn check_retained(world: &World) -> Result<(), String> {
    for (index, peer) in world.peers.iter().enumerate() {
        if let Some(connection) = &peer.connection
            && connection.retained() > connection.retained_bound()
        {
            return Err(format!(
                "peer {index}'s connection retains {} bytes, over {}",
                connection.retained(),
                connection.retained_bound()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seeds per scenario in the normal suite.
    const SEEDS: u64 = 3;

    fn check(scenario: &Scenario, seeds: std::ops::Range<u64>) -> Vec<Report> {
        seeds
            .map(|seed| {
                run(scenario, seed).unwrap_or_else(|error| {
                    panic!(
                        "{} seed {seed}: {error}\nreplay: mise run faults:ipc -- {} {seed}",
                        scenario.name, scenario.name
                    )
                })
            })
            .collect()
    }

    #[test]
    fn every_scenario_holds_its_invariants_over_a_few_seeds() {
        for scenario in scenarios() {
            let reports = check(&scenario, 0..SEEDS);
            let compared: usize =
                reports.iter().map(|report| report.views_compared).sum();
            assert!(compared > 0, "{} compared no views", scenario.name);
            let written: usize =
                reports.iter().map(|report| report.inputs_written).sum();
            assert!(written > 0, "{} wrote no input", scenario.name);
            let effects: usize =
                reports.iter().map(|report| report.effects_delivered).sum();
            assert!(effects > 0, "{} delivered no effect", scenario.name);
            if !scenario.healthy {
                assert!(
                    reports.iter().any(|report| report.permission_changes > 0),
                    "{} changed no permission",
                    scenario.name
                );
            }
            if scenario.cut > 0 {
                assert!(
                    reports.iter().any(|report| report.cuts > 0),
                    "{} cut nothing",
                    scenario.name
                );
            }
            if scenario.end > 0 {
                assert!(
                    reports.iter().any(|report| report.revoked > 0)
                        && reports.iter().any(|report| report.closed > 0),
                    "{} ended no viewer",
                    scenario.name
                );
            }
            if scenario.long_stall >= 100 {
                assert!(
                    reports.iter().any(|report| report.stall_disconnects > 0),
                    "{} disconnected no stalled reader",
                    scenario.name
                );
            }
            if scenario.damage > 0 {
                let damaged: u64 =
                    reports.iter().map(|report| report.damaged).sum();
                let resyncs: u64 =
                    reports.iter().map(|report| report.resyncs).sum();
                assert!(damaged > 0, "{} damaged nothing", scenario.name);
                assert!(
                    resyncs > 0,
                    "{}: digests caught no damage",
                    scenario.name
                );
            }
        }
    }

    #[test]
    fn swarm_draws_hold_their_invariants_over_a_few_seeds() {
        for seed in 0..2 * SEEDS {
            let scenario = swarm(seed);
            check(&scenario, seed..seed + 1);
        }
    }

    /// Many seeds per scenario, or one replayed: `mise run faults:ipc`
    /// runs it in release with `HUTERM_FAULT_SEEDS`, or with
    /// `HUTERM_FAULT_REPLAY="SCENARIO SEED"`.
    #[test]
    #[ignore = "a long run; use mise run faults:ipc"]
    fn fault_injection_long_run() {
        if let Ok(replay) = std::env::var("HUTERM_FAULT_REPLAY") {
            let mut parts = replay.split_whitespace();
            let name = parts.next().expect("a scenario name");
            let seed: u64 = parts
                .next()
                .and_then(|seed| seed.parse().ok())
                .expect("a seed");
            let scenario = if name == "swarm" {
                swarm(seed)
            } else {
                scenarios()
                    .into_iter()
                    .find(|scenario| scenario.name == name)
                    .expect("a known scenario")
            };
            println!("{scenario:#?}");
            println!("{:#?}", check(&scenario, seed..seed + 1));
            return;
        }
        let seeds: u64 = std::env::var("HUTERM_FAULT_SEEDS")
            .ok()
            .and_then(|seeds| seeds.parse().ok())
            .unwrap_or(200);
        for scenario in scenarios() {
            let reports = check(&scenario, 0..seeds);
            println!(
                "faults scenario={} seeds={seeds} cuts={} stalls={} stall_disconnects={} revoked={} closed={} damaged={} resyncs={} inputs={} effects={}/{} discarded={} expired={} permission_changes={}",
                scenario.name,
                reports.iter().map(|r| r.cuts).sum::<usize>(),
                reports.iter().map(|r| r.stalls).sum::<usize>(),
                reports.iter().map(|r| r.stall_disconnects).sum::<usize>(),
                reports.iter().map(|r| r.revoked).sum::<usize>(),
                reports.iter().map(|r| r.closed).sum::<usize>(),
                reports.iter().map(|r| r.damaged).sum::<u64>(),
                reports.iter().map(|r| r.resyncs).sum::<u64>(),
                reports.iter().map(|r| r.inputs_written).sum::<usize>(),
                reports.iter().map(|r| r.effects_delivered).sum::<usize>(),
                reports.iter().map(|r| r.effects_admitted).sum::<usize>(),
                reports.iter().map(|r| r.effects_discarded).sum::<u64>(),
                reports.iter().map(|r| r.effects_expired).sum::<u64>(),
                reports.iter().map(|r| r.permission_changes).sum::<usize>(),
            );
        }
        for seed in 0..seeds {
            check(&swarm(seed), seed..seed + 1);
        }
        println!("faults scenario=swarm seeds={seeds} ok");
    }
}
