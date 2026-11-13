use std::io::{ErrorKind, Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use huterm_protocol::SessionId;
use huterm_socket::testing::TestDirectory;
use huterm_wire::frame::{PREAMBLE_LEN, preamble};
use huterm_wire::handshake::RejectReason;
use huterm_wire::message::Message;
use huterm_wire::request::Request;

use super::*;
use crate::config::ServerConfig;
use crate::native::serve;
use crate::test_support::{SocketServer, TestPeer, server};

/// How long a test waits for something that should happen promptly.
const PROMPT: Duration = Duration::from_secs(10);

/// Waits until `condition` holds, failing after [`PROMPT`] with `what`.
fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + PROMPT;
    while !condition() {
        assert!(Instant::now() < deadline, "{what} within {PROMPT:?}");
        thread::sleep(Duration::from_micros(200));
    }
}

/// Runs `stop` on another thread, failing if it does not return within
/// [`PROMPT`] or a thread it joined panicked.
fn stopped(stop: impl FnOnce() -> thread::Result<()> + Send + 'static) {
    let (done, exited) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(stop().is_ok());
    });
    assert!(
        exited.recv_timeout(PROMPT).expect("the threads exit"),
        "a thread panicked"
    );
}

/// Reads one byte at a time until `peer` decodes a message matching
/// `wanted`, so every frame arrives split at every byte.
fn read_until(
    stream: &mut UnixStream,
    peer: &mut TestPeer,
    wanted: impl Fn(&Message) -> bool,
) -> Message {
    let mut byte = [0];
    loop {
        let read = stream.read(&mut byte).expect("the server's bytes");
        assert_eq!(read, 1, "the server closed before answering");
        if let Some(message) = peer
            .receive(&byte)
            .into_iter()
            .find(|message| wanted(message))
        {
            return message;
        }
    }
}

/// Everything the server sends until it closes. A connection the server
/// closes without reading what the client sent may end in a reset.
fn read_to_end(stream: &mut UnixStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    match stream.read_to_end(&mut bytes) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::ConnectionReset => {}
        Err(error) => panic!("reading the server failed: {error}"),
    }
    bytes
}

/// Connects a raw peer to `socket` and completes its handshake.
fn handshake(socket: &SocketPath, server: &Server) -> (UnixStream, TestPeer) {
    let mut stream = huterm_socket::connect(socket).unwrap();
    let mut peer = TestPeer::new(server.runtime());
    stream.write_all(&peer.opening()).unwrap();
    let message = read_until(&mut stream, &mut peer, |message| {
        matches!(message, Message::Welcome(_) | Message::Reject(_))
    });
    assert!(matches!(message, Message::Welcome(_)), "{message:?}");
    (stream, peer)
}

#[test]
fn a_peer_running_as_another_user_is_closed_before_the_preamble() {
    let server = Arc::new(server(ServerConfig::default()));
    let directory = TestDirectory::new();
    let socket = directory.socket("test");
    let (bound, _file) = huterm_socket::bind(&socket).unwrap();
    // The listener expects a user this process does not run as, so every
    // peer is foreign. The Docker test below uses a real second user.
    let listener =
        Listener::start(server, bound, huterm_socket::effective_uid() + 1)
            .unwrap();
    let mut stream = huterm_socket::connect(&socket).unwrap();
    let _ = stream.write_all(&preamble());
    assert_eq!(read_to_end(&mut stream), Vec::<u8>::new());
    assert_eq!(listener.refused_peers(), 1);
    assert_eq!(listener.connections(), 0);
    stopped(move || listener.shutdown());
}

#[test]
fn past_max_connections_a_hello_is_rejected_as_busy_until_one_ends() {
    let server = Arc::new(server(ServerConfig {
        max_connections: 1,
        ..ServerConfig::default()
    }));
    let listening = SocketServer::start(Arc::clone(&server));
    let (first, _) = handshake(listening.socket(), &server);
    assert_eq!(listening.listener().connections(), 1);

    let mut busy = huterm_socket::connect(listening.socket()).unwrap();
    let mut peer = TestPeer::new(server.runtime());
    busy.write_all(&peer.opening()).unwrap();
    let message = read_until(&mut busy, &mut peer, |_| true);
    assert!(
        matches!(&message, Message::Reject(reject) if reject.reason == RejectReason::ServerBusy),
        "{message:?}"
    );
    assert_eq!(read_to_end(&mut busy), Vec::<u8>::new());

    // Once the first connection's threads exit, its slot frees.
    drop(first);
    wait_until("the first connection ends", || {
        listening.listener().connections() == 0
    });
    let (_third, _) = handshake(listening.socket(), &server);
    stopped(move || listening.shutdown());
}

