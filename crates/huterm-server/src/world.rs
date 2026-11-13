//! A deterministic world for viewer tests and the seeded fault-injection
//! harness: the real server [`Connection`] and the real client
//! [`ClientSession`] joined by in-memory links on a virtual clock, with
//! model terminals behind the viewers.
//!
//! A link carries bytes in order with latency, jitter, random
//! fragmentation, and a bounded buffer, as the simulation's broker does,
//! and its reader can stall or trickle. Work the connection hands out runs
//! at once, or, when the world holds work, after a random delay and in a
//! random order. Everything draws from one seeded generator, so a run
//! replays from its seed.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use huterm_client::{ClientConfig, ClientEvent, ClientSession, ViewerEvent};
use huterm_core::Mux;
use huterm_protocol::{
    AttachmentId, GridSize, RuntimeId, SessionId, TerminalId, TerminalSnapshot,
    ViewerCapabilities, ViewerId,
};
use huterm_wire::handshake::ClientKind;
use huterm_wire::request::Request;
use huterm_wire::response::{Response, Success};
use huterm_wire::viewer::SubscribeOptions;

use crate::model::{ModelLimits, ModelTerminal, ModelTerminals};
use crate::test_support::offer;
use crate::{Connection, Server, ServerConfig, Work};

/// A snapshot's rows as text, for failure messages.
pub(crate) fn rows_text(snapshot: &TerminalSnapshot) -> Vec<String> {
    snapshot
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| {
                    if cell.text.is_empty() {
                        " "
                    } else {
                        &cell.text
                    }
                })
                .collect::<String>()
        })
        .collect()
}

/// `SplitMix64`, so every run replays from its seed.
#[derive(Clone, Debug)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub(crate) fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }

    pub(crate) fn index(&mut self, n: usize) -> usize {
        usize::try_from(self.below(n as u64)).unwrap_or(0)
    }

    /// True `per_mille` times in a thousand.
    pub(crate) fn chance(&mut self, per_mille: u64) -> bool {
        self.below(1000) < per_mille
    }
}

/// How a link behaves.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LinkProfile {
    /// Base one-way latency, in virtual milliseconds.
    pub(crate) latency: u64,
    /// Extra random latency, up to this many milliseconds.
    pub(crate) jitter: u64,
    /// Largest chunk the link delivers at once.
    pub(crate) chunk: usize,
    /// Bytes a direction buffers before its writer blocks.
    pub(crate) buffer: usize,
}

impl LinkProfile {
    /// A link that delivers everything at once.
    pub(crate) const INSTANT: Self = Self {
        latency: 0,
        jitter: 0,
        chunk: usize::MAX,
        buffer: usize::MAX,
    };
}

/// One direction of a link.
#[derive(Debug, Default)]
pub(crate) struct Pipe {
    chunks: VecDeque<(u64, Vec<u8>)>,
    bytes: usize,
    last_due: u64,
}

impl Pipe {
    pub(crate) fn len(&self) -> usize {
        self.bytes
    }

    fn room(&self, profile: &LinkProfile) -> usize {
        profile.buffer.saturating_sub(self.bytes)
    }

    fn send(
        &mut self,
        profile: &LinkProfile,
        bytes: &[u8],
        now: u64,
        rng: &mut Rng,
    ) {
        let mut rest = bytes;
        while !rest.is_empty() {
            let size = if profile.chunk == usize::MAX {
                rest.len()
            } else {
                (1 + rng.index(profile.chunk)).min(rest.len())
            };
            let due = (now + profile.latency + rng.below(profile.jitter + 1))
                .max(self.last_due);
            self.last_due = due;
            self.chunks.push_back((due, rest[..size].to_vec()));
            self.bytes += size;
            rest = &rest[size..];
        }
    }

    fn deliver(&mut self, now: u64, cap: usize) -> Vec<u8> {
        let mut out = Vec::new();
        while out.len() < cap {
            let Some((due, chunk)) = self.chunks.front_mut() else {
                break;
            };
            if *due > now {
                break;
            }
            let take = (cap - out.len()).min(chunk.len());
            out.extend(chunk.drain(..take));
            self.bytes -= take;
            if chunk.is_empty() {
                self.chunks.pop_front();
            }
        }
        out
    }

