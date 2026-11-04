//! The world: a deterministic scheduler that runs one server incarnation
//! after another, several clients, and the broker on a virtual clock, then
//! checks the design's invariants.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::broker::{Link, LinkProfile};
use crate::client::{Client, Outcome};
use crate::message::{ClientKind, Request, RetryClass};
use crate::model::{MINOR_0, MINOR_1};
use crate::profile::{Design, Profile};
use crate::rng::Rng;
use crate::server::{Counters, Record, Server};

/// A run's shape: versions, link behavior, and how hostile the world is.
#[derive(Clone, Debug)]
pub struct Scenario {
    /// Name, for reports.
    pub name: &'static str,
    /// The server's minor version.
    pub server_minor: u16,
    /// Each client's minor version and kind.
    pub clients: Vec<(u16, ClientKind)>,
    /// Link behavior.
    pub link: LinkProfile,
    /// Chance per thousand fault draws, one every ten milliseconds, per
    /// client, of a cut.
    pub cut_per_mille: u64,
    /// Chance per thousand fault draws, per client, of a reader stall.
    pub stall_per_mille: u64,
    /// Chance per thousand fault draws of a daemon restart.
    pub restart_per_mille: u64,
    /// Chance per thousand ticks, per client, of a workload action.
    pub action_per_mille: u64,
    /// Chance per thousand ticks of terminal activity.
    pub activity_per_mille: u64,
    /// Whether clients toggle clipboard permission.
    pub toggle_permission: bool,
    /// Chance per thousand that a client damages a decoded delta or event
    /// in a way that still decodes, so digests must catch it.
    pub damage_per_mille: u64,
    /// Whether nothing interferes with delivery, so every effect and input
    /// for a live terminal must arrive and no connection may end.
    pub healthy: bool,
    /// The limits both ends use.
    pub profile: Profile,
    /// Milliseconds of activity before quiescence.
    pub duration: u64,
    /// Milliseconds allowed to reach quiescence.
    pub quiesce: u64,
}

fn local_link() -> LinkProfile {
    LinkProfile {
        latency: 1,
        jitter: 1,
        chunk: 64,
        buffer: 256,
        corrupt_per_mille: 0,
    }
}

fn remote_link() -> LinkProfile {
    LinkProfile {
        latency: 80,
        jitter: 120,
        chunk: 5,
        buffer: 512,
        corrupt_per_mille: 0,
    }
}

/// Every scenario the tests and the program run.
#[must_use]
#[expect(clippy::too_many_lines, reason = "every scenario, listed once")]
pub fn scenarios() -> Vec<Scenario> {
    let desktop = |minor| (minor, ClientKind::Desktop);
    let base = Scenario {
        name: "clean",
        server_minor: MINOR_0,
        clients: vec![
            desktop(MINOR_0),
            desktop(MINOR_0),
            (MINOR_0, ClientKind::Text),
        ],
        link: local_link(),
        cut_per_mille: 0,
        stall_per_mille: 0,
        restart_per_mille: 0,
        action_per_mille: 60,
        activity_per_mille: 300,
        toggle_permission: false,
        damage_per_mille: 0,
        healthy: true,
        profile: Profile::default(),
        duration: 3_000,
        quiesce: 20_000,
    };
    let faulty = Scenario {
        healthy: false,
        ..base.clone()
    };
    vec![
        base.clone(),
        Scenario {
            name: "local-faults",
            cut_per_mille: 20,
            stall_per_mille: 20,
            toggle_permission: true,
            link: LinkProfile {
                chunk: 7,
                buffer: 96,
                ..local_link()
            },
            ..faulty.clone()
        },
        Scenario {
            name: "remote",
            link: remote_link(),
            cut_per_mille: 4,
            stall_per_mille: 4,
            restart_per_mille: 1,
            toggle_permission: true,
            duration: 12_000,
            ..faulty.clone()
        },
        // Slow but fault free: every effect must still arrive, which needs
        // effect frames to go first at the writer.
        Scenario {
            name: "remote-healthy",
            link: remote_link(),
            duration: 8_000,
            ..base.clone()
        },
        // Payloads larger than the outbound budget: memory stays bounded
        // only if messages are encoded one frame at a time.
        Scenario {
            name: "large-payloads",
            profile: Profile {
                clipboard_payload: 2_048,
                selection_payload: 2_048,
                terminal_effect_bytes: 4_096,
                ..Profile::default()
            },
            ..base.clone()
        },
        Scenario {
            name: "newer-client",
            clients: vec![desktop(MINOR_1), desktop(MINOR_0), desktop(MINOR_1)],
            cut_per_mille: 10,
            ..faulty.clone()
        },
        Scenario {
            name: "newer-server",
            server_minor: MINOR_1,
            clients: vec![
                desktop(MINOR_0),
                desktop(MINOR_1),
                (MINOR_0, ClientKind::Text),
            ],
            cut_per_mille: 10,
            ..faulty.clone()
        },
        // Damage that decodes, on intact links: only digests can catch it,
        // and every caught delta or event must recover by resync.
        Scenario {
            name: "state-corruption",
            damage_per_mille: 30,
            profile: Profile {
                update_digest_every: 1,
                hierarchy_digest_every: 1,
                ..Profile::default()
            },
            ..base.clone()
        },
        Scenario {
            name: "corruption",
            link: LinkProfile {
                corrupt_per_mille: 4,
                ..local_link()
            },
            toggle_permission: true,
            // Periodic digests find a corrupted delta only at the next
            // digest; checking every update makes detection complete.
            profile: Profile {
                update_digest_every: 1,
                hierarchy_digest_every: 1,
                ..Profile::default()
            },
            ..faulty
        },
    ]
}

