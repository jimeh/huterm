//! Raw wire helpers that play the client in server tests: building the
//! preamble, `Hello`, requests, and cancels as frames, and decoding what a
//! server sends. They follow no retry or reconnect rules; `huterm-client`
//! owns those.
//!
//! The peer decodes identities in the server's own runtime scope, so IDs
//! it reads name the server's `Mux` records directly.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use huterm_core::Mux;
use huterm_protocol::{
    CellSize, GridSize, RuntimeId, ScrollbackLimit, ServerIncarnation,
    ShellIntegration, StreamId, TerminalCommand, TerminalPresentation,
};
use huterm_wire::MINOR;
use huterm_wire::caps::{Direction, MessageCaps, MessageKind};
use huterm_wire::codec::{IdScope, Reader};
use huterm_wire::frame::{
    DEFAULT_MAX_FRAME, Frame, FrameDecoder, FrameEnd, HANDSHAKE_FRAME_LIMIT,
    PREAMBLE_LEN, check_preamble, preamble,
};
use huterm_wire::handshake::{
    ClientKind, Hello, ServerId, ServerOffer, Welcome, decode_welcome,
};
use huterm_wire::hierarchy::HierarchyContext;
use huterm_wire::message::{
    DecodeContext, Message, decode_message, encode_message,
};
use huterm_wire::request::{Cancel, Request, RequestId, RequestMessage};
use huterm_wire::stream::Reassembler;

use crate::config::ServerConfig;
use crate::connection::Connection;
use crate::runtime::execute;
use crate::server::Server;

/// A listener for a server on a socket in its own private temporary
/// directory, so parallel tests never share a socket.
#[cfg(unix)]
#[derive(Debug)]
pub struct SocketServer {
    // Dropped first: the listener stops and removes its socket before the
    // directory goes.
    listener: crate::listener::Listener,
    socket: huterm_socket::SocketPath,
    _directory: huterm_socket::testing::TestDirectory,
}

#[cfg(unix)]
impl SocketServer {
    /// Listens for `server`'s clients on a fresh socket.
    ///
    /// # Panics
    /// Panics when the socket cannot be bound.
    #[must_use]
    pub fn start(server: Arc<Server>) -> Self {
        let directory = huterm_socket::testing::TestDirectory::new();
        let socket = directory.socket("test");
        let listener = crate::listener::Listener::bind(server, &socket)
            .expect("the test socket binds");
        Self {
            listener,
            socket,
            _directory: directory,
        }
    }

    /// The socket clients connect to.
    #[must_use]
    pub fn socket(&self) -> &huterm_socket::SocketPath {
        &self.socket
    }

    /// The listener.
    #[must_use]
    pub fn listener(&self) -> &crate::listener::Listener {
        &self.listener
    }

    /// Stops the listener, closing every connection and waiting for their
    /// threads.
    ///
    /// # Errors
    /// Returns the payload of a thread that panicked.
    pub fn shutdown(self) -> std::thread::Result<()> {
        self.listener.shutdown()
    }
}

/// What a fixed test server offers at `minor`.
#[must_use]
pub fn offer(minor: u16) -> ServerOffer {
    ServerOffer {
        minor,
        features: std::collections::BTreeSet::new(),
        max_frame: DEFAULT_MAX_FRAME,
        incarnation: ServerIncarnation::from_bytes([7; 16]),
        server_id: ServerId([9; 16]),
        host_name: "test-host".into(),
        socket_name: "test".into(),
        build_version: "test".into(),
        process_id: std::process::id(),
    }
}

/// A server over a fresh `Mux` at this build's minor version.
#[must_use]
pub fn server(config: ServerConfig) -> Server {
    Server::new(Arc::new(Mutex::new(Mux::default())), offer(MINOR), config)
}

/// A command for a shell that waits quietly for input.
#[must_use]
pub fn quiet_command() -> TerminalCommand {
    command("read value")
}