#[test]
fn past_max_refusals_a_connection_is_closed_at_once() {
    let server = Arc::new(server(ServerConfig {
        max_connections: 0,
        ..ServerConfig::default()
    }));
    let listening = SocketServer::start(Arc::clone(&server));
    // Each refusing connection writes its preamble at once, which shows
    // the listener admitted it before the next connects.
    let refusing: Vec<UnixStream> = (0..MAX_REFUSALS)
        .map(|_| {
            let mut stream =
                huterm_socket::connect(listening.socket()).unwrap();
            let mut opening = [0; PREAMBLE_LEN];
            stream.read_exact(&mut opening).unwrap();
            assert_eq!(opening, preamble());
            stream
        })
        .collect();
    assert_eq!(listening.listener().refusals(), MAX_REFUSALS);

    let mut closed = huterm_socket::connect(listening.socket()).unwrap();
    assert_eq!(read_to_end(&mut closed), Vec::<u8>::new());

    // A refusing connection still answers its Hello.
    let mut stream = refusing.into_iter().next().unwrap();
    let mut peer = TestPeer::new(server.runtime());
    peer.receive(&preamble());
    stream.write_all(&peer.opening()).unwrap();
    let message = read_until(&mut stream, &mut peer, |_| true);
    assert!(
        matches!(&message, Message::Reject(reject) if reject.reason == RejectReason::ServerBusy),
        "{message:?}"
    );
    stopped(move || listening.shutdown());
}

#[test]
fn a_client_that_sends_no_hello_is_closed_at_the_handshake_deadline() {
    let deadline = Duration::from_millis(200);
    let server = Arc::new(server(ServerConfig {
        handshake_deadline: deadline,
        ..ServerConfig::default()
    }));
    let listening = SocketServer::start(server);
    let started = Instant::now();
    let mut silent = huterm_socket::connect(listening.socket()).unwrap();
    // The server sends its preamble, then closes once the deadline passes
    // on its own clock, the wall clock this adapter runs on.
    assert_eq!(read_to_end(&mut silent), preamble().to_vec());
    assert!(started.elapsed() >= deadline, "{:?}", started.elapsed());
    wait_until("the silent connection's slot frees", || {
        listening.listener().connections() == 0
    });
    stopped(move || listening.shutdown());
}

#[test]
fn shutting_down_closes_every_connection_and_removes_the_socket() {
    let server = Arc::new(server(ServerConfig::default()));
    let listening = SocketServer::start(Arc::clone(&server));
    let path = listening.socket().path().to_path_buf();
    assert_eq!(listening.listener().path(), Some(path.as_path()));
    let (mut client, _) = handshake(listening.socket(), &server);
    stopped(move || listening.shutdown());
    assert_eq!(read_to_end(&mut client), Vec::<u8>::new());
    assert!(!path.exists());
    let _ = server.lock().shutdown();
}

#[test]
fn frames_split_at_every_byte_decode_over_a_bound_socket() {
    let server = Arc::new(server(ServerConfig::default()));
    let directory = TestDirectory::new();
    let socket = directory.socket("test");
    let (listener, _file) = huterm_socket::bind(&socket).unwrap();
    let session = SessionId::in_runtime(server.runtime(), 999_999);
    let mut bytes = TestPeer::new(server.runtime()).opening();
    let (id, request) = TestPeer::new(server.runtime())
        .request(Request::SelectSession { session });
    bytes.extend(&request);
    for split in 1..bytes.len() {
        let mut client = huterm_socket::connect(&socket).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        let handle = serve(Arc::clone(&server), accepted).unwrap();
        // The server reads the first part before the rest exists.
        client.write_all(&bytes[..split]).unwrap();
        wait_until("the server reads the first part", || {
            handle.received() == split
        });
        client.write_all(&bytes[split..]).unwrap();
        let mut peer = TestPeer::new(server.runtime());
        read_until(&mut client, &mut peer, |message| {
            matches!(message, Message::Welcome(_))
        });
        read_until(
            &mut client,
            &mut peer,
            |message| matches!(message, Message::Result(result) if result.request == id),
        );
        drop(client);
        stopped(move || handle.join());
    }
}

