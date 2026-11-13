//! Release benchmark of runtime IPC requests and hierarchy events over the
//! native adapters on a `UnixStream` pair: a structural request's round
//! trip, the latency from a commit to the client's projection, and the
//! same operation in process for context. Run it through
//! `mise run bench:ipc`.
//!
//! The viewer benchmark measures a keystroke's echo through a real PTY over
//! IPC and in process, what IPC adds to output on its way to a client, and
//! a 100,000-line flood until a viewer shows its end.
//!
//! It reports medians and 99th percentiles and gates nothing: no CI
//! runner has measured it yet.

use std::future::Future;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use huterm_client::native::{Client, ClientHandle};
use huterm_client::{ClientConfig, ClientEvent, HierarchyUpdate};
use huterm_core::Mux;
use huterm_protocol::TabId;
use huterm_wire::handshake::ClientKind;
use huterm_wire::request::Request;
use huterm_wire::response::Response;

use crate::native::serve;
use crate::test_support::{quiet_command, server};
use crate::{Server, ServerConfig};

/// Samples per measurement, after as many again to warm up.
const SAMPLES: usize = 5_000;

/// How long one step may take before the benchmark gives up.
const PROMPT: Duration = Duration::from_secs(10);

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

/// Sorted nanosecond samples.
struct Samples(Vec<u128>);

impl Samples {
    fn new(mut samples: Vec<u128>) -> Self {
        samples.sort_unstable();
        Self(samples)
    }

    fn percentile(&self, percent: usize) -> f64 {
        let index = (self.0.len() * percent / 100).min(self.0.len() - 1);
        // Nanoseconds to microseconds, for the report.
        #[expect(clippy::cast_precision_loss, reason = "a report figure")]
        let micros = self.0[index] as f64 / 1000.0;
        micros
    }

    fn report(&self, op: &str) {
        println!(
            "ipc op={op} samples={} p50_us={:.2} p99_us={:.2}",
            self.0.len(),
            self.percentile(50),
            self.percentile(99),
        );
    }
}

/// Runs `step` [`SAMPLES`] times to warm up, then times as many runs.
fn measure(mut step: impl FnMut(usize) -> Duration) -> Samples {
    for index in 0..SAMPLES {
        step(index);
    }
    Samples::new(
        (0..SAMPLES)
            .map(|index| step(SAMPLES + index).as_nanos())
            .collect(),
    )
}

/// A name that differs from the previous step's, so every rename commits.
fn name(index: usize) -> &'static str {
    if index.is_multiple_of(2) {
        "even"
    } else {
        "odd"
    }
}

/// A served client subscribed to the hierarchy, and a tab it opened, in
/// the client's scope.
struct Pair {
    server: Arc<Server>,
    client: Client,
    handle: ClientHandle,
    tab: TabId,
}

impl Pair {
    fn new() -> Self {
        let server = Arc::new(server(ServerConfig::default()));
        let (stream, accepted) = UnixStream::pair().unwrap();
        // The connection ends when the client shuts down.
        let _connection = serve(Arc::clone(&server), accepted).unwrap();
        let client = Client::spawn(
            ClientConfig::local(ClientKind::Desktop, "benchmark", "test"),
            stream,
        )
        .unwrap();
        let handle = client.handle();
        block_on(handle.connected()).unwrap();
        block_on(handle.request(Request::SubscribeHierarchy)).unwrap();
        let Response::Session(session) =
            block_on(handle.request(Request::CreateSession { name: None }))
                .unwrap()
                .response
        else {
            panic!("a session");
        };
        let Response::Workspace(workspace) =
            block_on(handle.request(Request::CreateWorkspace {
                session,
                name: None,
            }))
            .unwrap()
            .response
        else {
            panic!("a workspace");
        };
        let Response::OpenedTab(opened) =
            block_on(handle.request(Request::OpenTab {
                workspace,
                command: Box::new(quiet_command()),
            }))
            .unwrap()
            .response
        else {
            panic!("a tab");
        };
        Self {
            server,
            client,
            handle,
            tab: opened.tab,
        }
    }

