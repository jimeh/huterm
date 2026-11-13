//! Serving one [`Connection`] over a `UnixStream`.
//!
//! Each connection runs three threads. The reader blocks in `read` and
//! reads only after the driver grants a permit, which it grants while
//! [`Connection::wants_input`] holds, so at most one read buffer is in
//! flight and a client that floods without reading meets socket
//! backpressure. The writer writes one batch of frames at a time with
//! vectored writes straight from the messages' shared buffers, reporting
//! every write's progress. The driver runs the connection on an
//! `async-executor` `LocalExecutor`: it waits on one event channel that the
//! reader, the writer, worker completions, and the hierarchy feed's
//! forwarding task feed, or on an `async-io` timer for the connection's next
//! deadline. The socket is non-blocking: while the writer is idle, the
//! driver first writes a batch itself and hands the writer only what the
//! socket did not take, so a reply the socket accepts at once skips a
//! thread, and the reader and writer wait with `poll` where a blocking
//! socket would block. Jobs run on `blocking`'s worker pool, never on these threads,
//! and the server never holds the `Mux` lock while it waits on the socket.
//!
//! When the connection ends, the driver shuts the socket down so blocked
//! threads return, keeps running until every job it handed out completes
//! and its cleanup ran, and then exits; [`ConnectionHandle::join`] joins
//! all three threads.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use async_channel::{Receiver, Sender};
use async_executor::{LocalExecutor, Task};
use huterm_protocol::ViewerId;
use huterm_wire::handshake::{Reject, RejectReason};

use crate::connection::{Connection, Transmit};
use crate::feed::HierarchyFeed;
use crate::listener::Slot;
use crate::runtime;
use crate::server::Server;
use crate::viewer::WakeSource;
use crate::work::{Completion, Job, Work, WorkId};

/// The most bytes one read takes.
const READ_BUFFER_BYTES: usize = 64 << 10;

/// About how many bytes the driver hands the writer at once.
const WRITE_BATCH_BYTES: usize = 256 << 10;

#[derive(Debug)]
enum Event {
    Read(Vec<u8>),
    ReadEnded,
    Written(usize),
    WriterIdle,
    WriteFailed,
    Completed(WorkId, Box<Completion>),
    Hierarchy,
    Viewer(ViewerId),
    ViewerEnded(ViewerId),
    Close,
}

#[derive(Debug, Default)]
struct Stats {
    received: AtomicUsize,
    written: AtomicUsize,
    short_writes: AtomicUsize,
    retained: AtomicUsize,
    max_retained: AtomicUsize,
    bound: AtomicUsize,
    paused: AtomicBool,
    closed: AtomicBool,
}

/// A served connection's threads and what tests and a listener observe of
/// it.
#[derive(Debug)]
pub struct ConnectionHandle {
    threads: Vec<JoinHandle<()>>,
    events: Sender<Event>,
    stats: Arc<Stats>,
}

impl ConnectionHandle {
    /// Asks the driver to end the connection.
    pub fn close(&self) {
        let _ = self.events.try_send(Event::Close);
    }

    /// The bytes the connection has read and taken in.
    #[must_use]
    pub fn received(&self) -> usize {
        self.stats.received.load(Ordering::Acquire)
    }

    /// The bytes the socket has accepted from the connection.
    #[must_use]
    pub fn written(&self) -> usize {
        self.stats.written.load(Ordering::Acquire)
    }

    /// How many times the socket took less than the driver offered, so
    /// the writer thread finished the batch once the socket had room.
    #[must_use]
    pub fn short_writes(&self) -> usize {
        self.stats.short_writes.load(Ordering::Acquire)
    }

    /// The encoded bytes the connection retains now.
    #[must_use]
    pub fn retained(&self) -> usize {
        self.stats.retained.load(Ordering::Acquire)
    }

    /// The most encoded bytes the connection ever retained.
    #[must_use]
    pub fn max_retained(&self) -> usize {
        self.stats.max_retained.load(Ordering::Acquire)
    }

    /// The most bytes the connection may retain at its negotiated caps.
    #[must_use]
    pub fn retained_bound(&self) -> usize {
        self.stats.bound.load(Ordering::Acquire)
    }

    /// Whether the connection stopped reading at its outbound budget.
    #[must_use]
    pub fn reading_paused(&self) -> bool {
        self.stats.paused.load(Ordering::Acquire)
    }

