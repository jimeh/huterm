//! Accepting connections on a Unix socket.
//!
//! A [`Listener`] runs one thread that waits with `poll` on its socket and
//! on a wake socket that stopping the listener closes. For each connection
//! it accepts, it first compares the peer's user ID with its own effective
//! user ID and closes a foreign peer's connection at once, before the
//! preamble. It serves up to [`ServerConfig::max_connections`] connections
//! with [`native::serve`](crate::native::serve)'s threads. Past that, it
//! answers up to [`MAX_REFUSALS`] connections' `Hello` with
//! `Reject { ServerBusy }` within the handshake deadline, and closes any
//! further connection at once, so a flood of connections costs no more
//! threads.
//!
//! Stopping the listener closes every connection it serves, waits for their
//! threads, and removes the socket file if it is still the one the listener
//! bound. Stale sockets and startup races belong to the daemon (#45).

use std::io;
use std::net::Shutdown;
use std::os::fd::AsFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};

use huterm_socket::{SocketError, SocketFile, SocketPath};
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

use crate::config::MAX_REFUSALS;
#[cfg(doc)]
use crate::config::ServerConfig;
use crate::native::{Admission, ConnectionHandle, serve_admitted};
use crate::server::Server;

/// How long the listener waits after `accept` fails for a reason other
/// than an empty queue, such as running out of descriptors, rather than
/// spinning on a socket that stays readable.
const ACCEPT_BACKOFF_MILLIS: u16 = 100;

/// The most `accept` calls between two checks of the wake socket, so a
/// flood of connections never holds off a stop.
const ACCEPT_BATCH: usize = 16;

/// Counts one connection in a listener's count until dropped. A
/// connection's driver thread holds it until the thread exits.
#[derive(Debug)]
pub(crate) struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn new(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        Self(Arc::clone(count))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, Default)]
struct Stats {
    served: Arc<AtomicUsize>,
    refusing: Arc<AtomicUsize>,
    refused_peers: AtomicUsize,
}

/// A server's listening socket and the thread that accepts on it.
/// Dropping it stops it as [`Listener::shutdown`] does.
#[derive(Debug)]
pub struct Listener {
    wake: UnixStream,
    thread: Option<JoinHandle<()>>,
    file: Option<SocketFile>,
    stats: Arc<Stats>,
}

impl Listener {
    /// Listens on `socket` for `server`'s clients, after creating and
    /// checking its directory as [`huterm_socket::bind`] does.
    ///
    /// # Errors
    /// Returns what is wrong with the socket's directory, or the failing
    /// call's error.
    pub fn bind(
        server: Arc<Server>,
        socket: &SocketPath,
    ) -> Result<Self, SocketError> {
        let (listener, file) = huterm_socket::bind(socket)?;
        match Self::start(server, listener, huterm_socket::effective_uid()) {
            Ok(mut started) => {
                started.file = Some(file);
                Ok(started)
            }
            Err(error) => {
                let _ = file.remove();
                Err(SocketError::Io {
                    action: "listen on socket",
                    path: socket.path().to_path_buf(),
                    error,
                })
            }
        }
    }

    /// Accepts `server`'s clients on a socket bound elsewhere, such as one
    /// a parent process passed down. The listener checks every peer's
    /// credentials but leaves the socket file to whoever bound it.
    ///
    /// # Errors
    /// Returns the error of configuring the socket or spawning the thread.
    pub fn from_listener(
        server: Arc<Server>,
        listener: UnixListener,
    ) -> io::Result<Self> {
        Self::start(server, listener, huterm_socket::effective_uid())
    }