    fn clear(&mut self) {
        self.chunks.clear();
        self.bytes = 0;
    }
}

/// Decodable damage injected into deltas on their way to a client: a
/// delta's first full row gets another first character, and the message is
/// framed again on its stream. Transport integrity is assumed, so the
/// damage still decodes; digests must catch it.
///
/// It holds each message's frames until the message completes, so damage
/// is turned off by setting `per_mille` to zero, never by removing it
/// mid-connection, which would drop the frames it holds.
#[derive(Debug)]
pub(crate) struct Tamper {
    /// Chance per thousand that a delta with a full row is damaged.
    pub(crate) per_mille: u64,
    /// Deltas damaged so far.
    pub(crate) damaged: u64,
    preamble: usize,
    decoder: huterm_wire::frame::FrameDecoder,
    partial: std::collections::HashMap<u32, Vec<huterm_wire::frame::Frame>>,
    context: huterm_wire::message::DecodeContext,
}

impl Tamper {
    pub(crate) fn new(per_mille: u64, runtime: RuntimeId) -> Self {
        let mut decoder = huterm_wire::frame::FrameDecoder::new();
        decoder.set_limit(u32::MAX);
        Self {
            per_mille,
            damaged: 0,
            // Created on a live connection, whose preamble already passed;
            // a new connection resets it.
            preamble: 0,
            decoder,
            partial: std::collections::HashMap::new(),
            context: huterm_wire::message::DecodeContext {
                hierarchy: huterm_wire::hierarchy::HierarchyContext {
                    stream: huterm_protocol::StreamId::new(runtime),
                },
                caps: huterm_wire::caps::MessageCaps::for_minor(0),
            },
        }
    }

    /// A new connection's bytes start with a preamble again.
    fn reset(&mut self) {
        self.preamble = huterm_wire::frame::PREAMBLE_LEN;
        self.decoder = huterm_wire::frame::FrameDecoder::new();
        self.decoder.set_limit(u32::MAX);
        self.partial.clear();
    }

    /// The bytes to deliver for `bytes` the server wrote: whole messages
    /// as their frames complete, some of their deltas damaged.
    fn process(&mut self, bytes: &[u8], rng: &mut Rng) -> Vec<u8> {
        use huterm_wire::frame::{Frame, FrameEnd};
        let mut out = Vec::new();
        let mut bytes = bytes;
        if self.preamble > 0 {
            let take = self.preamble.min(bytes.len());
            out.extend_from_slice(&bytes[..take]);
            self.preamble -= take;
            bytes = &bytes[take..];
        }
        self.decoder.push(bytes);
        while let Ok(Some(frame)) = self.decoder.next_frame() {
            let stream = frame.stream;
            let end = frame.end;
            self.partial.entry(stream).or_default().push(frame);
            if end == FrameEnd::More {
                continue;
            }
            let frames = self.partial.remove(&stream).unwrap_or_default();
            let message: Vec<u8> = frames
                .iter()
                .flat_map(|frame| frame.payload.iter().copied())
                .collect();

            if let Some(damaged) = self.damage(&message, rng) {
                out.extend(
                    Frame {
                        stream,
                        end: FrameEnd::Final,
                        payload: damaged,
                    }
                    .encode(u32::MAX)
                    .unwrap_or_default(),
                );
                continue;
            }
            for frame in frames {
                out.extend(frame.encode(u32::MAX).unwrap_or_default());
            }
        }
        out
    }

    fn damage(&mut self, message: &[u8], rng: &mut Rng) -> Option<Vec<u8>> {
        use huterm_wire::message::{Message, decode_message, encode_message};
        use huterm_wire::update::RowOp;
        if self.per_mille == 0 || message.len() > 60 << 10 {
            return None;
        }
        let Ok(Message::TerminalUpdate(mut update)) = decode_message(
            message,
            huterm_wire::caps::Direction::ServerToClient,
            self.context,
        ) else {
            return None;
        };
        update.baseline?;
        let row = update.rows.iter_mut().find_map(|op| match op {
            RowOp::Full(row) => Some(row),
            RowOp::Keep(_) => None,
        })?;
        if !rng.chance(self.per_mille) {
            return None;
        }
        let first = row.texts.first_mut()?;
        *first = if first.as_str() == "#" {
            huterm_protocol::CellText::from('%')
        } else {
            huterm_protocol::CellText::from('#')
        };
        // A text that no longer fits its run's form would not encode;
        // single scalars fit every form.
        let bytes = encode_message(
            &Message::TerminalUpdate(update),
            self.context.hierarchy.scope(),
            self.context.caps,
        )
        .ok()?;
        self.damaged += 1;
        Some(bytes)
    }
}

