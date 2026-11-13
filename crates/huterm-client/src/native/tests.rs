use std::future::Future;
use std::sync::mpsc;
use std::time::Duration;

use huterm_protocol::{DigestProfile, RuntimeId, SessionId};
use huterm_server::native::{ConnectionHandle, serve};
use huterm_server::test_support::{SocketServer, server};
use huterm_server::{Server, ServerConfig};
use huterm_socket::testing::TestDirectory;
use huterm_wire::caps::MessageCaps;
use huterm_wire::codec::IdScope;
use huterm_wire::frame::{DEFAULT_MAX_FRAME, Frame, FrameEnd, preamble};
use huterm_wire::handshake::{ClientKind, Reject, RejectReason};
use huterm_wire::message::{Message, encode_message};
use huterm_wire::response::Response;

use crate::session::HierarchyUpdate;

use super::*;

/// How long a test waits for something that should happen promptly.
const PROMPT: Duration = Duration::from_secs(10);

fn config() -> ClientConfig {
    ClientConfig::local(ClientKind::Desktop, "test", "test")
}

/// Runs `future` to completion, failing the test after [`PROMPT`].
fn block_on<T>(future: impl Future<Output = T>) -> T {
    futures_lite::future::block_on(futures_lite::future::or(
        async { Some(future.await) },
        async {
            async_io::Timer::after(PROMPT).await;
            None
        },
    ))
    .unwrap_or_else(|| panic!("nothing resolved within {PROMPT:?}"))
}

/// Serves one end of a fresh pair and returns the other.
fn serve_pair(server: &Arc<Server>) -> (UnixStream, ConnectionHandle) {
    let (client, accepted) = UnixStream::pair().unwrap();
    (client, serve(Arc::clone(server), accepted).unwrap())
}

/// Joins a server connection's threads, failing after [`PROMPT`].
fn joined(handle: ConnectionHandle) {
    let (done, exited) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(handle.join().is_ok());
    });
    assert!(
        exited.recv_timeout(PROMPT).expect("server threads exited"),
        "a server thread panicked"
    );
}

#[test]
fn requests_resolve_and_the_projection_follows_the_server_over_a_unix_stream() {
    let server = Arc::new(server(ServerConfig::default()));
    let (stream, connection) = serve_pair(&server);
    let client = Client::spawn(config(), stream).unwrap();
    let handle = client.handle();
    let connected = block_on(handle.connected()).unwrap();
    assert!(connected.new_incarnation);
    block_on(handle.request(Request::SubscribeHierarchy)).unwrap();
    let created =
        block_on(handle.request(Request::CreateSession { name: None }))
            .unwrap();
    assert!(matches!(created.response, Response::Session(_)));
    let committed = created.committed.unwrap();
    assert_eq!(block_on(handle.wait_for(committed.seq)), Resolution::Ready);
    let snapshot = block_on(handle.hierarchy()).unwrap();
    assert_eq!(snapshot.state.seq(), committed.seq);
    assert_eq!(Some(snapshot.digest), committed.digest);
    assert_eq!(
        snapshot.digest,
        server.lock().hierarchy_digest(DigestProfile::Base)
    );
    client.shutdown().unwrap();
    joined(connection);
    let _ = server.lock().shutdown();
}

#[test]
fn every_io_thread_exits_after_a_disconnect_and_the_driver_after_shutdown() {
    let server = Arc::new(server(ServerConfig::default()));
    let (stream, connection) = serve_pair(&server);
    let client = Client::spawn(config(), stream).unwrap();
    let handle = client.handle();
    let events = block_on(handle.events()).unwrap();
    block_on(handle.connected()).unwrap();
    assert_eq!(client.io_threads(), 2);

    // The server ends the connection; the driver joins both I/O threads
    // before it reports the end.
    connection.close();
    let closed = block_on(async {
        loop {
            if let ClientEvent::Closed(reason) = events.recv().await.unwrap() {
                return reason;
            }
        }
    });
    assert_eq!(closed, CloseReason::Disconnected);
    assert_eq!(client.io_threads(), 0);
    joined(connection);

    // A new stream to the same server keeps the incarnation's mapping.
    let (stream, replacement) = serve_pair(&server);
    let reconnected = block_on(handle.reconnect(stream)).unwrap();
    assert!(!reconnected.new_incarnation);
    assert_eq!(client.io_threads(), 2);
    block_on(handle.request(Request::CreateSession { name: None })).unwrap();

    client.shutdown().unwrap();
    assert_eq!(
        block_on(handle.request(Request::SubscribeHierarchy))
            .unwrap_err()
            .code,
        ErrorCode::Disconnected
    );
    joined(replacement);
    let _ = server.lock().shutdown();
}

