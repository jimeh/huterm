//! Descriptor-limit behavior across the whole process.
//!
//! `RLIMIT_NOFILE`, the descriptor table, and the recorded original limit are
//! process-wide, so these tests run in their own binary. Setup lowers the
//! soft limit to 256 and raises it once. Each test holds one mutex and
//! restores any temporary limit and filler descriptors before releasing it.
#![cfg(unix)]

use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, Once, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use huterm_core::{RuntimeClient, TerminalRuntime};
use huterm_protocol::{
    CellSize, GridSize, TerminalCommand, TerminalId, TerminalInput,
    TerminalPresentation,
};
use nix::sys::resource::{Resource, getrlimit, rlim_t, setrlimit};

const ORIGINAL_SOFT_LIMIT: rlim_t = 256;
const REQUIRED_LIMIT: rlim_t = 4096;
const TERMINAL_COUNT: u64 = 200;
/// `select(2)` cannot wait on descriptors numbered at or above this.
const FD_SETSIZE: i32 = 1024;
const DEADLINE: Duration = Duration::from_secs(10);

static SETUP: Once = Once::new();
static SERIAL: Mutex<()> = Mutex::new(());

/// Lowers only the soft limit, raises it once, and serializes the caller.
fn serialized() -> MutexGuard<'static, ()> {
    SETUP.call_once(|| {
        let (_, hard) = getrlimit(Resource::RLIMIT_NOFILE).unwrap();
        assert!(
            hard >= REQUIRED_LIMIT,
            "hard descriptor limit {hard} is below the {REQUIRED_LIMIT} these \
             tests need; raise it with `ulimit -Hn`"
        );
        setrlimit(Resource::RLIMIT_NOFILE, ORIGINAL_SOFT_LIMIT, hard).unwrap();
        huterm_core::raise_open_file_limit();
        let (soft, _) = getrlimit(Resource::RLIMIT_NOFILE).unwrap();
        assert!(
            soft >= REQUIRED_LIMIT,
            "raised soft descriptor limit {soft} is below {REQUIRED_LIMIT}"
        );
    });
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Restores the raised soft limit when dropped, including after a panic.
struct SoftLimitRestore((rlim_t, rlim_t));

impl SoftLimitRestore {
    fn capture() -> Self {
        Self(getrlimit(Resource::RLIMIT_NOFILE).unwrap())
    }
}

impl Drop for SoftLimitRestore {
    fn drop(&mut self) {
        let (soft, hard) = self.0;
        setrlimit(Resource::RLIMIT_NOFILE, soft, hard).unwrap();
    }
}

fn command(script: &str) -> TerminalCommand {
    TerminalCommand {
        program: PathBuf::from("/bin/sh"),
        arguments: vec!["-c".into(), script.into()],
        working_directory: std::env::current_dir().unwrap(),
        environment: Vec::new(),
        grid_size: GridSize::clamped(40, 8),
        cell_size: CellSize {
            width: 8,
            height: 16,
        },
        presentation: TerminalPresentation::default(),
    }
}

fn wait_for_text(client: &RuntimeClient, needle: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let snapshot = client.read_snapshot().expect("snapshot should work");
        let text: String =
            snapshot.cells().map(|cell| cell.text.as_str()).collect();
        if text.contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "terminal output did not contain {needle:?}: {text:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

/// Sends a line to a shell reading input and waits for its reply.
fn round_trip(client: &RuntimeClient, line: &str) {
    client
        .send_input(TerminalInput::Text(format!("{line}\n")))
        .unwrap();
    wait_for_text(client, &format!("ECHO:{line}:END"));
}

/// Shuts down on a worker thread so a hung shutdown fails the test.
fn shutdown_within(runtime: TerminalRuntime, limit: Duration) {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(runtime.shutdown());
    });
    finished
        .recv_timeout(limit)
        .expect("terminal shutdown did not finish")
        .unwrap();
}