/// A viewer a peer wants: the terminal it shows, whether it is visible,
/// and its live viewer, if any.
#[derive(Debug)]
pub(crate) struct Wanted {
    pub(crate) terminal: TerminalId,
    pub(crate) visible: bool,
    pub(crate) viewer: Option<ViewerId>,
    pub(crate) subscribing: Option<huterm_client::CallId>,
    /// The terminal stopped, so nothing subscribes to it again.
    pub(crate) stopped: bool,
    /// The viewer asks for host effects, every kind permitted.
    pub(crate) effects: bool,
}

impl Wanted {
    /// The options its subscription asks for.
    fn options(&self) -> SubscribeOptions {
        if self.effects {
            effect_options()
        } else {
            options()
        }
    }
}

/// One client: its session, its connection on the server, and the link
/// between them.
pub(crate) struct Peer {
    pub(crate) session: ClientSession,
    pub(crate) connection: Option<Connection>,
    pub(crate) up: Pipe,
    pub(crate) down: Pipe,
    /// Client bytes waiting for room in the link.
    client_out: VecDeque<u8>,
    /// Work handed out and not yet completed.
    pub(crate) works: Vec<Work>,
    pub(crate) stalled_until: u64,
    pub(crate) trickle_until: u64,
    pub(crate) log: Vec<ClientEvent>,
    pub(crate) attachment: Option<AttachmentId>,
    attach: Option<huterm_client::CallId>,
    pub(crate) wanted: Vec<Wanted>,
    pub(crate) profile: LinkProfile,
    /// Connections the server ended, with why.
    pub(crate) closes: Vec<crate::CloseReason>,
    /// Damage injected into this peer's deltas, if any.
    pub(crate) tamper: Option<Tamper>,
    /// A writer like the native one: it takes a batch of up to this many
    /// bytes and writes at most `write_rate` of it per step. Without one,
    /// the server writes whatever the link has room for at once.
    pub(crate) batch: Option<usize>,
    pub(crate) write_rate: usize,
    pub(crate) writing: Option<(Vec<u8>, usize)>,
    /// Effects the server dropped at a deadline on this peer's ended
    /// connections.
    pub(crate) effects_expired: u64,
}

/// The world.
pub(crate) struct World {
    pub(crate) now: u64,
    pub(crate) rng: Rng,
    pub(crate) server: Arc<Server>,
    pub(crate) session: u64,
    pub(crate) models: ModelTerminals,
    pub(crate) terminals: Vec<ModelTerminal>,
    pub(crate) peers: Vec<Peer>,
    /// Hold handed-out work and run it after random delays, in a random
    /// order.
    pub(crate) hold_work: bool,
    /// Keep handed-out work in each peer's `works` without running it.
    pub(crate) freeze_work: bool,
    size: GridSize,
    limits: ModelLimits,
}

/// Writable viewer options with host effects, every kind permitted.
pub(crate) fn effect_options() -> SubscribeOptions {
    SubscribeOptions {
        capabilities: ViewerCapabilities::ALL,
        allowed_effects: huterm_wire::effect::EffectKinds::ALL,
        ..options()
    }
}

/// Writable viewer options without host effects.
pub(crate) fn options() -> SubscribeOptions {
    SubscribeOptions {
        capabilities: ViewerCapabilities {
            host_effects: false,
            ..ViewerCapabilities::ALL
        },
        focused: false,
        geometry: None,
        presentation: None,
        spawned: false,
        allowed_effects: huterm_wire::effect::EffectKinds::NONE,
    }
}