    /// Starts accepting, admitting only peers that run as user `uid`.
    pub(crate) fn start(
        server: Arc<Server>,
        listener: UnixListener,
        uid: u32,
    ) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        let (wake, woken) = UnixStream::pair()?;
        let stats = Arc::new(Stats::default());
        let thread = thread::Builder::new()
            .name("huterm-server-listener".into())
            .spawn({
                let stats = Arc::clone(&stats);
                move || accept_loop(&server, &listener, &woken, uid, &stats)
            })?;
        Ok(Self {
            wake,
            thread: Some(thread),
            file: None,
            stats,
        })
    }

    /// The socket file the listener bound and removes when it stops.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.file.as_ref().map(SocketFile::path)
    }

    /// The connections the listener serves now. A connection counts until
    /// its driver has run every job and its cleanup, just before its
    /// threads exit.
    #[must_use]
    pub fn connections(&self) -> usize {
        self.stats.served.load(Ordering::Acquire)
    }

    /// The connections the listener is refusing with `ServerBusy` now.
    #[must_use]
    pub fn refusals(&self) -> usize {
        self.stats.refusing.load(Ordering::Acquire)
    }

    /// The connections closed at once because their peer runs as another
    /// user, or its credentials could not be read.
    #[must_use]
    pub fn refused_peers(&self) -> usize {
        self.stats.refused_peers.load(Ordering::Acquire)
    }

    /// Stops accepting, closes every connection, waits for their threads,
    /// and removes the socket file if it is still the one this listener
    /// bound.
    ///
    /// # Errors
    /// Returns the payload of a thread that panicked.
    pub fn shutdown(mut self) -> thread::Result<()> {
        self.stop()
    }

    fn stop(&mut self) -> thread::Result<()> {
        // The accept thread sees the closed wake socket as readable.
        let _ = self.wake.shutdown(Shutdown::Both);
        let result = self.thread.take().map_or(Ok(()), JoinHandle::join);
        if let Some(file) = self.file.take() {
            let _ = file.remove();
        }
        result
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

enum Woke {
    Accept,
    Stop,
    Idle,
}

/// Waits until `listener`, if any, has a connection, the wake socket
/// closes, or `timeout` passes.
fn wait(
    listener: Option<&UnixListener>,
    woken: &UnixStream,
    timeout: PollTimeout,
) -> Woke {
    loop {
        let mut fds = vec![PollFd::new(woken.as_fd(), PollFlags::POLLIN)];
        if let Some(listener) = listener {
            fds.push(PollFd::new(listener.as_fd(), PollFlags::POLLIN));
        }
        match poll(&mut fds, timeout) {
            Ok(0) => return Woke::Idle,
            Ok(_) => {
                return if fds[0]
                    .revents()
                    .is_some_and(|events| !events.is_empty())
                {
                    Woke::Stop
                } else {
                    Woke::Accept
                };
            }
            Err(Errno::EINTR) => {}
            Err(_) => return Woke::Stop,
        }
    }
}

fn accept_loop(
    server: &Arc<Server>,
    listener: &UnixListener,
    woken: &UnixStream,
    uid: u32,
    stats: &Stats,
) {
    let mut handles = Vec::new();
    let mut stopping = false;
    while !stopping {
        match wait(Some(listener), woken, PollTimeout::NONE) {
            Woke::Stop => break,
            Woke::Accept | Woke::Idle => {}
        }
        for _ in 0..ACCEPT_BATCH {
            match listener.accept() {
                Ok((stream, _)) => {
                    reap(&mut handles);
                    if let Some(handle) = admit(server, stream, uid, stats) {
                        handles.push(handle);
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted
                            | io::ErrorKind::ConnectionAborted
                    ) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(_) => {
                    let backoff = PollTimeout::from(ACCEPT_BACKOFF_MILLIS);
                    stopping = matches!(wait(None, woken, backoff), Woke::Stop);
                    break;
                }
            }
        }
    }
    for handle in &handles {
        handle.close();
    }
    let mut panic = None;
    for handle in handles {
        if let Err(payload) = handle.join() {
            panic.get_or_insert(payload);
        }
    }
    if let Some(payload) = panic {
        std::panic::resume_unwind(payload);
    }
}

/// Joins the threads of connections that ended.
fn reap(handles: &mut Vec<ConnectionHandle>) {
    let (finished, live): (Vec<_>, Vec<_>) = std::mem::take(handles)
        .into_iter()
        .partition(ConnectionHandle::is_finished);
    *handles = live;
    for handle in finished {
        let _ = handle.join();
    }
}

/// Serves `stream`, refuses it as busy, or closes it at once.
fn admit(
    server: &Arc<Server>,
    stream: UnixStream,
    uid: u32,
    stats: &Stats,
) -> Option<ConnectionHandle> {
    if huterm_socket::check_peer(&stream, uid).is_err() {
        // Dropping the stream closes it before the server writes anything.
        stats.refused_peers.fetch_add(1, Ordering::AcqRel);
        return None;
    }
    let busy =
        stats.served.load(Ordering::Acquire) >= server.config().max_connections;
    let slot = if !busy {
        Slot::new(&stats.served)
    } else if stats.refusing.load(Ordering::Acquire) < MAX_REFUSALS {
        Slot::new(&stats.refusing)
    } else {
        return None;
    };
    serve_admitted(
        Arc::clone(server),
        stream,
        Admission {
            busy,
            slot: Some(slot),
        },
    )
    .ok()
}

#[cfg(test)]
mod tests;