/// A scenario drawn at random from `seed`, for swarm testing: limits, fault
/// rates, link behavior, versions, and client kinds all vary, so bugs that
/// need an unusual combination have a chance to appear. The bounds keep
/// every drawn configuration valid: limits leave room for the workload's
/// largest message, and deadlines leave room for the link's round trip. A
/// healthy draw's link also carries the profile's minimum effect
/// throughput; other draws may be slower, and their effects miss deadlines.
#[must_use]
pub fn swarm(seed: u64) -> Scenario {
    let mut rng = Rng::new(seed ^ 0x5157_4152_4d53_4545);
    let r = |rng: &mut Rng, low: u64, high: u64| {
        usize::try_from(rng.range(low, high)).unwrap_or(1)
    };
    let minor = |rng: &mut Rng| if rng.chance(500) { MINOR_1 } else { MINOR_0 };
    let remote = rng.chance(300);
    let latency = if remote {
        rng.range(20, 150)
    } else {
        rng.range(0, 3)
    };
    let jitter = if remote {
        rng.range(0, 150)
    } else {
        rng.range(0, 3)
    };
    let mut link = LinkProfile {
        latency,
        jitter,
        chunk: r(&mut rng, 1, 80),
        buffer: r(&mut rng, 32, 1_024),
        corrupt_per_mille: 0,
    };
    let healthy = rng.chance(250);
    let fault =
        |rng: &mut Rng, high: u64| if healthy { 0 } else { rng.range(0, high) };
    let cut_per_mille = fault(&mut rng, 25);
    let stall_per_mille = fault(&mut rng, 25);
    let restart_per_mille = fault(&mut rng, 4);
    let clipboard_payload = r(&mut rng, 1, 600);
    let clients = (0..rng.range(1, 4))
        .map(|_| {
            let kind = if rng.chance(750) {
                ClientKind::Desktop
            } else {
                ClientKind::Text
            };
            (minor(&mut rng), kind)
        })
        .collect();
    // A round trip bounds how fast a handshake, an effect's first frame,
    // and a stalled reader can be judged.
    let round_trip = 2 * (latency + jitter) + 10;
    let profile = Profile {
        max_frame: r(&mut rng, 12, 160),
        // One stream is reserved for effects.
        max_open_streams: r(&mut rng, 2, 6),
        max_name_bytes: r(&mut rng, 12, 24),
        max_tabs: r(&mut rng, 1, 10),
        max_status_text: r(&mut rng, 0, 24),
        max_rows: r(&mut rng, 1, 6),
        max_columns: r(&mut rng, 2, 9),
        input_payload: r(&mut rng, 12, 48),
        clipboard_payload,
        selection_payload: r(&mut rng, 1, 600),
        credit: rng.range(1, 3),
        outbound_budget: r(&mut rng, 32, 2_048),
        stall_ms: rng.range(500, 3_000).max(4 * round_trip),
        idle_ms: rng.range(500, 3_000).max(4 * round_trip),
        handshake_ms: rng.range(200, 800).max(round_trip),
        effect_first_ms: rng.range(50, 400).max(2 * round_trip),
        effect_bytes_per_ms: rng.range(1, 4),
        terminal_effects: r(&mut rng, 1, 10),
        terminal_effect_bytes: clipboard_payload + r(&mut rng, 0, 600),
        viewer_input_budget: r(&mut rng, 12, 96),
        pty_drain_per_tick: r(&mut rng, 1, 16),
        hierarchy_lane: r(&mut rng, 16, 512),
        hierarchy_queue: r(&mut rng, 1, 10),
        hierarchy_digest_every: rng.range(1, 8),
        update_digest_every: rng.range(1, 8),
        max_in_flight: r(&mut rng, 1, 10),
        max_viewers: r(&mut rng, 1, 16),
        design: Design::default(),
    };
    // Only a link that carries the effect throughput can promise that every
    // effect arrives.
    if healthy {
        link.buffer = link.buffer.max(effect_buffer_floor(&link, &profile));
    }
    Scenario {
        name: "swarm",
        server_minor: minor(&mut rng),
        clients,
        link,
        cut_per_mille,
        stall_per_mille,
        restart_per_mille,
        action_per_mille: rng.range(10, 120),
        activity_per_mille: rng.range(50, 600),
        toggle_permission: !healthy && rng.chance(500),
        damage_per_mille: 0,
        healthy,
        profile,
        duration: rng.range(1_000, 6_000),
        quiesce: 60_000,
    }
}