impl World {
    /// A world on `seed` whose server applies `config`, with `terminals`
    /// model terminals of `size`.
    pub(crate) fn new(
        seed: u64,
        config: ServerConfig,
        terminals: usize,
        size: GridSize,
        limits: ModelLimits,
    ) -> Self {
        let server = Arc::new(Server::new(
            Arc::new(Mutex::new(Mux::default())),
            offer(0),
            config,
        ));
        let (session, runtime) = {
            let mut mux = server.lock();
            (mux.create_session(None).unwrap().get(), mux.runtime_id())
        };
        let models = ModelTerminals::default();
        let next_viewer = Arc::new(std::sync::atomic::AtomicU64::new(1));
        let terminals: Vec<ModelTerminal> = (0..terminals)
            .map(|index| {
                let terminal = ModelTerminal::new(
                    TerminalId::in_runtime(runtime, 10_000 + index as u64),
                    size,
                    limits,
                    Arc::clone(&next_viewer),
                );
                models.insert(terminal.clone());
                terminal
            })
            .collect();
        Self {
            now: 0,
            rng: Rng::new(seed),
            server,
            session,
            models,
            terminals,
            peers: Vec::new(),
            hold_work: false,
            freeze_work: false,
            size,
            limits,
        }
    }

    pub(crate) fn time(&self) -> Duration {
        Duration::from_millis(self.now)
    }

    /// Adds a desktop peer over `profile` that attaches to the world's
    /// session.
    pub(crate) fn add_peer(&mut self, profile: LinkProfile) -> usize {
        self.add_peer_as(profile, ClientKind::Desktop)
    }

    /// Adds a peer of client kind `kind` over `profile`.
    pub(crate) fn add_peer_as(
        &mut self,
        profile: LinkProfile,
        kind: ClientKind,
    ) -> usize {
        let session =
            ClientSession::new(ClientConfig::local(kind, "world", "test"));
        self.peers.push(Peer {
            session,
            connection: None,
            up: Pipe::default(),
            down: Pipe::default(),
            client_out: VecDeque::new(),
            works: Vec::new(),
            stalled_until: 0,
            trickle_until: 0,
            log: Vec::new(),
            attachment: None,
            attach: None,
            wanted: Vec::new(),
            profile,
            closes: Vec::new(),
            tamper: None,
            batch: None,
            write_rate: usize::MAX,
            writing: None,
            effects_expired: 0,
        });
        let index = self.peers.len() - 1;
        self.connect(index);
        index
    }

    /// Opens a new connection for `peer`.
    pub(crate) fn connect(&mut self, peer: usize) {
        let now = self.time();
        let connection = self.server.connection(now);
        let peer = &mut self.peers[peer];
        peer.up.clear();
        peer.down.clear();
        peer.client_out.clear();
        peer.writing = None;
        peer.connection = Some(connection);
        peer.attachment = None;
        peer.attach = None;
        if let Some(tamper) = peer.tamper.as_mut() {
            tamper.reset();
        }
        for wanted in &mut peer.wanted {
            wanted.viewer = None;
            wanted.subscribing = None;
        }
        peer.session.connect(now);
    }

    /// Breaks `peer`'s link: both ends see the connection end.
    pub(crate) fn cut(&mut self, peer: usize) {
        self.hang_up(peer);
        let now = self.time();
        let peer = &mut self.peers[peer];
        peer.session.disconnected(now);
        // Viewers end with their connection.
        for wanted in &mut peer.wanted {
            wanted.viewer = None;
            wanted.subscribing = None;
        }
    }

    /// Ends the server's side of `peer`'s connection and runs its cleanup.
    fn hang_up(&mut self, peer: usize) {
        let now = self.time();
        let index = peer;
        let peer = &mut self.peers[index];
        if let Some(mut connection) = peer.connection.take() {
            peer.effects_expired += connection.effect_stats().expired;
            peer.works.extend(connection.close(now));
            // Cleanup and late completions still run against the server.
            let works = std::mem::take(&mut peer.works);
            for work in works {
                let completion = self.models.execute(&self.server, work.job);
                connection.complete(work.id, completion, now);
            }
            while let Some(work) = connection.poll_work() {
                let completion = self.models.execute(&self.server, work.job);
                connection.complete(work.id, completion, now);
            }
        }
    }