/// A command for `/bin/sh -c script`.
#[must_use]
pub fn command(script: &str) -> TerminalCommand {
    TerminalCommand {
        program: "/bin/sh".into(),
        arguments: vec!["-c".into(), script.into()],
        working_directory: std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/")),
        environment: Vec::new(),
        grid_size: GridSize::clamped(40, 8),
        cell_size: CellSize {
            width: 8,
            height: 16,
        },
        presentation: TerminalPresentation::default(),
        scrollback: ScrollbackLimit::DEFAULT,
        shell_integration: ShellIntegration::OFF,
        scrollback_compression: false,
    }
}

/// Splits a whole message into frames on `stream` under `limit`.
///
/// # Panics
/// Panics when `limit` leaves no room for a frame on `stream`.
#[must_use]
pub fn frames(message: &[u8], stream: u32, limit: u32) -> Vec<u8> {
    let room = Frame::capacity(stream, limit).max(1);
    let mut bytes = Vec::new();
    let mut chunks = message.chunks(room).peekable();
    while let Some(chunk) = chunks.next() {
        let frame = Frame {
            stream,
            end: if chunks.peek().is_some() {
                FrameEnd::More
            } else {
                FrameEnd::Final
            },
            payload: chunk.to_vec(),
        };
        bytes.extend(frame.encode(limit).expect("a frame within its limit"));
    }
    bytes
}

/// Runs every job `connection` hands out against `server`, completing each
/// at `now`, and returns how many ran.
pub fn run(
    server: &Server,
    connection: &mut Connection,
    now: Duration,
) -> usize {
    let mut ran = 0;
    while let Some(work) = connection.poll_work() {
        let completion = execute(server, work.job);
        connection.complete(work.id, completion, now);
        ran += 1;
    }
    ran
}

/// Takes every frame `connection` has queued, reporting each written in
/// full at `now`.
pub fn flush(connection: &mut Connection, now: Duration) -> Vec<u8> {
    let mut bytes = Vec::new();
    while let Some(transmit) = connection.poll_transmit(usize::MAX) {
        bytes.extend(transmit.to_bytes());
        connection.transmitted(transmit.len(), now);
    }
    bytes
}

/// A raw client: frames what it sends, and reassembles and decodes what a
/// server sends back.
#[derive(Debug)]
pub struct TestPeer {
    hello: Hello,
    scope: IdScope,
    stream: StreamId,
    caps: MessageCaps,
    limit: u32,
    decoder: FrameDecoder,
    reassembler: Reassembler,
    preamble: Vec<u8>,
    last: u64,
    welcome: Option<Welcome>,
}

impl TestPeer {
    /// A desktop client at this build's minor version, decoding in
    /// `runtime`'s scope.
    #[must_use]
    pub fn new(runtime: RuntimeId) -> Self {
        Self::with_hello(
            runtime,
            Hello::new(ClientKind::Desktop, "test", "test"),
        )
    }

    /// A client that sends `hello`, decoding in `runtime`'s scope.
    #[must_use]
    pub fn with_hello(runtime: RuntimeId, hello: Hello) -> Self {
        let caps = MessageCaps::for_minor(0);
        Self {
            hello,
            scope: IdScope::new(runtime),
            stream: StreamId::new(runtime),
            caps,
            limit: HANDSHAKE_FRAME_LIMIT,
            decoder: FrameDecoder::new(),
            reassembler: Reassembler::new(
                caps,
                Direction::ServerToClient,
                usize::MAX,
            ),
            preamble: Vec::new(),
            last: 0,
            welcome: None,
        }
    }

    /// The server's `Welcome`, once received.
    #[must_use]
    pub fn welcome(&self) -> Option<&Welcome> {
        self.welcome.as_ref()
    }

    /// The caps in force: minor version 0's until `Welcome` arrives.
    #[must_use]
    pub fn caps(&self) -> MessageCaps {
        self.caps
    }