/// The smallest socket buffer with which `link` carries the profile's
/// minimum effect throughput, so a healthy run's effects meet their
/// completion deadlines.
///
/// A byte holds buffer space from the tick it is written until it arrives,
/// up to `latency + jitter` later, so the link sustains `buffer / cycle`
/// bytes per millisecond. Effect bytes travel in frames with a four-byte
/// length and a flags-and-stream header. The margin of 4 leaves the effect
/// a quarter of the link, halving twice: the writer alternates effect
/// frames with ordinary ones at frame boundaries, and it builds one effect
/// frame per tick, while room returns in bursts after latency and jitter,
/// and ordinary frames take the rest of each burst. A margin of 2 still
/// lost effects in healthy swarm draws.
fn effect_buffer_floor(link: &LinkProfile, profile: &Profile) -> usize {
    const MARGIN: u64 = 4;
    let frame = profile.max_frame as u64;
    let cycle = link.latency + link.jitter + 1;
    let wire = MARGIN * profile.effect_bytes_per_ms * cycle * (frame + 4)
        / (frame - 2);
    usize::try_from(wire).unwrap_or(usize::MAX)
}

/// Totals from one run.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Milliseconds run, including quiescence.
    pub ticks: u64,
    /// Connections cut by the broker.
    pub cuts: u64,
    /// Connections the server closed.
    pub server_closes: u64,
    /// Connections a client closed after a fault.
    pub client_closes: u64,
    /// Daemon restarts.
    pub restarts: u64,
    /// Mutations executed.
    pub executed: usize,
    /// Mutations whose outcome was unknown.
    pub unknown: usize,
    /// Effects executed.
    pub effects: usize,
    /// Effects dropped by deadlines or revocation.
    pub dropped: u64,
    /// Inputs that reached a PTY.
    pub inputs: usize,
    /// Connections the server closed for stalling.
    pub stalls: usize,
    /// Live views compared with the server at the end.
    pub views_checked: usize,
    /// Stopped terminals' final views compared at the end.
    pub stopped_checked: usize,
    /// Status baselines compared at the end.
    pub statuses_checked: usize,
    /// Faults, violations, and resyncs that detected damage, under
    /// corruption.
    pub detections: usize,
    /// Damaged deltas and events clients injected.
    pub damage_injected: usize,
    /// Rare-path counters, summed over incarnations.
    pub counters: Counters,
}

impl Report {
    /// Adds another run's totals.
    pub fn add(&mut self, other: &Report) {
        self.ticks += other.ticks;
        self.cuts += other.cuts;
        self.server_closes += other.server_closes;
        self.client_closes += other.client_closes;
        self.restarts += other.restarts;
        self.executed += other.executed;
        self.unknown += other.unknown;
        self.effects += other.effects;
        self.dropped += other.dropped;
        self.inputs += other.inputs;
        self.stalls += other.stalls;
        self.views_checked += other.views_checked;
        self.stopped_checked += other.stopped_checked;
        self.statuses_checked += other.statuses_checked;
        self.detections += other.detections;
        self.damage_injected += other.damage_injected;
        add_counters(&mut self.counters, &other.counters);
    }
}

