//! Running a [`ClientSession`] over a `UnixStream`.
//!
//! A [`Client`] runs one driver thread for its whole life and, per
//! connection, a reader and a writer thread. The reader blocks in `read`
//! and reads only after the driver grants a permit, so at most one read
//! buffer is in flight. The writer takes one batch of messages at a time
//! and writes their frames with vectored writes straight from the encoded
//! messages. The driver owns the session: it waits on one event channel,
//! which the I/O threads and every [`ClientHandle`] feed, or on an
//! `async-io` timer for the session's next deadline, under
//! `futures_lite::future::block_on`.
//!
//! Handles are async and cheap to clone: [`ClientHandle::request`] resolves
//! with the request's result, [`ClientHandle::wait_for`] with a sequence
//! waiter's resolution, [`ClientHandle::hierarchy`] reads the projection
//! and its digest, [`ClientHandle::events`] follows hierarchy changes and
//! connection state, and [`ClientHandle::reconnect`] replaces the
//! connection with one over a new stream. When a connection ends, the
//! driver shuts its socket down and joins both I/O threads before it
//! reports the end. Slice 5 adds `connect`, which resolves a socket path.

use std::collections::HashMap;
use std::io::{self, IoSlice, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use async_channel::{Receiver, Sender};
use huterm_protocol::HierarchyState;
use huterm_wire::error::{ErrorCode, WireError};
use huterm_wire::hierarchy::HierarchySnapshot;
use huterm_wire::request::Request;
use huterm_wire::response::Success;

use crate::config::ClientConfig;
use crate::outbound::{FRAME_HEADER_LEN, Transmit};
use crate::session::{
    CallId, ClientEvent, ClientSession, CloseReason, Connected, Resolution,
    WaitId,
};

/// The most bytes one read takes.
const READ_BUFFER_BYTES: usize = 64 << 10;

/// About how many bytes the driver hands the writer at once.
const WRITE_BATCH_BYTES: usize = 256 << 10;

/// What a handshake ended in.
pub type Handshake = Result<Connected, CloseReason>;

type Reply<T> = Sender<T>;

enum Command {
    Request(Request, Reply<Result<Success, WireError>>),
    Wait(u64, Reply<Resolution>),
    Hierarchy(Reply<Option<HierarchySnapshot>>),
    Events(Reply<Receiver<ClientEvent>>),
    Connected(Reply<Handshake>),
    Reconnect(UnixStream, Reply<Handshake>),
    Shutdown,
}

enum Event {
    Read(u64, Vec<u8>),
    ReadEnded(u64),
    WriterIdle(u64),
    WriteFailed(u64),
    Command(Command),
}

impl std::fmt::Debug for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Read(..) => "Read",
            Self::ReadEnded(_) => "ReadEnded",
            Self::WriterIdle(_) => "WriterIdle",
            Self::WriteFailed(_) => "WriteFailed",
            Self::Command(_) => "Command",
        })
    }
}

#[derive(Debug, Default)]
struct Stats {
    io_threads: AtomicUsize,
}

/// Counts one live I/O thread until it exits.
struct Live(Arc<Stats>);