    /// Wants a viewer of terminal `terminal` on `peer`.
    pub(crate) fn want(&mut self, peer: usize, terminal: usize, visible: bool) {
        self.want_viewer(peer, terminal, visible, false);
    }

    /// Wants a viewer of terminal `terminal` on `peer` that receives host
    /// effects.
    pub(crate) fn want_effects(
        &mut self,
        peer: usize,
        terminal: usize,
        visible: bool,
    ) {
        self.want_viewer(peer, terminal, visible, true);
    }

    fn want_viewer(
        &mut self,
        peer: usize,
        terminal: usize,
        visible: bool,
        effects: bool,
    ) {
        let terminal = self.client_terminal(peer, terminal);
        self.peers[peer].wanted.push(Wanted {
            terminal,
            visible,
            viewer: None,
            subscribing: None,
            stopped: false,
            effects,
        });
    }

    /// The host effects `peer` received, in order.
    pub(crate) fn effects(
        &self,
        peer: usize,
    ) -> Vec<&huterm_client::EffectDelivery> {
        self.viewer_events(peer, 0)
            .into_iter()
            .filter_map(|event| match event {
                ViewerEvent::HostEffect(delivery) => Some(delivery),
                _ => None,
            })
            .collect()
    }

    /// The model terminal `index`'s identity in `peer`'s scope, once
    /// connected.
    pub(crate) fn client_terminal(
        &self,
        peer: usize,
        index: usize,
    ) -> TerminalId {
        let runtime = self.peers[peer]
            .session
            .stream()
            .map_or(RuntimeId::new(0), huterm_protocol::StreamId::runtime);
        TerminalId::in_runtime(runtime, self.terminals[index].id().get())
    }

    /// Advances one virtual millisecond.
    pub(crate) fn step(&mut self) {
        self.now += 1;
        let signalled: Vec<(ViewerId, bool)> = self
            .terminals
            .iter()
            .flat_map(|terminal| {
                let stopped = terminal.is_stopped();
                terminal
                    .take_signalled()
                    .into_iter()
                    .map(move |viewer| (viewer, stopped))
            })
            .collect();
        for index in 0..self.peers.len() {
            self.step_peer(index, &signalled);
        }
    }

    /// Steps until nothing moves for `quiet` consecutive steps, or
    /// `limit` steps pass.
    pub(crate) fn settle(&mut self, limit: u64) {
        let mut quiet = 0;
        for _ in 0..limit {
            let before = self.activity();
            self.step();
            if self.activity() == before && self.idle() {
                quiet += 1;
                if quiet > 50 {
                    return;
                }
            } else {
                quiet = 0;
            }
        }
    }

    fn activity(&self) -> (usize, usize) {
        let logged = self.peers.iter().map(|peer| peer.log.len()).sum();
        let bytes = self
            .peers
            .iter()
            .map(|peer| peer.up.len() + peer.down.len())
            .sum();
        (logged, bytes)
    }

    fn idle(&self) -> bool {
        self.peers.iter().all(|peer| {
            peer.up.len() == 0
                && peer.down.len() == 0
                && peer.client_out.is_empty()
                && peer.works.is_empty()
                && peer.writing.is_none()
        })
    }