    /// Whether the connection ended.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.stats.closed.load(Ordering::Acquire)
    }

    /// Whether every thread exited.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.threads.iter().all(JoinHandle::is_finished)
    }

    /// Waits for every thread to exit.
    ///
    /// # Errors
    /// Returns the payload of a thread that panicked.
    pub fn join(self) -> thread::Result<()> {
        let mut result = Ok(());
        for thread in self.threads {
            if let Err(panic) = thread.join() {
                result = Err(panic);
            }
        }
        result
    }
}

/// How a listener admitted a connection.
#[derive(Debug, Default)]
pub(crate) struct Admission {
    /// Answer the client's `Hello` with `Reject { ServerBusy }`.
    pub(crate) busy: bool,
    /// The listener's count of this connection, which the driver holds
    /// until it exits.
    pub(crate) slot: Option<Slot>,
}

/// Serves `stream` as one connection of `server`.
///
/// # Errors
/// Returns the error of cloning the stream or spawning a thread.
pub fn serve(
    server: Arc<Server>,
    stream: UnixStream,
) -> io::Result<ConnectionHandle> {
    serve_admitted(server, stream, Admission::default())
}

/// Serves `stream` as [`serve`] does, as `admission` says.
pub(crate) fn serve_admitted(
    server: Arc<Server>,
    stream: UnixStream,
    admission: Admission,
) -> io::Result<ConnectionHandle> {
    // Every clone shares the setting: the driver writes without blocking,
    // and the reader and writer wait with `poll`.
    stream.set_nonblocking(true)?;
    let reader = stream.try_clone()?;
    let writer = stream.try_clone()?;
    let (events, inbox) = async_channel::unbounded();
    // One permit at a time keeps at most one read buffer in flight.
    let (permits, permit) = async_channel::bounded(1);
    // The writer holds one batch, and how much of it the driver already
    // wrote; the connection keeps everything else.
    let (batches, batch) = async_channel::bounded::<(Transmit, usize)>(1);
    let stats = Arc::new(Stats::default());
    let mut threads = Vec::with_capacity(3);
    threads.push(
        thread::Builder::new()
            .name("huterm-server-reader".into())
            .spawn({
                let events = events.clone();
                move || read(reader, &permit, &events)
            })?,
    );
    threads.push(
        thread::Builder::new()
            .name("huterm-server-writer".into())
            .spawn({
                let events = events.clone();
                move || write(writer, &batch, &events)
            })?,
    );
    threads.push(
        thread::Builder::new()
            .name("huterm-server-driver".into())
            .spawn({
                let events = events.clone();
                let stats = Arc::clone(&stats);
                move || {
                    let driver = Driver {
                        server,
                        stream,
                        events,
                        inbox,
                        permits: Some(permits),
                        batches: Some(batches),
                        stats,
                        admission,
                    };
                    let executor = LocalExecutor::new();
                    futures_lite::future::block_on(
                        executor.run(driver.run(&executor)),
                    );
                }
            })?,
    );
    Ok(ConnectionHandle {
        threads,
        events,
        stats,
    })
}

fn read(mut stream: UnixStream, permit: &Receiver<()>, events: &Sender<Event>) {
    let mut buffer = vec![0; READ_BUFFER_BYTES];
    while permit.recv_blocking().is_ok() {
        let read = loop {
            match stream.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if let Err(error) =
                        ready(&stream, nix::poll::PollFlags::POLLIN)
                    {
                        break Err(error);
                    }
                }
                read => break read,
            }
        };
        let event = match read {
            Ok(0) | Err(_) => Event::ReadEnded,
            Ok(read) => Event::Read(buffer[..read].to_vec()),
        };
        let ended = matches!(event, Event::ReadEnded);
        if events.send_blocking(event).is_err() || ended {
            return;
        }
    }
}