impl Live {
    fn new(stats: &Arc<Stats>) -> Self {
        stats.io_threads.fetch_add(1, Ordering::AcqRel);
        Self(Arc::clone(stats))
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.0.io_threads.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A running client: its driver thread and what tests observe of it.
/// Dropping it shuts the client down and joins every thread.
#[derive(Debug)]
pub struct Client {
    handle: ClientHandle,
    driver: Option<JoinHandle<()>>,
    stats: Arc<Stats>,
}

/// An async handle to a running [`Client`].
#[derive(Clone, Debug)]
pub struct ClientHandle {
    inbox: Sender<Event>,
}

impl Client {
    /// Starts a client that connects over `stream`.
    ///
    /// # Errors
    /// Returns the error of cloning the stream or spawning a thread.
    pub fn spawn(config: ClientConfig, stream: UnixStream) -> io::Result<Self> {
        let (inbox_sender, inbox) = async_channel::unbounded();
        let stats = Arc::new(Stats::default());
        let io = Io::spawn(stream, 1, &inbox_sender, &stats)?;
        let driver = thread::Builder::new()
            .name("huterm-client-driver".into())
            .spawn({
                let events = inbox_sender.clone();
                let stats = Arc::clone(&stats);
                move || {
                    let driver = Driver {
                        session: ClientSession::new(config),
                        inbox,
                        events,
                        stats,
                        io: Some(io),
                        generation: 1,
                        replies: HashMap::new(),
                        waits: HashMap::new(),
                        handshakes: Vec::new(),
                        listener: None,
                        start: Instant::now(),
                    };
                    futures_lite::future::block_on(driver.run());
                }
            })?;
        Ok(Self {
            handle: ClientHandle {
                inbox: inbox_sender,
            },
            driver: Some(driver),
            stats,
        })
    }

    /// A handle to the client.
    #[must_use]
    pub fn handle(&self) -> ClientHandle {
        self.handle.clone()
    }

    /// The reader and writer threads running now.
    #[must_use]
    pub fn io_threads(&self) -> usize {
        self.stats.io_threads.load(Ordering::Acquire)
    }

    /// Ends the connection, resolves what is outstanding as a disconnect
    /// does, and waits for every thread to exit.
    ///
    /// # Errors
    /// Returns the payload of a thread that panicked.
    pub fn shutdown(mut self) -> thread::Result<()> {
        self.stop()
    }

    fn stop(&mut self) -> thread::Result<()> {
        let _ = self
            .handle
            .inbox
            .try_send(Event::Command(Command::Shutdown));
        match self.driver.take() {
            Some(driver) => driver.join(),
            None => Ok(()),
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl ClientHandle {
    /// Sends `request` and resolves with its result.
    ///
    /// # Errors
    /// Returns the request's failure, from the server or refused locally.
    /// Once the client has shut down, every request fails `Disconnected`.
    pub async fn request(
        &self,
        request: Request,
    ) -> Result<Success, WireError> {
        let (reply, result) = async_channel::bounded(1);
        self.command(Command::Request(request, reply));
        result.recv().await.unwrap_or_else(|_| {
            Err(WireError::new(
                ErrorCode::Disconnected,
                "the client shut down",
            ))
        })
    }

    /// Resolves once the projection reaches hierarchy sequence `seq`, or
    /// the wait is cancelled.
    pub async fn wait_for(&self, seq: u64) -> Resolution {
        let (reply, resolution) = async_channel::bounded(1);
        self.command(Command::Wait(seq, reply));
        resolution.recv().await.unwrap_or(Resolution::Cancelled)
    }

    /// The projection and its digest at the negotiated minor version, once
    /// a snapshot has arrived.
    pub async fn hierarchy(&self) -> Option<HierarchySnapshot> {
        let (reply, snapshot) = async_channel::bounded(1);
        self.command(Command::Hierarchy(reply));
        snapshot.recv().await.ok().flatten()
    }

    /// A receiver of everything the session reports besides results and
    /// waiter resolutions: hierarchy changes and connection state, from
    /// now on. Drain it, as core's subscription; dropping it stops the
    /// forwarding.
    pub async fn events(&self) -> Option<Receiver<ClientEvent>> {
        let (reply, events) = async_channel::bounded(1);
        self.command(Command::Events(reply));
        events.recv().await.ok()
    }

    /// Resolves once the current connection's handshake completes or the
    /// connection ends.
    ///
    /// # Errors
    /// Returns why the connection ended before its handshake completed.
    pub async fn connected(&self) -> Handshake {
        let (reply, handshake) = async_channel::bounded(1);
        self.command(Command::Connected(reply));
        handshake
            .recv()
            .await
            .unwrap_or(Err(CloseReason::Disconnected))
    }

    /// Replaces the connection with one over `stream`, applying the
    /// disconnect rules to the old one, and resolves with the new
    /// handshake's outcome.
    ///
    /// # Errors
    /// Returns why the new connection ended before its handshake
    /// completed.
    pub async fn reconnect(&self, stream: UnixStream) -> Handshake {
        let (reply, handshake) = async_channel::bounded(1);
        self.command(Command::Reconnect(stream, reply));
        handshake
            .recv()
            .await
            .unwrap_or(Err(CloseReason::Disconnected))
    }

    fn command(&self, command: Command) {
        // A closed inbox drops the reply sender with the command, which
        // resolves the caller as disconnected.
        let _ = self.inbox.try_send(Event::Command(command));
    }
}

/// One connection's I/O threads.
struct Io {
    stream: UnixStream,
    generation: u64,
    permits: Sender<()>,
    batches: Sender<Vec<Transmit>>,
    threads: Vec<JoinHandle<()>>,
    reading: bool,
    writer_idle: bool,
}

impl Io {
    fn spawn(
        stream: UnixStream,
        generation: u64,
        events: &Sender<Event>,
        stats: &Arc<Stats>,
    ) -> io::Result<Self> {
        let reader = stream.try_clone()?;
        let writer = stream.try_clone()?;
        let (permits, permit) = async_channel::bounded(1);
        let (batches, batch) = async_channel::bounded(1);
        let mut threads = Vec::with_capacity(2);
        let live = Live::new(stats);
        threads.push(
            thread::Builder::new()
                .name("huterm-client-reader".into())
                .spawn({
                    let events = events.clone();
                    move || {
                        let _live = live;
                        read(reader, generation, &permit, &events);
                    }
                })?,
        );
        let live = Live::new(stats);
        let spawned = thread::Builder::new()
            .name("huterm-client-writer".into())
            .spawn({
                let events = events.clone();
                move || {
                    let _live = live;
                    write(writer, generation, &batch, &events);
                }
            });
        let io = Self {
            stream,
            generation,
            permits,
            batches,
            threads,
            reading: false,
            writer_idle: true,
        };
        match spawned {
            Ok(thread) => {
                let mut io = io;
                io.threads.push(thread);
                Ok(io)
            }
            Err(error) => {
                io.close();
                Err(error)
            }
        }
    }

    /// Shuts the socket down so blocked threads return, closes their
    /// channels, and joins them.
    fn close(self) {
        let _ = self.stream.shutdown(Shutdown::Both);
        drop(self.permits);
        drop(self.batches);
        for thread in self.threads {
            let _ = thread.join();
        }
    }
}

fn read(
    mut stream: UnixStream,
    generation: u64,
    permit: &Receiver<()>,
    events: &Sender<Event>,
) {
    let mut buffer = vec![0; READ_BUFFER_BYTES];
    while permit.recv_blocking().is_ok() {
        let read = loop {
            match stream.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                read => break read,
            }
        };
        let event = match read {
            Ok(0) | Err(_) => Event::ReadEnded(generation),
            Ok(read) => Event::Read(generation, buffer[..read].to_vec()),
        };
        let ended = matches!(event, Event::ReadEnded(_));
        if events.send_blocking(event).is_err() || ended {
            return;
        }
    }
}

fn write(
    mut stream: UnixStream,
    generation: u64,
    batch: &Receiver<Vec<Transmit>>,
    events: &Sender<Event>,
) {
    while let Ok(transmits) = batch.recv_blocking() {
        let event = match write_batch(&mut stream, &transmits) {
            Ok(()) => Event::WriterIdle(generation),
            Err(_) => Event::WriteFailed(generation),
        };
        let failed = matches!(event, Event::WriteFailed(_));
        if events.send_blocking(event).is_err() || failed {
            return;
        }
    }
}

/// Writes every frame of `transmits` with vectored writes from the
/// encoded messages.
fn write_batch(
    stream: &mut UnixStream,
    transmits: &[Transmit],
) -> io::Result<()> {
    let headers: Vec<[u8; FRAME_HEADER_LEN]> = transmits
        .iter()
        .flat_map(|transmit| transmit.frames().map(|(header, _)| header))
        .collect();
    let mut slices = Vec::with_capacity(headers.len() * 2 + 1);
    let mut header = headers.iter();
    for transmit in transmits {
        if let Some(preamble) = transmit.preamble() {
            slices.push(IoSlice::new(preamble));
        }
        for ((_, payload), header) in transmit.frames().zip(&mut header) {
            slices.push(IoSlice::new(header));
            slices.push(IoSlice::new(payload));
        }
    }
    let mut slices = &mut slices[..];
    while !slices.is_empty() {
        match stream.write_vectored(slices) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => IoSlice::advance_slices(&mut slices, written),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

struct Driver {
    session: ClientSession,
    inbox: Receiver<Event>,
    events: Sender<Event>,
    stats: Arc<Stats>,
    io: Option<Io>,
    generation: u64,
    replies: HashMap<CallId, Reply<Result<Success, WireError>>>,
    waits: HashMap<WaitId, Reply<Resolution>>,
    handshakes: Vec<Reply<Handshake>>,
    listener: Option<Sender<ClientEvent>>,
    start: Instant,
}

enum Woke {
    Event(Event),
    Deadline,
}

impl Driver {
    fn now(&self) -> std::time::Duration {
        self.start.elapsed()
    }

    async fn run(mut self) {
        let now = self.now();
        self.session.connect(now);
        loop {
            // A connection the session ended, or that broke, is shut down
            // and its threads joined before its end is reported.
            if !self.session.is_connected()
                && let Some(io) = self.io.take()
            {
                io.close();
            }
            self.route();
            self.transmit();
            if let Some(io) = &mut self.io
                && !io.reading
            {
                io.reading = io.permits.try_send(()).is_ok();
            }
            let deadline = self
                .session
                .next_deadline()
                .map(|deadline| self.start + deadline);
            match self.wait(deadline).await {
                Woke::Deadline => {
                    let now = self.now();
                    self.session.tick(now);
                }
                Woke::Event(Event::Command(Command::Shutdown)) => {
                    self.shutdown();
                    return;
                }
                Woke::Event(event) => self.handle(event),
            }
        }
    }

    fn handle(&mut self, event: Event) {
        let now = self.now();
        let current = self.io.as_ref().map(|io| io.generation);
        match event {
            Event::Read(generation, bytes) if Some(generation) == current => {
                if let Some(io) = &mut self.io {
                    io.reading = false;
                }
                self.session.receive(&bytes, now);
            }
            Event::ReadEnded(generation) | Event::WriteFailed(generation)
                if Some(generation) == current =>
            {
                self.session.disconnected(now);
            }
            Event::WriterIdle(generation) if Some(generation) == current => {
                if let Some(io) = &mut self.io {
                    io.writer_idle = true;
                }
            }
            Event::Command(command) => self.command(command, now),
            // Events of a connection already replaced.
            Event::Read(..)
            | Event::ReadEnded(_)
            | Event::WriteFailed(_)
            | Event::WriterIdle(_) => {}
        }
    }

    fn command(&mut self, command: Command, now: std::time::Duration) {
        match command {
            Command::Request(request, reply) => {
                let call = self.session.request(request, now);
                self.replies.insert(call, reply);
            }
            Command::Wait(seq, reply) => {
                let wait = self.session.wait_for(seq, now);
                self.waits.insert(wait, reply);
            }
            Command::Hierarchy(reply) => {
                let snapshot = self
                    .session
                    .hierarchy()
                    .zip(self.session.hierarchy_digest());
                let _ = reply.try_send(snapshot.map(|(state, digest)| {
                    HierarchySnapshot {
                        digest,
                        state: HierarchyState::clone(state),
                    }
                }));
            }
            Command::Events(reply) => {
                let (sender, receiver) = async_channel::unbounded();
                self.listener = Some(sender);
                let _ = reply.try_send(receiver);
            }
            Command::Connected(reply) => match self.session.connected() {
                Some(connected) => {
                    let _ = reply.try_send(Ok(connected));
                }
                None if self.session.is_connected() => {
                    self.handshakes.push(reply);
                }
                None => {
                    let _ = reply.try_send(Err(CloseReason::Disconnected));
                }
            },
            Command::Reconnect(stream, reply) => {
                self.session.disconnected(now);
                if let Some(io) = self.io.take() {
                    io.close();
                }
                self.generation += 1;
                match Io::spawn(
                    stream,
                    self.generation,
                    &self.events,
                    &self.stats,
                ) {
                    Ok(io) => {
                        self.io = Some(io);
                        self.session.connect(now);
                        self.handshakes.push(reply);
                    }
                    Err(_) => {
                        let _ = reply.try_send(Err(CloseReason::Disconnected));
                    }
                }
            }
            // The run loop handles shutdown before dispatching commands.
            Command::Shutdown => {}
        }
    }

    /// Routes what the session reports: results and waiter resolutions to
    /// their callers, handshake outcomes to whoever awaits them, and the
    /// rest to the event listener.
    fn route(&mut self) {
        while let Some(event) = self.session.poll_event() {
            match event {
                ClientEvent::Result { call, outcome } => {
                    if let Some(reply) = self.replies.remove(&call) {
                        let _ = reply.try_send(outcome);
                    }
                    continue;
                }
                ClientEvent::Waiter { wait, resolution } => {
                    if let Some(reply) = self.waits.remove(&wait) {
                        let _ = reply.try_send(resolution);
                    }
                    continue;
                }
                ClientEvent::Connected(connected) => {
                    for reply in self.handshakes.drain(..) {
                        let _ = reply.try_send(Ok(connected));
                    }
                }
                ClientEvent::Closed(ref reason) => {
                    for reply in self.handshakes.drain(..) {
                        let _ = reply.try_send(Err(reason.clone()));
                    }
                }
                ClientEvent::Hierarchy(_) => {}
            }
            if let Some(listener) = &self.listener
                && listener.try_send(event).is_err()
            {
                self.listener = None;
            }
        }
    }

    /// Hands the idle writer every queued message, up to a batch.
    fn transmit(&mut self) {
        let Some(io) = &mut self.io else {
            return;
        };
        if !io.writer_idle {
            return;
        }
        let mut batch = Vec::new();
        let mut bytes = 0;
        while bytes < WRITE_BATCH_BYTES
            && let Some(transmit) = self.session.poll_transmit()
        {
            bytes += transmit.len();
            batch.push(transmit);
        }
        if !batch.is_empty() {
            io.writer_idle = io.batches.try_send(batch).is_err();
        }
    }

    async fn wait(&self, deadline: Option<Instant>) -> Woke {
        let event = async {
            match self.inbox.recv().await {
                Ok(event) => Woke::Event(event),
                // The driver holds a sender, so the inbox never closes.
                Err(_) => Woke::Event(Event::Command(Command::Shutdown)),
            }
        };
        match deadline {
            None => event.await,
            Some(at) => {
                futures_lite::future::or(event, async {
                    async_io::Timer::at(at).await;
                    Woke::Deadline
                })
                .await
            }
        }
    }

    /// Ends the connection and resolves everything outstanding: what the
    /// disconnect rules leave waiting for a next connection fails
    /// `Disconnected`, as for a dropped session.
    fn shutdown(&mut self) {
        let now = self.now();
        self.session.abandon(now);
        if let Some(io) = self.io.take() {
            io.close();
        }
        self.route();
        // Commands still queued drop their reply senders, so their callers
        // resolve as disconnected instead of waiting for the last handle.
        self.inbox.close();
        while self.inbox.try_recv().is_ok() {}
        for reply in self.handshakes.drain(..) {
            let _ = reply.try_send(Err(CloseReason::Disconnected));
        }
    }
}

#[cfg(test)]
mod tests;