    #[expect(clippy::too_many_lines, reason = "one pass of the world")]
    fn step_peer(&mut self, index: usize, signalled: &[(ViewerId, bool)]) {
        let now = self.time();
        let ms = self.now;
        let hold = self.hold_work;
        // The client writes into its link.
        {
            let peer = &mut self.peers[index];
            while let Some(transmit) = peer.session.poll_transmit() {
                peer.client_out.extend(transmit.to_bytes());
            }
            let room = peer.up.room(&peer.profile).min(peer.client_out.len());
            if room > 0 {
                let bytes: Vec<u8> = peer.client_out.drain(..room).collect();
                peer.up.send(&peer.profile, &bytes, ms, &mut self.rng);
            }
        }
        // The server reads.
        {
            let peer = &mut self.peers[index];
            if let Some(connection) = peer.connection.as_mut()
                && connection.wants_input()
            {
                let bytes = peer.up.deliver(ms, 64 << 10);
                if !bytes.is_empty() {
                    connection.receive(&bytes, now);
                }
            }
        }
        // Work runs.
        {
            let peer = &mut self.peers[index];
            if let Some(connection) = peer.connection.as_mut() {
                while let Some(work) = connection.poll_work() {
                    peer.works.push(work);
                }
            }
            let mut ready = Vec::new();
            if self.freeze_work {
                // Nothing runs until a test thaws it.
            } else if hold {
                let mut kept = Vec::new();
                for work in peer.works.drain(..) {
                    if self.rng.chance(300) {
                        ready.push(work);
                    } else {
                        kept.push(work);
                    }
                }
                peer.works = kept;
                // Any order.
                for at in (1..ready.len()).rev() {
                    let other = self.rng.index(at + 1);
                    ready.swap(at, other);
                }
            } else {
                ready = std::mem::take(&mut peer.works);
            }
            for work in ready {
                let completion = self.models.execute(&self.server, work.job);
                let peer = &mut self.peers[index];
                if let Some(connection) = peer.connection.as_mut() {
                    connection.complete(work.id, completion, now);
                }
            }
        }
        // Viewer wakes and ends.
        {
            let peer = &mut self.peers[index];
            if let Some(connection) = peer.connection.as_mut() {
                for (viewer, stopped) in signalled {
                    connection.viewer_woke(*viewer, now);
                    if *stopped {
                        connection.viewer_ended(*viewer, now);
                    }
                }
                while connection.poll_watch().is_some() {}
                while connection.poll_unwatch().is_some() {}
            }
        }
        // The server writes into its link.
        {
            let peer = &mut self.peers[index];
            if let (Some(connection), Some(batch)) =
                (peer.connection.as_mut(), peer.batch)
            {
                if peer.writing.is_none()
                    && let Some(transmit) = connection.poll_transmit(batch)
                {
                    peer.writing = Some((transmit.to_bytes(), 0));
                }
                if let Some((bytes, written)) = peer.writing.as_mut() {
                    let count = (bytes.len() - *written)
                        .min(peer.write_rate)
                        .min(peer.down.room(&peer.profile));
                    if count > 0 {
                        let chunk = bytes[*written..*written + count].to_vec();
                        *written += count;
                        connection.transmitted(count, now);
                        let chunk = match peer.tamper.as_mut() {
                            Some(tamper) => {
                                tamper.process(&chunk, &mut self.rng)
                            }
                            None => chunk,
                        };
                        peer.down.send(
                            &peer.profile,
                            &chunk,
                            ms,
                            &mut self.rng,
                        );
                    }
                    if *written == bytes.len() {
                        peer.writing = None;
                    }
                }
            } else if let Some(connection) = peer.connection.as_mut() {
                loop {
                    let room = peer.down.room(&peer.profile);
                    if room == 0 {
                        break;
                    }
                    let Some(transmit) = connection.poll_transmit(room) else {
                        break;
                    };
                    let bytes = transmit.to_bytes();
                    connection.transmitted(bytes.len(), now);
                    let bytes = match peer.tamper.as_mut() {
                        Some(tamper) => tamper.process(&bytes, &mut self.rng),
                        None => bytes,
                    };
                    peer.down.send(&peer.profile, &bytes, ms, &mut self.rng);
                }
            }
        }
        // The client reads.
        {
            let peer = &mut self.peers[index];
            if peer.stalled_until <= ms {
                let cap = if peer.trickle_until > ms { 1 } else { 64 << 10 };
                let bytes = peer.down.deliver(ms, cap);
                if !bytes.is_empty() {
                    peer.session.receive(&bytes, now);
                }
            }
        }
        // Deadlines.
        {
            let peer = &mut self.peers[index];
            if let Some(connection) = peer.connection.as_mut()
                && connection.next_deadline().is_some_and(|at| at <= now)
            {
                connection.tick(now);
            }
            // A stalled reader is a frozen client: its clock stops too.
            if peer.stalled_until <= ms
                && peer.session.next_deadline().is_some_and(|at| at <= now)
            {
                peer.session.tick(now);
            }
        }
        // A connection the server ended: the client sees its end.
        let ended = self.peers[index]
            .connection
            .as_ref()
            .and_then(|connection| connection.close_reason().cloned());
        if let Some(reason) = ended {
            self.peers[index].closes.push(reason);
            self.cut(index);
        }
        // A connection the client ended, at a deadline or a violation: the
        // server sees the transport close.
        if self.peers[index].connection.is_some()
            && !self.peers[index].session.is_connected()
        {
            self.hang_up(index);
        }
        self.drive_client(index);
    }