/// Waits until `stream` is ready for `events`, as a blocking call on it
/// would; the socket is non-blocking so the driver never blocks on it.
fn ready(stream: &UnixStream, events: nix::poll::PollFlags) -> io::Result<()> {
    use std::os::fd::AsFd;

    use nix::errno::Errno;
    use nix::poll::{PollFd, PollTimeout, poll};
    loop {
        let mut fds = [PollFd::new(stream.as_fd(), events)];
        match poll(&mut fds, PollTimeout::NONE) {
            Ok(_) => return Ok(()),
            Err(Errno::EINTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

/// Writes as much of `transmit` after its first `written` bytes as the
/// socket takes without blocking, returning the bytes written, or `None`
/// once the socket failed.
fn send_now(
    mut stream: &UnixStream,
    transmit: &Transmit,
    mut written: usize,
) -> Option<usize> {
    let start = written;
    while written < transmit.len() {
        match stream.write_vectored(&transmit.io_slices(written)) {
            Ok(0) => return None,
            Ok(count) => written += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => return None,
        }
    }
    Some(written - start)
}

fn write(
    mut stream: UnixStream,
    batch: &Receiver<(Transmit, usize)>,
    events: &Sender<Event>,
) {
    while let Ok((transmit, mut written)) = batch.recv_blocking() {
        while written < transmit.len() {
            match stream.write_vectored(&transmit.io_slices(written)) {
                Ok(0) => {
                    let _ = events.send_blocking(Event::WriteFailed);
                    return;
                }
                Ok(count) => {
                    written += count;
                    if events.send_blocking(Event::Written(count)).is_err() {
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if ready(&stream, nix::poll::PollFlags::POLLOUT).is_err() {
                        let _ = events.send_blocking(Event::WriteFailed);
                        return;
                    }
                }
                Err(_) => {
                    let _ = events.send_blocking(Event::WriteFailed);
                    return;
                }
            }
        }
        if events.send_blocking(Event::WriterIdle).is_err() {
            return;
        }
    }
}

struct Driver {
    server: Arc<Server>,
    stream: UnixStream,
    events: Sender<Event>,
    inbox: Receiver<Event>,
    permits: Option<Sender<()>>,
    batches: Option<Sender<(Transmit, usize)>>,
    stats: Arc<Stats>,
    admission: Admission,
}

enum Woke {
    Event(Event),
    Deadline,
}

impl Driver {
    async fn run(mut self, executor: &LocalExecutor<'_>) {
        let start = Instant::now();
        let now = || start.elapsed();
        let mut connection = self.server.connection(now());
        if self.admission.busy {
            connection.refuse(Reject {
                reason: RejectReason::ServerBusy,
                message: format!(
                    "the server already serves {} connections",
                    self.server.config().max_connections
                ),
            });
        }
        let mut reading = false;
        let mut writer_idle = true;
        let mut feed: Option<(u64, Task<()>)> = None;
        let mut watches: HashMap<ViewerId, (Task<()>, Arc<AtomicBool>)> =
            HashMap::new();
        loop {
            while let Some(work) = connection.poll_work() {
                self.spawn(executor, work);
            }
            self.watch_viewers(executor, &mut connection, &mut watches);
            match connection.hierarchy_feed() {
                Some((generation, _))
                    if feed.as_ref().is_some_and(|(g, _)| *g == generation) => {
                }
                Some((generation, current)) => {
                    let task =
                        executor.spawn(forward(current, self.events.clone()));
                    feed = Some((generation, task));
                }
                None => feed = None,
            }
            self.transmit(&mut connection, &mut writer_idle, now());
            if !reading
                && connection.wants_input()
                && let Some(permits) = &self.permits
            {
                reading = permits.try_send(()).is_ok();
            }
            self.publish(&connection);
            if connection.is_closed() && self.permits.is_some() {
                // Blocked reads and writes return, and closed channels end
                // the threads that wait on them.
                let _ = self.stream.shutdown(Shutdown::Both);
                self.permits = None;
                self.batches = None;
                feed = None;
                watches.clear();
            }
            if connection.is_finished() {
                // Every job and its cleanup ran: the listener may admit
                // another connection in this one's place.
                drop(self.admission.slot.take());
                return;
            }
            let deadline = connection.next_deadline().map(|at| start + at);
            match self.wait(deadline).await {
                Woke::Deadline => connection.tick(now()),
                Woke::Event(Event::Read(bytes)) => {
                    reading = false;
                    connection.receive(&bytes, now());
                    self.stats
                        .received
                        .fetch_add(bytes.len(), Ordering::AcqRel);
                }
                Woke::Event(
                    Event::ReadEnded | Event::WriteFailed | Event::Close,
                ) => {
                    reading = false;
                    for work in connection.close(now()) {
                        self.spawn(executor, work);
                    }
                }
                Woke::Event(Event::Written(count)) => {
                    connection.transmitted(count, now());
                    self.stats.written.fetch_add(count, Ordering::AcqRel);
                }
                Woke::Event(Event::WriterIdle) => writer_idle = true,
                Woke::Event(Event::Completed(id, completion)) => {
                    connection.complete(id, *completion, now());
                }
                Woke::Event(Event::Hierarchy) => connection.pump(now()),
                Woke::Event(Event::Viewer(viewer)) => {
                    if let Some((_, queued)) = watches.get(&viewer) {
                        queued.store(false, Ordering::Release);
                    }
                    connection.viewer_woke(viewer, now());
                }
                Woke::Event(Event::ViewerEnded(viewer)) => {
                    watches.remove(&viewer);
                    connection.viewer_ended(viewer, now());
                }
            }
        }
    }

    /// Starts forwarding new viewers' wakes and stops forwarding ended
    /// ones'.
    fn watch_viewers(
        &self,
        executor: &LocalExecutor<'_>,
        connection: &mut Connection,
        watches: &mut HashMap<ViewerId, (Task<()>, Arc<AtomicBool>)>,
    ) {
        while let Some((viewer, wake)) = connection.poll_watch() {
            let queued = Arc::new(AtomicBool::new(false));
            let task = executor.spawn(watch(
                viewer,
                wake,
                Arc::clone(&queued),
                self.events.clone(),
            ));
            watches.insert(viewer, (task, queued));
        }
        while let Some(viewer) = connection.poll_unwatch() {
            watches.remove(&viewer);
        }
    }

    /// Writes what the connection has to send while the writer is idle:
    /// at once when the socket takes it, else by handing the rest to the
    /// writer thread.
    fn transmit(
        &self,
        connection: &mut Connection,
        writer_idle: &mut bool,
        now: std::time::Duration,
    ) {
        while *writer_idle
            && let Some(batches) = &self.batches
            && let Some(transmit) = connection.poll_transmit(WRITE_BATCH_BYTES)
        {
            let Some(written) = send_now(&self.stream, &transmit, 0) else {
                let _ = self.events.try_send(Event::WriteFailed);
                return;
            };
            if written > 0 {
                connection.transmitted(written, now);
                self.stats.written.fetch_add(written, Ordering::AcqRel);
            }
            if written < transmit.len() {
                self.stats.short_writes.fetch_add(1, Ordering::AcqRel);
                *writer_idle = batches.try_send((transmit, written)).is_err();
            }
        }
    }

    async fn wait(&self, deadline: Option<Instant>) -> Woke {
        let event = async {
            match self.inbox.recv().await {
                Ok(event) => Woke::Event(event),
                // The driver holds a sender, so the inbox never closes.
                Err(_) => Woke::Event(Event::Close),
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

    fn spawn(&self, executor: &LocalExecutor<'_>, work: Work) {
        let events = self.events.clone();
        let Work { id, job } = work;
        if let Job::Viewer(job) = job {
            // A viewer job goes to its terminal at once and blocks no
            // thread: the driver awaits its reply.
            let reply = job.start();
            executor
                .spawn(async move {
                    let completion = reply.await;
                    let _ = events
                        .send(Event::Completed(id, Box::new(completion)))
                        .await;
                })
                .detach();
            return;
        }
        let server = Arc::clone(&self.server);
        executor
            .spawn(async move {
                let completion =
                    blocking::unblock(move || runtime::execute(&server, job))
                        .await;
                let _ = events
                    .send(Event::Completed(id, Box::new(completion)))
                    .await;
            })
            .detach();
    }

    fn publish(&self, connection: &Connection) {
        let retained = connection.retained();
        self.stats.retained.store(retained, Ordering::Release);
        self.stats
            .max_retained
            .fetch_max(retained, Ordering::AcqRel);
        self.stats
            .bound
            .store(connection.retained_bound(), Ordering::Release);
        self.stats.paused.store(
            !connection.is_closed() && !connection.wants_input(),
            Ordering::Release,
        );
        self.stats
            .closed
            .store(connection.is_closed(), Ordering::Release);
    }
}

/// Forwards a viewer's wakes to the driver, at most one queued at a time,
/// then its end.
async fn watch(
    viewer: ViewerId,
    wake: Box<dyn WakeSource>,
    queued: Arc<AtomicBool>,
    events: Sender<Event>,
) {
    while wake.wait().await {
        if !queued.swap(true, Ordering::AcqRel)
            && events.send(Event::Viewer(viewer)).await.is_err()
        {
            return;
        }
    }
    let _ = events.send(Event::ViewerEnded(viewer)).await;
}

async fn forward(feed: Arc<dyn HierarchyFeed>, events: Sender<Event>) {
    while feed.activity().await {
        if events.send(Event::Hierarchy).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests;