fn add_counters(total: &mut Counters, other: &Counters) {
    total.downgrade_skipped += other.downgrade_skipped;
    total.downgrade_single += other.downgrade_single;
    total.downgrade_resync += other.downgrade_resync;
    total.lag_resyncs += other.lag_resyncs;
    total.busy += other.busy;
    total.stopped_subscribes += other.stopped_subscribes;
    total.stopped_closes += other.stopped_closes;
    total.deadline_drops += other.deadline_drops;
}

struct World {
    scenario: Scenario,
    profile: Rc<Profile>,
    rng: Rng,
    now: u64,
    server: Server,
    incarnation: u64,
    records: Vec<(u64, Record)>,
    links: BTreeMap<usize, Link>,
    next_link: usize,
    clients: Vec<Client>,
    reconnect_at: Vec<u64>,
    report: Report,
}

impl World {
    fn new(scenario: &Scenario, seed: u64) -> Self {
        let profile = Rc::new(scenario.profile.clone());
        let clients = scenario
            .clients
            .iter()
            .enumerate()
            .map(|(index, (minor, kind))| {
                Client::new(
                    index as u64 + 1,
                    *minor,
                    *kind,
                    Rc::clone(&profile),
                )
            })
            .collect::<Vec<_>>();
        let mut clients = clients;
        if scenario.damage_per_mille > 0 {
            for client in &mut clients {
                client
                    .inject_damage(scenario.damage_per_mille, seed ^ client.id);
            }
        }
        let count = clients.len();
        Self {
            scenario: scenario.clone(),
            server: Server::new(
                1,
                77,
                scenario.server_minor,
                Rc::clone(&profile),
            ),
            profile,
            rng: Rng::new(seed),
            now: 0,
            incarnation: 1,
            records: Vec::new(),
            links: BTreeMap::new(),
            next_link: 0,
            clients,
            reconnect_at: vec![0; count],
            report: Report::default(),
        }
    }

    fn cut(&mut self, client: usize) {
        let Some(link) = self.clients[client].link() else {
            return;
        };
        self.links.remove(&link);
        self.server.disconnect(link);
        self.clients[client].disconnected();
        self.reconnect_at[client] = self.now + 1 + self.rng.below(200);
    }

    fn restart(&mut self) {
        for client in 0..self.clients.len() {
            self.cut(client);
        }
        self.incarnation += 1;
        let fresh = Server::new(
            self.incarnation,
            77,
            self.scenario.server_minor,
            Rc::clone(&self.profile),
        );
        let old = std::mem::replace(&mut self.server, fresh);
        self.records.push((self.incarnation - 1, old.record));
        self.report.restarts += 1;
    }