    fn shutdown(self) {
        self.client.shutdown().unwrap();
        self.server.lock().shutdown().unwrap();
    }
}

/// A `RenameTab` from the client's call until its result arrives.
fn round_trip(pair: &Pair) -> Samples {
    measure(|index| {
        let request = Request::RenameTab {
            tab: pair.tab,
            name: Some(name(index).to_owned()),
        };
        let started = Instant::now();
        let result = block_on(pair.handle.request(request));
        let elapsed = started.elapsed();
        assert!(result.unwrap().committed.is_some());
        elapsed
    })
}

/// A rename committed straight on the server's `Mux` until the client's
/// projection applies its event.
fn event_latency(pair: &Pair) -> Samples {
    let events = block_on(pair.handle.events()).unwrap();
    let tab = TabId::in_runtime(pair.server.runtime(), pair.tab.get());
    measure(|index| {
        let (seq, started) = {
            let mut mux = pair.server.lock();
            mux.rename_tab(tab, Some(name(index))).unwrap();
            (mux.hierarchy_seq(), Instant::now())
        };
        loop {
            if let ClientEvent::Hierarchy(HierarchyUpdate::Applied {
                seq: applied,
                ..
            }) = block_on(events.recv()).unwrap()
                && applied == seq
            {
                return started.elapsed();
            }
        }
    })
}

/// The same rename in process, through the lock embedded mode shares.
fn in_process() -> Samples {
    let mux = Mutex::new(Mux::default());
    let tab = {
        let mut mux = mux.lock().unwrap();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        mux.open_tab(workspace, &quiet_command()).unwrap().tab.id
    };
    let samples = measure(|index| {
        let started = Instant::now();
        let mut locked = mux.lock().unwrap();
        locked.rename_tab(tab, Some(name(index))).unwrap();
        let _ = locked.hierarchy_seq();
        drop(locked);
        started.elapsed()
    });
    mux.lock().unwrap().shutdown().unwrap();
    samples
}

#[test]
#[ignore = "release benchmark; run through mise run bench:ipc"]
fn ipc_benchmark() {
    let pair = Pair::new();
    round_trip(&pair).report("rename_round_trip");
    event_latency(&pair).report("rename_event_to_projection");
    pair.shutdown();
    in_process().report("rename_in_process");
}

/// Samples of each viewer latency, after as many again to warm up: each
/// one is a keystroke through a real PTY.
const VIEWER_SAMPLES: usize = 2_000;

/// Runs `step` [`VIEWER_SAMPLES`] times to warm up, then times as many.
fn measure_viewer(mut step: impl FnMut(usize) -> Duration) -> Samples {
    for index in 0..VIEWER_SAMPLES {
        step(index);
    }
    Samples::new(
        (0..VIEWER_SAMPLES)
            .map(|index| step(VIEWER_SAMPLES + index).as_nanos())
            .collect(),
    )
}

/// A terminal that echoes each byte as it arrives, with no line buffering.
fn echo() -> huterm_protocol::TerminalCommand {
    crate::test_support::command("stty raw -echo; exec cat")
}

/// A viewer's snapshot as the desktop reads it: the reply awaited, not
/// `read_snapshot`'s polling, which parks a millisecond at a time.
fn snapshot(
    viewer: &huterm_core::TerminalViewer,
) -> std::sync::Arc<huterm_protocol::TerminalSnapshot> {
    block_on(viewer.request_snapshot().unwrap().recv())
        .unwrap()
        .snapshot
}

/// What changes when an echo lands: the content generation. The cursor
/// alone would miss an echo at the last column, which defers its wrap.
fn generation(view: &huterm_protocol::TerminalSnapshot) -> u64 {
    view.generation
}