    /// Plays the client application: attaches, subscribes what it wants,
    /// and keeps its visibility.
    fn drive_client(&mut self, index: usize) {
        let now = self.time();
        let session = self.session;
        let peer = &mut self.peers[index];
        while let Some(event) = peer.session.poll_event() {
            match &event {
                ClientEvent::Result { call, outcome } => {
                    if Some(*call) == peer.attach {
                        peer.attach = None;
                        if let Ok(Success {
                            response: Response::Attachment(attachment),
                            ..
                        }) = outcome
                        {
                            peer.attachment = Some(*attachment);
                        }
                    }
                    for wanted in &mut peer.wanted {
                        if wanted.subscribing == Some(*call) {
                            wanted.subscribing = None;
                            match outcome {
                                Ok(Success {
                                    response: Response::Viewer { viewer, .. },
                                    ..
                                }) => wanted.viewer = Some(*viewer),
                                Ok(Success {
                                    response: Response::Stopped { .. },
                                    ..
                                }) => wanted.stopped = true,
                                _ => {}
                            }
                        }
                    }
                }
                ClientEvent::Viewer(ViewerEvent::Stopped {
                    viewer, ..
                }) => {
                    for wanted in &mut peer.wanted {
                        if wanted.viewer == Some(*viewer) {
                            wanted.viewer = None;
                            wanted.stopped = true;
                        }
                    }
                }
                ClientEvent::Viewer(
                    ViewerEvent::Revoked { viewer, .. }
                    | ViewerEvent::Fault { viewer },
                ) => {
                    for wanted in &mut peer.wanted {
                        if wanted.viewer == Some(*viewer) {
                            wanted.viewer = None;
                        }
                    }
                }
                _ => {}
            }
            peer.log.push(event);
        }
        let Some(connected) = peer.session.connected() else {
            return;
        };
        if peer.attachment.is_none() && peer.attach.is_none() {
            peer.attach = Some(peer.session.request(
                Request::AttachSession {
                    session: SessionId::in_runtime(connected.runtime, session),
                },
                now,
            ));
            return;
        }
        let Some(attachment) = peer.attachment else {
            return;
        };
        for wanted in &mut peer.wanted {
            // Identities follow the connection's runtime.
            wanted.terminal = TerminalId::in_runtime(
                connected.runtime,
                wanted.terminal.get(),
            );
            if wanted.viewer.is_none()
                && wanted.subscribing.is_none()
                && !wanted.stopped
            {
                wanted.subscribing = Some(peer.session.subscribe_terminal(
                    attachment,
                    wanted.terminal,
                    wanted.options(),
                    wanted.visible,
                    now,
                ));
            }
        }
        let visibility: Vec<(ViewerId, bool)> = peer
            .wanted
            .iter()
            .filter_map(|wanted| {
                wanted.viewer.map(|viewer| (viewer, wanted.visible))
            })
            .collect();
        for (viewer, visible) in visibility {
            peer.session.set_visible(viewer, visible);
        }
    }

    /// Damages deltas on their way to `peer`, `per_mille` of those with a
    /// full row.
    pub(crate) fn tamper(&mut self, peer: usize, per_mille: u64) {
        let runtime = self.server.runtime();
        self.peers[peer].tamper = Some(Tamper::new(per_mille, runtime));
    }