    fn act(&mut self, index: usize) {
        let rng = &mut self.rng;
        let client = &mut self.clients[index];
        if !client.is_open() {
            return;
        }
        let tabs = client.known_tabs();
        let pick = |rng: &mut Rng| {
            (!tabs.is_empty()).then(|| tabs[rng.index(tabs.len())])
        };
        let name = |rng: &mut Rng| -> String {
            let len = 1 + rng.index(12);
            (0..len)
                .map(|_| {
                    char::from(b'a' + u8::try_from(rng.below(26)).unwrap_or(0))
                })
                .collect()
        };
        match rng.below(100) {
            0..=9 => client.request(Request::Open { name: name(rng) }),
            10..=19 => {
                if let Some(tab) = pick(rng) {
                    client.request(Request::Rename {
                        tab,
                        name: name(rng),
                    });
                }
            }
            20..=24 => {
                if let Some(tab) = pick(rng) {
                    client.request(Request::Move {
                        tab,
                        index: rng.index(tabs.len()),
                    });
                }
            }
            25..=26 => {
                if let Some(tab) = pick(rng) {
                    client.request(Request::Close { tab });
                }
            }
            27..=29 => {
                if let Some(tab) = pick(rng) {
                    client.request(Request::Pin {
                        tab,
                        pinned: rng.chance(500),
                    });
                }
            }
            30 => client.request(Request::Reverse),
            31..=34 => client.request(Request::ListTabs),
            35..=49 => {
                if let Some(tab) = pick(rng) {
                    client.show(tab);
                }
            }
            50..=59 => {
                let viewed = client.viewed_tabs();
                if !viewed.is_empty() {
                    client.focus(viewed[rng.index(viewed.len())]);
                }
            }
            60..=84 => {
                let viewed = client.viewed_tabs();
                if !viewed.is_empty() {
                    let tab = client.visible.unwrap_or(viewed[0]);
                    let text = name(rng);
                    client.type_text(tab, text);
                }
            }
            85..=89 if self.scenario.toggle_permission => {
                let viewed = client.viewed_tabs();
                if !viewed.is_empty() {
                    client.set_permission(
                        viewed[rng.index(viewed.len())],
                        rng.chance(600),
                    );
                }
            }
            _ => {
                let viewed = client.viewed_tabs();
                if !viewed.is_empty() {
                    client.select(viewed[rng.index(viewed.len())]);
                }
            }
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one tick of the whole world, in order"
    )]
    fn step(&mut self, active: bool) -> Result<(), String> {
        let now = self.now;
        let mut link_profile = self.scenario.link.clone();
        if !active {
            link_profile.corrupt_per_mille = 0;
        }
        // Faults are drawn every tenth tick.
        let faults = active && now.is_multiple_of(10);
        if active {
            for client in 0..self.clients.len() {
                if faults
                    && self.rng.chance(self.scenario.cut_per_mille)
                    && self.clients[client].link().is_some()
                {
                    self.report.cuts += 1;
                    self.cut(client);
                }
                if faults
                    && self.rng.chance(self.scenario.stall_per_mille)
                    && let Some(link) = self.clients[client]
                        .link()
                        .and_then(|id| self.links.get_mut(&id))
                {
                    let until = now + self.rng.below(self.profile.stall_ms * 2);
                    if self.rng.chance(500) {
                        link.stalled_until = until;
                    } else {
                        link.trickle_until = until;
                    }
                }
                if self.rng.chance(self.scenario.action_per_mille) {
                    self.act(client);
                }
            }
            if faults && self.rng.chance(self.scenario.restart_per_mille) {
                self.restart();
            }
            if self.rng.chance(self.scenario.activity_per_mille) {
                self.server.terminal_activity(&mut self.rng, now);
            }
        }
        for client in 0..self.clients.len() {
            // A client gives up on a handshake that never completes.
            if self.clients[client]
                .handshake_overdue(now, self.profile.handshake_ms * 4)
            {
                self.report.client_closes += 1;
                self.cut(client);
            }
            if self.clients[client].link().is_none()
                && now >= self.reconnect_at[client]
            {
                self.next_link += 1;
                let id = self.next_link;
                self.links.insert(id, Link::default());
                self.server.accept(id, self.clients[client].id, now);
                self.clients[client].connect(id, now);
            }
        }
        for client in 0..self.clients.len() {
            let Some(id) = self.clients[client].link() else {
                continue;
            };
            let Some(link) = self.links.get_mut(&id) else {
                continue;
            };
            let cap = if self.server.reading_paused(id) {
                0
            } else {
                usize::MAX
            };
            let bytes = Link::deliver(
                &link_profile,
                &mut link.up,
                now,
                cap,
                &mut self.rng,
            );
            if !bytes.is_empty() {
                self.server.receive(id, &bytes, now);
            }
            let cap = if now < link.stalled_until {
                0
            } else if now < link.trickle_until {
                1
            } else {
                usize::MAX
            };
            let bytes = Link::deliver(
                &link_profile,
                &mut link.down,
                now,
                cap,
                &mut self.rng,
            );
            if !bytes.is_empty()
                && let Err(error) = self.clients[client].receive(&bytes, now)
            {
                if link_profile.corrupt_per_mille == 0 && error != "rejected" {
                    let id = self.clients[client].id;
                    return Err(format!("client {id} fault at {now}: {error}"));
                }
                self.report.client_closes += 1;
                self.cut(client);
            }
        }
        self.server.tick(now);
        for client in 0..self.clients.len() {
            self.clients[client].tick();
            let Some(id) = self.clients[client].link() else {
                continue;
            };
            if self.server.is_closed(id) {
                self.report.server_closes += 1;
                self.cut(client);
                continue;
            }
            let Some(link) = self.links.get_mut(&id) else {
                continue;
            };
            let room = Link::room(&link_profile, &link.down);
            let bytes = self.server.take(id, room, now);
            if !bytes.is_empty() {
                Link::send(
                    &link_profile,
                    &mut link.down,
                    &bytes,
                    now,
                    &mut self.rng,
                );
            }
            let room = Link::room(&link_profile, &link.up);
            let bytes = self.clients[client].take(room);
            if !bytes.is_empty() {
                Link::send(
                    &link_profile,
                    &mut link.up,
                    &bytes,
                    now,
                    &mut self.rng,
                );
            }
        }
        // Encoded bytes stay within the budget plus one ordinary frame and
        // the effect lane's one frame, and nothing is held encoded outside
        // the queue.
        let frame = self.profile.max_frame + 4;
        if self.server.record.max_over_budget > 2 * frame {
            let over = self.server.record.max_over_budget;
            return Err(format!(
                "outbound queue exceeded its budget by {over}"
            ));
        }
        if self.server.record.max_retained > 0 {
            return Err(
                "the server held an encoded message outside its queue".into()
            );
        }
        if self.server.record.max_reassembly > self.profile.server_reassembly()
        {
            return Err("reassembly exceeded its budget".into());
        }
        self.now += 1;
        Ok(())
    }