fn key(index: usize) -> huterm_protocol::TerminalInput {
    let letter = char::from(b'a' + u8::try_from(index % 26).unwrap_or(0));
    huterm_protocol::TerminalInput::Text(letter.to_string())
}

/// Waits for the next update of `viewer` whose content generation moved
/// from `before`, returning when that update's event arrived.
fn echoed(
    remote: &crate::viewer_tests::Remote,
    viewer: huterm_protocol::ViewerId,
    before: Option<u64>,
) -> (Instant, Option<u64>) {
    loop {
        let event = block_on(remote.events.recv()).unwrap();
        let arrived = Instant::now();
        if let ClientEvent::Viewer(huterm_client::ViewerEvent::Updated {
            viewer: updated,
            ..
        }) = event
            && updated == viewer
        {
            let now = block_on(remote.handle.view(viewer))
                .map(|view| generation(&view));
            if now != before {
                return (arrived, now);
            }
        }
    }
}

/// A keystroke from the client's send until the update that shows its
/// echo arrives at the client: input to the PTY, the echo's output, and
/// its update back.
fn keystroke_over_ipc() -> Samples {
    let (server, session, terminal) =
        crate::viewer_tests::served(ServerConfig::default(), &echo());
    let remote =
        crate::viewer_tests::Remote::connect(&server, session, terminal);
    let viewer = remote.subscribe(crate::viewer_tests::writable());
    let mut before = Some(generation(&remote.view_until(viewer, |_| true)));
    let samples = measure_viewer(|index| {
        let started = Instant::now();
        remote.handle.send_input(viewer, key(index), None);
        let (arrived, now) = echoed(&remote, viewer, before);
        before = now;
        arrived - started
    });
    let _ = server.lock().shutdown();
    remote.shutdown();
    samples
}

/// The same keystroke in process: a viewer's input until the snapshot that
/// shows its echo, as the desktop reads it.
fn keystroke_in_process() -> Samples {
    let (server, session, terminal) =
        crate::viewer_tests::served(ServerConfig::default(), &echo());
    let viewer = {
        let mut mux = server.lock();
        let runtime = mux.runtime_id();
        let attachment = mux
            .attach_session(huterm_protocol::SessionId::in_runtime(
                runtime, session,
            ))
            .unwrap();
        mux.subscribe_terminal(
            attachment,
            huterm_protocol::TerminalId::in_runtime(runtime, terminal),
            crate::convert::viewer_options(crate::viewer_tests::writable()),
        )
        .unwrap()
    };
    let mut before = Some(generation(&snapshot(&viewer)));
    let samples = measure_viewer(|index| {
        let started = Instant::now();
        viewer.send_input(key(index)).unwrap();
        loop {
            block_on(viewer.wait()).unwrap();
            if viewer.poll().invalidated.is_none() {
                continue;
            }
            let now = Some(generation(&snapshot(&viewer)));
            if now != before {
                before = now;
                return started.elapsed();
            }
        }
    });
    drop(viewer);
    let _ = server.lock().shutdown();
    samples
}

/// An echo's output from the moment an in-process viewer of the same
/// terminal sees it until the remote client's update that shows it
/// arrives: what IPC adds to output.
fn output_to_client() -> Samples {
    let (server, session, terminal) =
        crate::viewer_tests::served(ServerConfig::default(), &echo());
    let remote =
        crate::viewer_tests::Remote::connect(&server, session, terminal);
    let remote_viewer = remote.subscribe(crate::viewer_tests::writable());
    let local = {
        let mut mux = server.lock();
        let runtime = mux.runtime_id();
        let attachment = mux
            .attach_session(huterm_protocol::SessionId::in_runtime(
                runtime, session,
            ))
            .unwrap();
        mux.subscribe_terminal(
            attachment,
            huterm_protocol::TerminalId::in_runtime(runtime, terminal),
            crate::convert::viewer_options(crate::viewer_tests::writable()),
        )
        .unwrap()
    };
    let mut before =
        Some(generation(&remote.view_until(remote_viewer, |_| true)));
    let samples = measure_viewer(|index| {
        local.send_input(key(index)).unwrap();
        let seen = loop {
            block_on(local.wait()).unwrap();
            if local.poll().invalidated.is_none() {
                continue;
            }
            if Some(generation(&snapshot(&local))) != before {
                break Instant::now();
            }
        };
        let (arrived, now) = echoed(&remote, remote_viewer, before);
        before = now;
        arrived.saturating_duration_since(seen)
    });
    drop(local);
    let _ = server.lock().shutdown();
    remote.shutdown();
    samples
}