#[test]
fn a_message_larger_than_the_socket_buffers_arrives_whole_through_partial_writes()
 {
    use nix::sys::socket::{getsockopt, sockopt};

    const SESSIONS: usize = 2_500;

    let server = Arc::new(server(ServerConfig::default()));
    {
        let mut mux = server.lock();
        let name = "n".repeat(256);
        // Session creation costs time in proportion to the hierarchy, so
        // this stays near the fewest that outgrow the buffers.
        for _ in 0..SESSIONS {
            mux.create_session(Some(&name)).unwrap();
        }
    }
    let listening = SocketServer::start(Arc::clone(&server));
    let (mut client, mut peer) = handshake(listening.socket(), &server);
    let (_, subscribe) = peer.request(Request::SubscribeHierarchy);
    client.write_all(&subscribe).unwrap();
    // An odd chunk size splits frames at shifting offsets.
    let mut buffer = [0; 61];
    let mut read = 0;
    let snapshot = loop {
        let count = client.read(&mut buffer).expect("the server's bytes");
        assert!(count > 0, "the server closed before answering");
        read += count;
        if let Some(Message::HierarchySnapshot(snapshot)) = peer
            .receive(&buffer[..count])
            .into_iter()
            .find(|message| matches!(message, Message::HierarchySnapshot(_)))
        {
            break snapshot;
        }
    };
    assert_eq!(snapshot.state.digest(), snapshot.digest);
    assert_eq!(snapshot.state.sessions().len(), SESSIONS);
    // The connection's buffers hold less than the snapshot, so the
    // server's writes were partial. Each end's buffers have the same
    // defaults, and Linux counts only the sender's.
    let buffered = getsockopt(&client, sockopt::SndBuf).unwrap()
        + getsockopt(&client, sockopt::RcvBuf).unwrap();
    assert!(read > buffered, "{read} <= {buffered}");
    drop(client);
    stopped(move || listening.shutdown());
    let _ = server.lock().shutdown();
}

/// Where the second-user test's child process connects.
const PEER_SOCKET_VARIABLE: &str = "HUTERM_TEST_PEER_SOCKET";

/// The user the child process runs as: `nobody` on Debian and Ubuntu.
const SECOND_USER: u32 = 65_534;

/// Runs as root in the Linux test container, where it can start a process
/// as a second user. See the core runtime guide for the command.
#[test]
#[ignore = "needs root and a second user; run in the Linux Docker image"]
fn a_second_user_is_closed_before_the_preamble() {
    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;

    assert_eq!(
        huterm_socket::effective_uid(),
        0,
        "run as root so the test can start a process as another user"
    );
    let server = Arc::new(server(ServerConfig::default()));
    // A socket the second user can reach, so only the peer check stops it.
    let directory = TestDirectory::new();
    fs::set_permissions(directory.path(), Permissions::from_mode(0o711))
        .unwrap();
    let path = directory.path().join("shared");
    let bound = UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, Permissions::from_mode(0o777)).unwrap();
    let listener = Listener::from_listener(Arc::clone(&server), bound).unwrap();

    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "listener::tests::second_user_peer",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(PEER_SOCKET_VARIABLE, &path)
        .uid(SECOND_USER)
        .gid(SECOND_USER)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "the second user's peer failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(listener.refused_peers(), 1);

    // The listener's own user is served.
    let mut stream = UnixStream::connect(&path).unwrap();
    let mut peer = TestPeer::new(server.runtime());
    stream.write_all(&peer.opening()).unwrap();
    read_until(&mut stream, &mut peer, |message| {
        matches!(message, Message::Welcome(_))
    });
    drop(stream);
    stopped(move || listener.shutdown());
}

/// The second user's side of the test above: the server closes its
/// connection without writing a byte.
#[test]
#[ignore = "the child process of a_second_user_is_closed_before_the_preamble"]
fn second_user_peer() {
    let path = std::env::var_os(PEER_SOCKET_VARIABLE)
        .expect("started by a_second_user_is_closed_before_the_preamble");
    assert_ne!(huterm_socket::effective_uid(), 0);
    let mut stream = UnixStream::connect(path).unwrap();
    let _ = stream.write_all(&preamble());
    assert_eq!(read_to_end(&mut stream), Vec::<u8>::new());
}
