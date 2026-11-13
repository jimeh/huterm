//! Release benchmark of runtime IPC requests and hierarchy events over the
//! native adapters on a `UnixStream` pair: a structural request's round
//! trip, the latency from a commit to the client's projection, and the
//! same operation in process for context. Run it through
//! `mise run bench:ipc`.
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