#[test]
fn reconnecting_over_a_live_connection_resolves_with_the_new_handshake() {
    let server = Arc::new(server(ServerConfig::default()));
    let (stream, first) = serve_pair(&server);
    let client = Client::spawn(config(), stream).unwrap();
    let handle = client.handle();
    block_on(handle.connected()).unwrap();

    let (stream, second) = serve_pair(&server);
    let reconnected = block_on(handle.reconnect(stream)).unwrap();
    assert!(!reconnected.new_incarnation);
    joined(first);
    let created =
        block_on(handle.request(Request::CreateSession { name: None }))
            .unwrap();
    assert!(matches!(created.response, Response::Session(_)));

    client.shutdown().unwrap();
    joined(second);
    let _ = server.lock().shutdown();
}

/// Subscribes and commits more hierarchy events than the event channel
/// holds, without draining it; returns the renamed session and the last
/// sequence, which the projection has reached.
fn overflow_events(handle: &ClientHandle) -> (SessionId, u64) {
    block_on(handle.request(Request::SubscribeHierarchy)).unwrap();
    let Response::Session(session) =
        block_on(handle.request(Request::CreateSession { name: None }))
            .unwrap()
            .response
    else {
        panic!("a created session");
    };
    let mut last = 0;
    for index in 0..EVENT_CHANNEL_CAPACITY {
        last = rename(handle, session, &format!("name {index}"));
    }
    assert_eq!(block_on(handle.wait_for(last)), Resolution::Ready);
    (session, last)
}

/// Renames `session` and returns the rename's sequence.
fn rename(handle: &ClientHandle, session: SessionId, name: &str) -> u64 {
    block_on(handle.request(Request::RenameSession {
        session,
        name: Some(name.into()),
    }))
    .unwrap()
    .committed
    .unwrap()
    .seq
}

#[test]
fn an_undrained_event_receiver_stays_at_capacity_and_then_reports_one_lag() {
    let server = Arc::new(server(ServerConfig::default()));
    let (stream, connection) = serve_pair(&server);
    let client = Client::spawn(config(), stream).unwrap();
    let handle = client.handle();
    let events = block_on(handle.events()).unwrap();
    block_on(handle.connected()).unwrap();
    let (session, last) = overflow_events(&handle);
    assert_eq!(events.len(), EVENT_CHANNEL_CAPACITY);

    let mut delivered = 0;
    while block_on(events.recv()).unwrap() != ClientEvent::Lagged {
        delivered += 1;
    }
    assert_eq!(delivered, EVENT_CHANNEL_CAPACITY);
    let snapshot = block_on(handle.hierarchy()).unwrap();
    assert_eq!(snapshot.state.seq(), last);

    // Forwarding resumes after the marker, and without another overflow
    // no second marker follows.
    let renamed = rename(&handle, session, "after the lag");
    assert_eq!(block_on(handle.wait_for(renamed)), Resolution::Ready);
    assert!(matches!(
        events.try_recv(),
        Ok(ClientEvent::Hierarchy(HierarchyUpdate::Applied { seq, .. }))
            if seq == renamed
    ));
    assert!(events.is_empty());

    client.shutdown().unwrap();
    joined(connection);
    let _ = server.lock().shutdown();
}