    /// The hierarchy stream decoded states and events belong to.
    #[must_use]
    pub fn stream(&self) -> StreamId {
        self.stream
    }

    /// The preamble and `Hello`, framed.
    #[must_use]
    pub fn opening(&self) -> Vec<u8> {
        let mut bytes = preamble().to_vec();
        bytes.extend(self.message(&Message::Hello(self.hello.clone())));
        bytes
    }

    /// `message` encoded and framed on stream 0.
    ///
    /// # Panics
    /// Panics when the message does not encode.
    #[must_use]
    pub fn message(&self, message: &Message) -> Vec<u8> {
        let encoded = encode_message(message, self.scope, self.caps)
            .expect("the test message encodes");
        frames(&encoded, 0, self.limit)
    }

    /// The next request ID.
    ///
    /// # Panics
    /// Panics once request IDs run out.
    pub fn next_id(&mut self) -> RequestId {
        self.last += 1;
        RequestId::new(self.last).expect("request IDs start at 1")
    }

    /// `request` under the next request ID, framed.
    pub fn request(&mut self, request: Request) -> (RequestId, Vec<u8>) {
        let id = self.next_id();
        (id, self.request_as(id, request))
    }

    /// `request` under `id`, framed.
    #[must_use]
    pub fn request_as(&self, id: RequestId, request: Request) -> Vec<u8> {
        self.message(&Message::Request(RequestMessage { id, request }))
    }

    /// A cancel of `request`, framed.
    #[must_use]
    pub fn cancel(&self, request: RequestId) -> Vec<u8> {
        self.message(&Message::Cancel(Cancel { request }))
    }

    /// Every whole message the server's `bytes` complete, undecoded.
    ///
    /// # Panics
    /// Panics on a framing or stream violation.
    pub fn receive_raw(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut bytes = bytes;
        if self.preamble.len() < PREAMBLE_LEN {
            let take = (PREAMBLE_LEN - self.preamble.len()).min(bytes.len());
            self.preamble.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.preamble.len() == PREAMBLE_LEN {
                let mut peer = [0; PREAMBLE_LEN];
                peer.copy_from_slice(&self.preamble);
                check_preamble(peer).expect("the server's preamble");
            }
        }
        self.decoder.push(bytes);
        let mut messages = Vec::new();
        while let Some(frame) = self.decoder.next_frame().expect("a frame") {
            let Some(assembled) = self
                .reassembler
                .accept(frame, Duration::ZERO)
                .expect("a stream")
            else {
                continue;
            };
            if assembled.kind == MessageKind::Welcome {
                let mut reader = Reader::new(&assembled.bytes);
                reader.u64().expect("a kind");
                let welcome = decode_welcome(&mut reader).expect("a Welcome");
                self.caps = MessageCaps::for_minor(welcome.minor);
                self.limit = welcome.max_frame;
                self.decoder.set_limit(welcome.max_frame);
                self.reassembler.set_caps(self.caps, usize::MAX);
                self.welcome = Some(welcome);
            }
            messages.push(assembled.bytes);
        }
        messages
    }

    /// Every message the server's `bytes` complete, decoded.
    ///
    /// # Panics
    /// Panics on a violation or a message that does not decode.
    pub fn receive(&mut self, bytes: &[u8]) -> Vec<Message> {
        self.receive_raw(bytes)
            .into_iter()
            .map(|message| self.decode(&message))
            .collect()
    }

    /// Decodes one whole message from the server.
    ///
    /// # Panics
    /// Panics when it does not decode.
    #[must_use]
    pub fn decode(&self, message: &[u8]) -> Message {
        decode_message(
            message,
            Direction::ServerToClient,
            DecodeContext {
                hierarchy: HierarchyContext {
                    stream: self.stream,
                },
                caps: self.caps,
            },
        )
        .expect("the server's message decodes")
    }
}
