use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use huterm_protocol::SessionId;
use huterm_wire::message::Message;
use huterm_wire::request::Request;

use super::*;
use crate::config::ServerConfig;
use crate::test_support::{TestPeer, server};

/// How long a test waits for threads that should exit promptly.
const EXIT_DEADLINE: Duration = Duration::from_secs(10);

/// Joins `handle`'s threads, failing if they have not all exited within
/// [`EXIT_DEADLINE`].
fn joined(handle: ConnectionHandle) {
    let (done, exited) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(handle.join().is_ok());
    });
    match exited.recv_timeout(EXIT_DEADLINE) {
        Ok(clean) => assert!(clean, "a connection thread panicked"),
        Err(error) => {
            panic!(
                "connection threads still running after {EXIT_DEADLINE:?}: {error}"
            )
        }
    }
}

/// Reads from `stream` until `peer` decodes a message matching `wanted`.
fn read_until(
    stream: &mut UnixStream,
    peer: &mut TestPeer,
    wanted: impl Fn(&Message) -> bool,
) -> Message {
    let mut buffer = vec![0; 64 << 10];
    loop {
        let read = stream.read(&mut buffer).expect("the server's bytes");
        assert!(read > 0, "the server closed before answering");
        if let Some(message) = peer
            .receive(&buffer[..read])
            .into_iter()
            .find(|message| wanted(message))
        {
            return message;
        }
    }
}

fn connect(server: &Arc<Server>) -> (UnixStream, TestPeer, ConnectionHandle) {
    let (mut client, accepted) = UnixStream::pair().unwrap();
    let handle = serve(Arc::clone(server), accepted).unwrap();
    let mut peer = TestPeer::new(server.runtime());
    client.write_all(&peer.opening()).unwrap();
    read_until(&mut client, &mut peer, |message| {
        matches!(message, Message::Welcome(_))
    });
    (client, peer, handle)
}

#[test]
fn every_thread_exits_after_the_peer_vanishes() {
    let server = Arc::new(server(ServerConfig::default()));
    let (mut client, mut peer, handle) = connect(&server);
    let (id, bytes) = peer.request(Request::CreateSession { name: None });
    client.write_all(&bytes).unwrap();
    read_until(
        &mut client,
        &mut peer,
        |message| matches!(message, Message::Result(result) if result.request == id),
    );
    drop(client);
    joined(handle);
    let _ = server.lock().shutdown();
}

#[test]
fn every_thread_exits_after_the_server_ends_the_connection() {
    let server = Arc::new(server(ServerConfig::default()));
    let (mut client, accepted) = UnixStream::pair().unwrap();
    let handle = serve(Arc::clone(&server), accepted).unwrap();
    client.write_all(b"HTTP/1.1 200 OK\r\n").unwrap();
    // The server shuts the socket down: the client reads its preamble, if
    // it was written, and then the end.
    let mut rest = Vec::new();
    client.read_to_end(&mut rest).unwrap();
    assert!(rest.len() <= huterm_wire::frame::PREAMBLE_LEN);
    joined(handle);
}

#[test]
fn a_flooding_client_meets_backpressure_and_retained_bytes_stay_bounded() {
    let server = Arc::new(server(ServerConfig {
        outbound_budget: 64 << 10,
        ..ServerConfig::default()
    }));
    let (mut client, mut peer, handle) = connect(&server);
    let session = SessionId::in_runtime(server.runtime(), 999_999);
    // The server answers before the flood starts.
    let (id, bytes) = peer.request(Request::SelectSession { session });
    client.write_all(&bytes).unwrap();
    read_until(
        &mut client,
        &mut peer,
        |message| matches!(message, Message::Result(result) if result.request == id),
    );
    // Write requests without reading until the socket refuses more while
    // the server has stopped reading.
    client.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut pending = Vec::new();
    loop {
        if pending.is_empty() {
            pending = peer.request(Request::SelectSession { session }).1;
        }
        match client.write(&pending) {
            Ok(written) => {
                pending.drain(..written);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if handle.reading_paused() {
                    break;
                }
                assert!(Instant::now() < deadline, "the server never paused");
                std::thread::yield_now();
            }
            Err(error) => panic!("write failed: {error}"),
        }
    }
    assert!(handle.retained() >= 64 << 10, "paused at its budget");
    assert!(
        handle.max_retained() <= handle.retained_bound(),
        "{} > {}",
        handle.max_retained(),
        handle.retained_bound()
    );
    drop(client);
    joined(handle);
    let _ = server.lock().shutdown();
}
