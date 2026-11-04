//! The world: a deterministic scheduler that runs one server incarnation
//! after another, several clients, and the broker on a virtual clock, then
//! checks the design's invariants.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::broker::{Link, LinkProfile};
use crate::client::{Client, Outcome};
use crate::message::{ClientKind, Request, RetryClass};
use crate::model::{MINOR_0, MINOR_1};
use crate::profile::Profile;
use crate::rng::Rng;
use crate::server::{Record, Server};

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
    /// Chance per thousand fault draws, one every ten ticks, per client, of
    /// a cut.
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
    /// The limits both ends use.
    pub profile: Profile,
    /// Ticks of activity before quiescence.
    pub duration: u64,
    /// Ticks allowed to reach quiescence.
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

/// Every scenario the tests and the program run.
#[must_use]
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
        profile: Profile::default(),
        duration: 3_000,
        quiesce: 20_000,
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
            ..base.clone()
        },
        Scenario {
            name: "remote",
            link: LinkProfile {
                latency: 80,
                jitter: 120,
                chunk: 5,
                buffer: 512,
                corrupt_per_mille: 0,
            },
            cut_per_mille: 4,
            stall_per_mille: 4,
            restart_per_mille: 1,
            toggle_permission: true,
            duration: 12_000,
            ..base.clone()
        },
        Scenario {
            name: "newer-client",
            clients: vec![desktop(MINOR_1), desktop(MINOR_0), desktop(MINOR_1)],
            cut_per_mille: 10,
            ..base.clone()
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
            ..base
        },
    ]
}

/// Totals from one run.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Ticks run, including quiescence.
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
        let old = std::mem::replace(
            &mut self.server,
            Server::new(
                self.incarnation,
                77,
                self.scenario.server_minor,
                Rc::clone(&self.profile),
            ),
        );
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
        // Faults are drawn every tenth tick, so rates are per ten seconds.
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
            if self.clients[client].link().is_none()
                && now >= self.reconnect_at[client]
            {
                self.next_link += 1;
                let id = self.next_link;
                self.links.insert(id, Link::default());
                self.server.accept(id, now);
                self.server.set_client(id, self.clients[client].id);
                self.clients[client].connect(id);
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
                    return Err(format!(
                        "client {} fault at {now}: {error}",
                        self.clients[client].id
                    ));
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
        if self.server.record.max_over_budget
            > (self.profile.max_frame + 4) * self.profile.max_open_streams
        {
            return Err(format!(
                "outbound queue exceeded its budget by {}",
                self.server.record.max_over_budget
            ));
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
                out.push(format!("server conn {id} busy"));
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
        // Corruption can break undigested flow-control state, so recovery is
        // a fresh connection, as a client that saw a fault would make.
        if self.scenario.link.corrupt_per_mille > 0 {
            for client in 0..self.clients.len() {
                self.cut(client);
            }
        }
        for link in self.links.values_mut() {
            link.stalled_until = 0;
            link.trickle_until = 0;
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
        let corrupt = self.scenario.link.corrupt_per_mille > 0;
        let clean = self.scenario.name == "clean";
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
            for tab in client.viewed_tabs() {
                let Some((view, status, stopped)) =
                    self.server.terminal(tab, minor)
                else {
                    continue;
                };
                if stopped {
                    continue;
                }
                if client.view(tab) != Some(&view) {
                    let mine = client
                        .view(tab)
                        .map(|v| (v.revision, v.geometry, v.columns, v.cursor));
                    let theirs = (
                        view.revision,
                        view.geometry,
                        view.columns,
                        view.cursor,
                    );
                    let link = client.link().unwrap_or(0);
                    return Err(format!(
                        "client {id} tab {tab} view diverged: client {mine:?} server {theirs:?}; client {} ; server {}",
                        client.debug_viewer(tab),
                        self.server.debug_viewers(link)
                    ));
                }
                let seen = client
                    .seen
                    .get(&(incarnation, tab))
                    .cloned()
                    .unwrap_or_default();
                // Status carries no digest, so corruption can alter it.
                if !corrupt
                    && (seen.revision != status.revision
                        || seen.first_bells + seen.bells != status.bells
                        || seen.exits != u32::from(status.exited.is_some())
                        || seen.failures
                            != status.failures.last().map_or(0, |f| f.0))
                {
                    return Err(format!(
                        "client {id} tab {tab} status {seen:?} vs {status:?}"
                    ));
                }
            }
            if !corrupt && !client.faults.is_empty() {
                return Err(format!("client {id} faults {:?}", client.faults));
            }
            // With ordering lanes, a gap or digest mismatch needs corruption.
            if !corrupt
                && (client.hierarchy_resyncs > 0 || client.viewer_resyncs > 0)
            {
                return Err(format!(
                    "client {id} asked for {} hierarchy and {} viewer resyncs without corruption",
                    client.hierarchy_resyncs, client.viewer_resyncs
                ));
            }
        }
        let mut records = std::mem::take(&mut self.records);
        records.push((incarnation, std::mem::take(&mut self.server.record)));
        // Request and effect identities carry no digest, so a corrupted ID
        // can still decode. The design relies on the transport for byte
        // integrity; under corruption only digested state is checked.
        let mut executed = BTreeSet::new();
        for (_, record) in &records {
            for key in &record.executed {
                if !executed.insert(*key) && !corrupt {
                    return Err(format!("mutation {key:?} executed twice"));
                }
            }
            if !corrupt {
                let bad: Vec<_> = record
                    .violations
                    .iter()
                    .filter(|(_, why)| why != "writer stalled" && why != "Idle")
                    .collect();
                if !bad.is_empty() {
                    return Err(format!("protocol violations {bad:?}"));
                }
            }
            if clean && !record.violations.is_empty() {
                return Err(format!(
                    "violations in a clean run: {:?}",
                    record.violations
                ));
            }
            self.report.dropped += record.dropped;
            self.report.stalls += record
                .violations
                .iter()
                .filter(|(_, why)| why == "writer stalled")
                .count();
            self.report.inputs += record.pty.len();
        }
        self.report.executed = executed.len();
        for client in self.clients.iter().filter(|_| !corrupt) {
            for (id, request, outcome) in &client.outcomes {
                let ran = executed.contains(&(client.id, *id));
                match outcome {
                    Outcome::Ok
                        if request.class() == RetryClass::Mutation && !ran =>
                    {
                        return Err(format!(
                            "client {} request {id} succeeded but never ran",
                            client.id
                        ));
                    }
                    Outcome::Err(_)
                        if request.class() == RetryClass::Mutation && ran =>
                    {
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
        if clean {
            // Effects for a closed or stopped terminal are legitimately lost.
            let live = |tab: u64| {
                self.server
                    .terminal(tab, MINOR_0)
                    .is_some_and(|(_, _, stopped)| !stopped)
            };
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
                    "clean run lost effects: {done} of {expected} ran"
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
            if clean {
                // Input for a closed or stopped terminal is legitimately lost.
                let live = |tab: u64| {
                    self.server
                        .terminal(tab, MINOR_0)
                        .is_some_and(|(_, _, stopped)| !stopped)
                };
                let typed = self
                    .clients
                    .iter()
                    .flat_map(|c| &c.typed)
                    .filter(|t| live(t.1))
                    .count();
                let reached = record.pty.iter().filter(|p| live(p.0)).count();
                if typed != reached {
                    return Err(format!(
                        "clean run lost input: {reached} of {typed}"
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
    design: crate::profile::Design,
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