    fn quiet(&self) -> bool {
        self.clients.iter().all(|client| {
            client.is_open() && client.unresolved() == 0 && !client.busy()
        }) && self
            .links
            .values()
            .all(|link| link.up.is_empty() && link.down.is_empty())
            && self.links.keys().all(|id| !self.server.busy(*id))
            && !self.server.pending_work()
    }

    fn why_busy(&self) -> String {
        let mut out = Vec::new();
        for client in &self.clients {
            if !client.is_open() {
                out.push(format!(
                    "client {} not open (link {:?})",
                    client.id,
                    client.link()
                ));
            }
            if client.unresolved() > 0 {
                out.push(format!(
                    "client {} unresolved {}",
                    client.id,
                    client.unresolved()
                ));
            }
            if client.busy() {
                out.push(format!("client {} busy", client.id));
            }
        }
        for (id, link) in &self.links {
            if !link.up.is_empty() || !link.down.is_empty() {
                out.push(format!(
                    "link {id} up {} down {}",
                    link.up.len(),
                    link.down.len()
                ));
            }
            if self.server.busy(*id) {
                out.push(format!(
                    "server conn {id} busy: {}",
                    self.server.debug_viewers(*id)
                ));
            }
        }
        if self.server.pending_work() {
            out.push("server pending work".into());
        }
        out.join("; ")
    }

    fn run(mut self) -> Result<Report, String> {
        while self.now < self.scenario.duration {
            self.step(true)?;
        }
        for client in &mut self.clients {
            client.visible = None;
        }
        for link in self.links.values_mut() {
            link.stalled_until = 0;
            link.trickle_until = 0;
        }
        // Undigested state on a link that carried damaged bytes, such as an
        // input acknowledgement, may be wrong; the client reconnects, as one
        // that saw a fault would. Clean links keep their connections, so
        // their final state shows whether digests caught the damage.
        let damaged: Vec<usize> = (0..self.clients.len())
            .filter(|&client| {
                self.clients[client]
                    .link()
                    .and_then(|id| self.links.get(&id))
                    .is_some_and(|link| {
                        link.up.corrupted || link.down.corrupted
                    })
            })
            .collect();
        for client in damaged {
            self.cut(client);
        }
        let deadline = self.now + self.scenario.quiesce;
        let mut calm = 0;
        while calm < 50 {
            if self.now >= deadline {
                return Err(format!(
                    "no quiescence by {}: {}",
                    self.now,
                    self.why_busy()
                ));
            }
            self.step(false)?;
            calm = if self.quiet() { calm + 1 } else { 0 };
        }
        self.check()?;
        self.report.ticks = self.now;
        Ok(self.report)
    }