/// Opens `/dev/null` until the lowest free descriptor exceeds `number`.
fn fill_through(number: i32) -> Vec<File> {
    let mut fillers = Vec::new();
    loop {
        let filler = File::open("/dev/null").unwrap();
        let above = filler.as_raw_fd() > number;
        fillers.push(filler);
        if above {
            return fillers;
        }
    }
}

fn highest_open_descriptor() -> i32 {
    std::fs::read_dir("/dev/fd")
        .unwrap()
        .filter_map(|entry| {
            entry.ok()?.file_name().into_string().ok()?.parse().ok()
        })
        .max()
        .unwrap()
}

const ECHO_SCRIPT: &str = "printf READY; \
    while IFS= read -r line; do \
        if [ \"$line\" = burst ]; then \
            head -c 262144 /dev/zero | tr '\\0' x; printf '\\nBURST:END'; \
        else printf 'ECHO:%s:END' \"$line\"; fi; \
    done";

#[test]
fn terminal_above_descriptor_1024_round_trips_and_closes() {
    let _serial = serialized();
    let fillers = fill_through(FD_SETSIZE);
    let runtime =
        TerminalRuntime::spawn(TerminalId::new(1), &command(ECHO_SCRIPT))
            .unwrap();
    let client = runtime.client();
    wait_for_text(&client, "READY");
    round_trip(&client, "ping");
    client
        .send_input(TerminalInput::Text("burst\n".into()))
        .unwrap();
    wait_for_text(&client, "BURST:END");
    round_trip(&client, "after");
    shutdown_within(runtime, Duration::from_secs(5));
    drop(fillers);
}

#[test]
fn two_hundred_terminals_start_from_a_soft_limit_of_256() {
    let _serial = serialized();
    let runtimes: Vec<_> = (0..TERMINAL_COUNT)
        .map(|index| {
            TerminalRuntime::spawn(
                TerminalId::new(100 + index),
                &command(ECHO_SCRIPT),
            )
            .unwrap_or_else(|error| {
                panic!("terminal {index} failed to start: {error}")
            })
        })
        .collect();
    for (index, runtime) in runtimes.iter().enumerate() {
        let client = runtime.client();
        wait_for_text(&client, "READY");
        round_trip(&client, &format!("t{index}"));
    }
    for runtime in &runtimes {
        runtime.client().close().unwrap();
    }
    for runtime in runtimes {
        shutdown_within(runtime, Duration::from_secs(5));
    }
}

#[test]
fn children_start_with_the_original_soft_limit() {
    let _serial = serialized();
    let runtime = TerminalRuntime::spawn(
        TerminalId::new(2),
        &command("printf 'LIMIT:%s:END' \"$(ulimit -Sn)\"; read -r _"),
    )
    .unwrap();
    wait_for_text(
        &runtime.client(),
        &format!("LIMIT:{ORIGINAL_SOFT_LIMIT}:END"),
    );
    shutdown_within(runtime, Duration::from_secs(5));
}

#[test]
fn descriptor_exhaustion_fails_terminal_creation() {
    let _serial = serialized();
    let restore = SoftLimitRestore::capture();
    // Fill every gap first, so only the descriptors above the highest one in
    // use remain.
    let fillers = fill_through(highest_open_descriptor());
    let highest =
        rlim_t::try_from(fillers.last().unwrap().as_raw_fd()).unwrap();
    setrlimit(Resource::RLIMIT_NOFILE, highest + 4, restore.0.1).unwrap();
    let result =
        TerminalRuntime::spawn(TerminalId::new(3), &command("read -r _"));
    drop(restore);
    drop(fillers);
    match result {
        Err(error) => assert!(!error.to_string().is_empty()),
        Ok(runtime) => {
            shutdown_within(runtime, Duration::from_secs(5));
            panic!("terminal started with only three free descriptors");
        }
    }
}