/// Runs of the output throughput measurement.
const THROUGHPUT_RUNS: usize = 5;

/// A terminal that prints 100,000 numbered lines once it reads a line,
/// then the marker `DONE`.
fn flood() -> huterm_protocol::TerminalCommand {
    crate::test_support::command(
        "head -n 1 >/dev/null; seq 1 100000; printf 'DONE\\n'; exec cat",
    )
}

fn done(view: &huterm_protocol::TerminalSnapshot) -> bool {
    crate::viewer_tests::screen_text(view)
        .iter()
        .any(|line| line == "DONE")
}

/// From the line that starts a 100,000-line flood until a viewer shows its
/// end, over IPC and in process.
fn throughput() -> (Samples, Samples) {
    let mut ipc = Vec::new();
    let mut local = Vec::new();
    for _ in 0..THROUGHPUT_RUNS {
        let (server, session, terminal) =
            crate::viewer_tests::served(ServerConfig::default(), &flood());
        let remote =
            crate::viewer_tests::Remote::connect(&server, session, terminal);
        let viewer = remote.subscribe(crate::viewer_tests::writable());
        remote.view_until(viewer, |_| true);
        let started = Instant::now();
        remote.type_text(viewer, "go\r");
        loop {
            if let ClientEvent::Viewer(huterm_client::ViewerEvent::Updated {
                ..
            }) = block_on(remote.events.recv()).unwrap()
                && block_on(remote.handle.view(viewer))
                    .is_some_and(|view| done(&view))
            {
                ipc.push(started.elapsed().as_nanos());
                break;
            }
        }
        let _ = server.lock().shutdown();
        remote.shutdown();

        let (server, session, terminal) =
            crate::viewer_tests::served(ServerConfig::default(), &flood());
        let viewer = {
            let mut mux = server.lock();
            let runtime = mux.runtime_id();
            let attachment = mux
                .attach_session(huterm_protocol::SessionId::in_runtime(
                    runtime, session,
                ))
                .unwrap();
            mux.subscribe_terminal(
                attachment,
                huterm_protocol::TerminalId::in_runtime(runtime, terminal),
                crate::convert::viewer_options(crate::viewer_tests::writable()),
            )
            .unwrap()
        };
        let _ = snapshot(&viewer);
        let started = Instant::now();
        viewer
            .send_input(huterm_protocol::TerminalInput::Text("go\r".into()))
            .unwrap();
        loop {
            block_on(viewer.wait()).unwrap();
            if viewer.poll().invalidated.is_some() && done(&snapshot(&viewer)) {
                local.push(started.elapsed().as_nanos());
                break;
            }
        }
        drop(viewer);
        let _ = server.lock().shutdown();
    }
    (Samples::new(ipc), Samples::new(local))
}

#[test]
#[ignore = "release benchmark; run through mise run bench:ipc"]
fn ipc_viewer_benchmark() {
    keystroke_over_ipc().report("keystroke_echo_over_ipc");
    keystroke_in_process().report("keystroke_echo_in_process");
    output_to_client().report("output_to_client_over_ipc");
    let (ipc, local) = throughput();
    ipc.report("flood_100k_lines_over_ipc");
    local.report("flood_100k_lines_in_process");
}