#[test]
fn connected_reports_why_the_last_connection_ended_after_its_close_was_dropped()
{
    let server = Arc::new(server(ServerConfig::default()));
    let (stream, connection) = serve_pair(&server);
    let client = Client::spawn(config(), stream).unwrap();
    let handle = client.handle();
    let events = block_on(handle.events()).unwrap();
    block_on(handle.connected()).unwrap();
    overflow_events(&handle);
    assert_eq!(events.len(), EVENT_CHANNEL_CAPACITY);

    // The next peer refuses the `Hello`; its bytes wait in the socket.
    let (stream, mut peer) = UnixStream::pair().unwrap();
    let reject = Reject {
        reason: RejectReason::ServerBusy,
        message: "busy".into(),
    };
    let payload = encode_message(
        &Message::Reject(reject.clone()),
        IdScope::new(RuntimeId::new(0)),
        MessageCaps::for_minor(0),
    )
    .unwrap();
    let frame = Frame {
        stream: 0,
        end: FrameEnd::Final,
        payload,
    }
    .encode(DEFAULT_MAX_FRAME)
    .unwrap();
    peer.write_all(&[&preamble()[..], &frame].concat()).unwrap();
    let rejected = Err(CloseReason::Rejected(reject));
    assert_eq!(block_on(handle.reconnect(stream)), rejected);
    joined(connection);
    assert_eq!(
        events.len(),
        EVENT_CHANNEL_CAPACITY,
        "the close was dropped"
    );
    assert_eq!(block_on(handle.connected()), rejected);

    client.shutdown().unwrap();
    drop(peer);
    let _ = server.lock().shutdown();
}

#[test]
fn a_client_connects_by_socket_path_and_reconnects_through_it() {
    let server = Arc::new(server(ServerConfig::default()));
    let listening = SocketServer::start(Arc::clone(&server));
    let client = Client::connect(config(), listening.socket()).unwrap();
    let handle = client.handle();
    assert!(block_on(handle.connected()).unwrap().new_incarnation);
    block_on(handle.request(Request::CreateSession { name: None })).unwrap();

    let stream = connect(listening.socket()).unwrap();
    let reconnected = block_on(handle.reconnect(stream)).unwrap();
    assert!(!reconnected.new_incarnation);
    block_on(handle.request(Request::CreateSession { name: None })).unwrap();

    client.shutdown().unwrap();
    listening.shutdown().unwrap();
    let _ = server.lock().shutdown();
}

#[test]
fn connecting_where_no_server_listens_fails_with_the_socket_path() {
    let directory = TestDirectory::new();
    let socket = directory.socket("absent");
    let error = Client::connect(config(), &socket).unwrap_err();
    assert!(
        matches!(&error, SocketError::Io { path, .. } if path == socket.path()),
        "{error:?}"
    );
}

#[test]
fn a_welcome_split_at_every_byte_completes_the_handshake_over_a_bound_socket() {
    let server = server(ServerConfig::default());
    let directory = TestDirectory::new();
    let socket = directory.socket("test");
    let (listener, _file) = huterm_socket::bind(&socket).unwrap();
    for split in 1.. {
        let client = Client::connect(config(), &socket).unwrap();
        let (mut accepted, _) = listener.accept().unwrap();
        // A sans-IO connection plays the server: its preamble, then its
        // Welcome to the client's Hello.
        let mut connection = server.connection(Duration::ZERO);
        let mut answer = Vec::new();
        let mut buffer = vec![0; 64 << 10];
        while answer.len() <= huterm_wire::frame::PREAMBLE_LEN {
            let read = accepted.read(&mut buffer).unwrap();
            assert!(read > 0, "the client closed before its Hello");
            connection.receive(&buffer[..read], Duration::ZERO);
            while let Some(transmit) = connection.poll_transmit(usize::MAX) {
                connection.transmitted(transmit.len(), Duration::ZERO);
                answer.extend(transmit.to_bytes());
            }
        }
        if split == answer.len() {
            client.shutdown().unwrap();
            break;
        }
        // The client reads the first part before the rest exists.
        accepted.write_all(&answer[..split]).unwrap();
        let deadline = std::time::Instant::now() + PROMPT;
        while client.received() < split {
            assert!(
                std::time::Instant::now() < deadline,
                "the client read {} of {split} bytes",
                client.received()
            );
            thread::sleep(Duration::from_micros(200));
        }
        assert_eq!(client.received(), split);
        accepted.write_all(&answer[split..]).unwrap();
        let connected = block_on(client.handle().connected()).unwrap();
        assert_eq!(connected.incarnation, server.offer().incarnation);
        client.shutdown().unwrap();
    }
}
