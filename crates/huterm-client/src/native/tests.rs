use std::future::Future;
use std::sync::mpsc;
use std::time::Duration;

use huterm_protocol::DigestProfile;
use huterm_server::native::{ConnectionHandle, serve};
use huterm_server::test_support::server;
use huterm_server::{Server, ServerConfig};
use huterm_wire::handshake::ClientKind;
use huterm_wire::response::Response;

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