    #[expect(clippy::too_many_lines, reason = "every invariant, listed once")]
    fn check(&mut self) -> Result<(), String> {
        let incarnation = self.incarnation;
        // Damaged links can corrupt anything; damaged state only touches
        // digested deltas and events, so every other check still holds.
        let corrupt = self.scenario.link.corrupt_per_mille > 0;
        let damaged = corrupt || self.scenario.damage_per_mille > 0;
        let healthy = self.scenario.healthy;
        for client in &self.clients {
            let minor = client.negotiated().ok_or("client not connected")?;
            let id = client.id;
            let expected = self.server.hierarchy.project(minor);
            if client.hierarchy.as_ref() != Some(&expected) {
                return Err(format!(
                    "client {id} hierarchy {:?} != server {expected:?}",
                    client.hierarchy
                ));
            }
            let mut statuses = Vec::new();
            for tab in client.viewed_tabs() {
                let Some((view, _, stopped)) = self.server.terminal(tab, minor)
                else {
                    continue;
                };
                if stopped {
                    continue;
                }
                if client.view(tab) != Some(&view) {
                    let mine = client
                        .view(tab)
                        .map(|v| (v.revision, v.geometry, v.columns));
                    return Err(format!(
                        "client {id} tab {tab} view diverged: client {mine:?} server {:?}; {}",
                        (view.revision, view.geometry, view.columns),
                        client.debug_viewer(tab),
                    ));
                }
                self.report.views_checked += 1;
                statuses.push(tab);
            }
            // A stopped terminal's final status and view stay the client's.
            for (&(stopped_in, tab), view) in &client.stopped {
                if stopped_in != incarnation {
                    continue;
                }
                let Some((final_view, _, true)) =
                    self.server.terminal(tab, minor)
                else {
                    continue;
                };
                // A view damaged before its terminal stopped cannot be
                // repaired once the viewer closed; corruption can do that.
                if !corrupt && view.as_ref() != Some(&final_view) {
                    return Err(format!(
                        "client {id} tab {tab} lost its stopped terminal's view"
                    ));
                }
                self.report.stopped_checked += 1;
                statuses.push(tab);
            }
            // So does a stopped terminal the client learned of only from a
            // `Stopped` reply, after subscribing to it.
            // A tab the client stopped tracking, such as one whose
            // resubscription was refused, makes no claim.
            for &(seen_in, tab) in &client.stopped_replies {
                let stopped =
                    self.server.terminal(tab, minor).is_some_and(|(_, _, s)| s);
                if seen_in == incarnation && stopped && !statuses.contains(&tab)
                {
                    statuses.push(tab);
                }
            }
            for tab in statuses {
                let Some((_, status, _)) = self.server.terminal(tab, minor)
                else {
                    continue;
                };
                let seen = client
                    .seen
                    .get(&(incarnation, tab))
                    .cloned()
                    .unwrap_or_default();
                // Status carries no digest, so corruption can alter it.
                // Counts are deltas since the client's first read.
                let last = status.failures.last().map_or(0, |f| f.0);
                let exited = status.exited.is_some() && !seen.first_exited;
                if !corrupt
                    && (seen.revision != status.revision
                        || seen.first_bells + seen.bells != status.bells
                        || seen.exits != u32::from(exited)
                        || seen.failures != last - seen.first_failure)
                {
                    return Err(format!(
                        "client {id} tab {tab} status {seen:?} vs {status:?}"
                    ));
                }
                self.report.statuses_checked += 1;
            }
            if !corrupt && !client.faults.is_empty() {
                return Err(format!("client {id} faults {:?}", client.faults));
            }
            // With ordering lanes, a gap or digest mismatch needs damage.
            if !damaged
                && (client.hierarchy_resyncs > 0 || client.viewer_resyncs > 0)
            {
                return Err(format!(
                    "client {id} asked for {} hierarchy and {} viewer resyncs without corruption",
                    client.hierarchy_resyncs, client.viewer_resyncs
                ));
            }
            self.report.detections += client.faults.len()
                + client.hierarchy_resyncs as usize
                + client.viewer_resyncs as usize;
            self.report.damage_injected += client.damage_injected as usize;
        }
        let mut records = std::mem::take(&mut self.records);
        records.push((incarnation, std::mem::take(&mut self.server.record)));
        // Request and effect identities carry no digest, so a corrupted ID
        // can still decode. The design relies on the transport for byte
        // integrity; under corruption only digested state is checked.
        let mut executed = BTreeSet::new();
        let mut intents: BTreeMap<(u64, u64), u32> = BTreeMap::new();
        for (_, record) in &records {
            for &(client, id) in &record.executed {
                executed.insert((client, id));
                let intent = self
                    .clients
                    .iter()
                    .find(|c| c.id == client)
                    .and_then(|c| c.intents.get(&id))
                    .copied()
                    .unwrap_or(id);
                *intents.entry((client, intent)).or_default() += 1;
            }
            let violations: Vec<_> = record
                .violations
                .iter()
                .filter(|(_, why)| why != "writer stalled")
                .collect();
            if !corrupt && !violations.is_empty() {
                return Err(format!("protocol violations {violations:?}"));
            }
            if healthy && !record.violations.is_empty() {
                return Err(format!(
                    "connections ended in a healthy run: {:?}",
                    record.violations
                ));
            }
            self.report.dropped += record.dropped;
            self.report.inputs += record.pty.len();
            self.report.stalls += record
                .violations
                .iter()
                .filter(|(_, why)| why == "writer stalled")
                .count();
            self.report.detections += record.violations.len();
            add_counters(&mut self.report.counters, &record.counters);
        }
        self.report.executed = executed.len();
        if !corrupt
            && let Some(((client, intent), count)) =
                intents.iter().find(|(_, count)| **count > 1)
        {
            return Err(format!(
                "client {client} intent {intent} ran {count} times"
            ));
        }
        for client in self.clients.iter().filter(|_| !corrupt) {
            for (id, request, outcome) in &client.outcomes {
                let ran = executed.contains(&(client.id, *id));
                let mutation = request.class() == RetryClass::Mutation;
                match outcome {
                    Outcome::Ok | Outcome::Committed if mutation && !ran => {
                        return Err(format!(
                            "client {} request {id} committed but never ran",
                            client.id
                        ));
                    }
                    Outcome::Err(_) if mutation && ran => {
                        return Err(format!(
                            "client {} request {id} failed but ran",
                            client.id
                        ));
                    }
                    Outcome::Unknown => self.report.unknown += 1,
                    _ => {}
                }
            }
        }
        // Each effect runs at most once, on the connection core chose.
        let mut admitted = BTreeMap::new();
        for (incarnation, record) in &records {
            for (terminal, origin, conn, _) in &record.admitted {
                admitted.insert((*incarnation, *terminal, *origin), *conn);
            }
        }
        let mut ran = BTreeSet::new();
        for client in self.clients.iter().filter(|_| !corrupt) {
            for (incarnation, link, terminal, origin) in &client.executed {
                let key = (*incarnation, *terminal, *origin);
                if !ran.insert(key) {
                    return Err(format!("effect {key:?} executed twice"));
                }
                if admitted.get(&key) != Some(link) {
                    return Err(format!(
                        "effect {key:?} ran on link {link}, not the one core chose"
                    ));
                }
            }
        }
        self.report.effects = ran.len();
        let live = |tab: u64| {
            self.server
                .terminal(tab, MINOR_0)
                .is_some_and(|(_, _, stopped)| !stopped)
        };
        if healthy {
            // Effects for a closed or stopped terminal are legitimately lost.
            let expected = admitted
                .keys()
                .filter(|(i, t, _)| *i == incarnation && live(*t))
                .count();
            let done = ran
                .iter()
                .filter(|(i, t, _)| *i == incarnation && live(*t))
                .count();
            if done != expected {
                return Err(format!(
                    "healthy run lost effects: {done} of {expected} ran"
                ));
            }
        }
        // Input reaches the PTY once, in order, per viewer.
        for (incarnation, record) in &records {
            let mut last: BTreeMap<(u64, u64, u64), u64> = BTreeMap::new();
            for (tab, client, viewer, seq, _) in &record.pty {
                let previous =
                    last.insert((*tab, *client, *viewer), *seq).unwrap_or(0);
                if *seq != previous + 1 {
                    return Err(format!(
                        "input {seq} after {previous} on incarnation {incarnation}"
                    ));
                }
            }
            if healthy {
                // Input for a closed or stopped terminal is legitimately lost.
                let typed = self
                    .clients
                    .iter()
                    .flat_map(|c| &c.typed)
                    .filter(|t| live(t.1))
                    .count();
                let reached = record.pty.iter().filter(|p| live(p.0)).count();
                if typed != reached {
                    return Err(format!(
                        "healthy run lost input: {reached} of {typed}"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Runs `scenario` from `seed` with some design rules switched off.
///
/// # Errors
/// Returns the first broken invariant.
pub fn run_with(
    scenario: &Scenario,
    seed: u64,
    design: Design,
) -> Result<Report, String> {
    let mut scenario = scenario.clone();
    scenario.profile.design = design;
    run(&scenario, seed)
}

/// Runs `scenario` from `seed` and checks every invariant.
///
/// # Errors
/// Returns the first broken invariant, with enough context to replay it.
pub fn run(scenario: &Scenario, seed: u64) -> Result<Report, String> {
    World::new(scenario, seed)
        .run()
        .map_err(|error| format!("{} seed {seed}: {error}", scenario.name))
}