    /// Replaces the server with a new incarnation over the same model
    /// terminals, as a restart that lost the terminals would; every peer's
    /// connection ends.
    pub(crate) fn restart_server(&mut self) {
        for index in 0..self.peers.len() {
            self.cut(index);
        }
        let mut offer = offer(0);
        offer.incarnation = huterm_protocol::ServerIncarnation::from_bytes(
            [u8::try_from(self.now % 256).unwrap_or(0) ^ 0x5a; 16],
        );
        let config = self.server.config();
        let server = Arc::new(Server::new(
            Arc::new(Mutex::new(Mux::default())),
            offer,
            config,
        ));
        let (session, runtime) = {
            let mut mux = server.lock();
            (mux.create_session(None).unwrap().get(), mux.runtime_id())
        };
        // A new incarnation's terminals are new.
        self.models = ModelTerminals::default();
        let next_viewer = Arc::new(std::sync::atomic::AtomicU64::new(1));
        self.terminals = (0..self.terminals.len())
            .map(|index| {
                let terminal = ModelTerminal::new(
                    TerminalId::in_runtime(runtime, 10_000 + index as u64),
                    self.size,
                    self.limits,
                    Arc::clone(&next_viewer),
                );
                self.models.insert(terminal.clone());
                terminal
            })
            .collect();
        self.server = server;
        self.session = session;
    }

    /// Revokes `peer`'s viewer of terminal `index` in its terminal, as a
    /// detach elsewhere would; the peer subscribes again once it hears.
    pub(crate) fn revoke(&mut self, peer: usize, index: usize) -> bool {
        let Some(viewer) = self.viewer(peer, index) else {
            return false;
        };
        let terminal = &self.terminals[index];
        terminal.revoke(ViewerId::in_runtime(
            terminal.id().runtime(),
            viewer.get(),
        ));
        true
    }

    /// Closes `peer`'s viewer of terminal `index` from the client, which
    /// subscribes a new one later.
    pub(crate) fn close_viewer(&mut self, peer: usize, index: usize) -> bool {
        let Some(viewer) = self.viewer(peer, index) else {
            return false;
        };
        let now = self.time();
        let id = self.terminals[index].id().get();
        let peer = &mut self.peers[peer];
        peer.session.request(Request::CloseViewer { viewer }, now);
        for wanted in &mut peer.wanted {
            if wanted.terminal.get() == id {
                wanted.viewer = None;
            }
        }
        true
    }

    /// Types `text` into `peer`'s viewer of terminal `index`, returning
    /// its input sequence, or `None` without a live viewer.
    pub(crate) fn type_text(
        &mut self,
        peer: usize,
        index: usize,
        text: &str,
    ) -> Option<u64> {
        let viewer = self.viewer(peer, index)?;
        self.peers[peer]
            .session
            .send_input(
                viewer,
                huterm_protocol::TerminalInput::Text(text.into()),
                None,
            )
            .ok()
    }

    /// The view `peer`'s viewer of terminal `index` shows, if it has one.
    pub(crate) fn view(
        &self,
        peer: usize,
        index: usize,
    ) -> Option<TerminalSnapshot> {
        let id = self.terminals[index].id().get();
        let peer = &self.peers[peer];
        let wanted = peer.wanted.iter().find(|wanted| {
            wanted.terminal.get() == id && wanted.viewer.is_some()
        })?;
        peer.session.view(wanted.viewer?)
    }

    /// `peer`'s live viewer of terminal `index`.
    pub(crate) fn viewer(&self, peer: usize, index: usize) -> Option<ViewerId> {
        let id = self.terminals[index].id().get();
        self.peers[peer]
            .wanted
            .iter()
            .find(|wanted| wanted.terminal.get() == id)?
            .viewer
    }

    /// Whether `peer`'s view of terminal `index` shows the model's current
    /// snapshot: the same rows and metadata.
    pub(crate) fn shows_current(&self, peer: usize, index: usize) -> bool {
        let Some(mut view) = self.view(peer, index) else {
            return false;
        };
        let current = self.terminals[index].snapshot();
        view.terminal_id = current.terminal_id;
        view == *current
    }

    /// Viewer events `peer` logged, from `since`.
    pub(crate) fn viewer_events(
        &self,
        peer: usize,
        since: usize,
    ) -> Vec<&ViewerEvent> {
        self.peers[peer].log[since..]
            .iter()
            .filter_map(|event| match event {
                ClientEvent::Viewer(event) => Some(event),
                _ => None,
            })
            .collect()
    }
}
