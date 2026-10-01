use super::*;
use std::collections::VecDeque;
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::sync::mpsc::{self, Receiver, Sender};

#[cfg(unix)]
#[test]
fn readiness_cancellation_survives_notification_before_or_during_wait() {
    for cancel_first in [true, false] {
        // Use a quiet stream as the data descriptor so only cancellation
        // can release the readiness wait.
        let (data, _peer) = UnixStream::pair().unwrap();
        let (cancellation, cancel_stream) = UnixStream::pair().unwrap();
        let cancel = IoCancellation {
            stream: Arc::new(cancel_stream),
        };
        let waiter = ReadinessWaiter {
            interest: Readiness::Read,
            fd: FileDescriptor::dup(&MasterDescriptor(data.as_raw_fd()))
                .unwrap(),
            cancellation,
            cancel: cancel.clone(),
        };
        if cancel_first {
            cancel.cancel();
        }
        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            started_tx.send(()).unwrap();
            done_tx.send(waiter.wait(None)).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        cancel.cancel();
        let outcome = done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("cancellation did not release readiness wait")
            .unwrap();
        assert_eq!(outcome, ReadinessOutcome::Cancelled);
        worker.join().unwrap();
    }
}

#[cfg(unix)]
#[test]
fn poll_timeout_rounds_up_to_milliseconds_and_clamps_its_range() {
    use nix::poll::PollTimeout;

    assert_eq!(poll_timeout(None), PollTimeout::NONE);
    assert_eq!(poll_timeout(Some(Duration::ZERO)), PollTimeout::ZERO);
    assert_eq!(
        poll_timeout(Some(Duration::from_micros(1))),
        PollTimeout::from(1_u8)
    );
    assert_eq!(poll_timeout(Some(Duration::MAX)), PollTimeout::MAX);
}

/// `select(2)` cannot wait on descriptors numbered at or above this.
#[cfg(unix)]
const FD_SETSIZE: RawFileDescriptor = 1024;

#[cfg(unix)]
const WAIT_DEADLINE: Duration = Duration::from_secs(3);

/// A PTY pair whose slave is in raw mode: a canonical-mode master can
/// accept megabytes of input without reporting `WouldBlock`.
#[cfg(unix)]
struct RawPty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    slave: std::fs::File,
}

#[cfg(unix)]
impl RawPty {
    fn open() -> Self {
        use nix::fcntl::OFlag;
        use nix::sys::termios::{SetArg, cfmakeraw, tcgetattr, tcsetattr};
        use std::os::unix::fs::OpenOptionsExt;

        let pair = portable_pty::native_pty_system()
            .openpty(PtySize {
                rows: 8,
                cols: 40,
                pixel_width: 320,
                pixel_height: 128,
            })
            .unwrap();
        set_nonblocking(pair.master.as_ref()).unwrap();
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags((OFlag::O_NOCTTY | OFlag::O_NONBLOCK).bits())
            .open(pair.master.tty_name().expect("PTY has no slave path"))
            .unwrap();
        drop(pair.slave);
        let mut termios = tcgetattr(&slave).unwrap();
        cfmakeraw(&mut termios);
        tcsetattr(&slave, SetArg::TCSANOW, &termios).unwrap();
        let writer = clone_writer(pair.master.as_ref()).unwrap();
        Self {
            master: pair.master,
            writer,
            slave,
        }
    }

    /// Writes to the master until it reports `WouldBlock`.
    fn fill(&mut self) {
        let chunk = [b'x'; 4096];
        for _ in 0..4096 {
            match self.writer.write(&chunk) {
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    return;
                }
                Err(error) => panic!("PTY fill failed: {error}"),
            }
        }
        panic!("PTY master accepted 16 MiB without WouldBlock");
    }

    /// Fills the master until it stays full. Linux moves PTY input into
    /// the slave's line discipline on a kernel worker, which can free
    /// room after `WouldBlock`. No event reports that the worker has
    /// finished, so require a short write wait to time out.
    fn fill_until_settled(&mut self, waiter: &ReadinessWaiter) {
        let deadline = Instant::now() + WAIT_DEADLINE;
        loop {
            self.fill();
            match waiter.wait(Some(Duration::from_millis(50))).unwrap() {
                ReadinessOutcome::TimedOut => return,
                ReadinessOutcome::Ready => assert!(
                    Instant::now() < deadline,
                    "PTY master kept accepting input"
                ),
                ReadinessOutcome::Cancelled => {
                    panic!("fill observed an unexpected cancellation")
                }
            }
        }
    }

    /// Reads everything the slave currently has queued.
    fn drain(&mut self) {
        let mut buffer = [0_u8; 4096];
        for _ in 0..4096 {
            match self.slave.read(&mut buffer) {
                Ok(0) => panic!("PTY slave reached end-of-file"),
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    return;
                }
                Err(error) => panic!("PTY drain failed: {error}"),
            }
        }
        panic!("PTY slave returned 16 MiB without WouldBlock");
    }

    /// Creates a waiter whose descriptors are all above `FD_SETSIZE`.
    fn high_descriptor_waiter(&self, interest: Readiness) -> ReadinessWaiter {
        raise_descriptor_limit();
        let mut fillers = Vec::new();
        let mut rejected = Vec::new();
        for _ in 0..64 {
            // Take every free number up to FD_SETSIZE so the waiter's
            // descriptors are allocated above it.
            loop {
                let filler = std::fs::File::open("/dev/null").unwrap();
                let high = filler.as_raw_fd() > FD_SETSIZE;
                fillers.push(filler);
                assert!(
                    fillers.len() < 4 * FD_SETSIZE as usize,
                    "descriptor table did not reach {FD_SETSIZE}"
                );
                if high {
                    break;
                }
            }
            let waiter =
                ReadinessWaiter::new(self.master.as_ref(), interest).unwrap();
            let descriptors = [
                waiter.fd.as_raw_file_descriptor(),
                waiter.cancellation.as_raw_fd(),
                waiter.cancel.stream.as_raw_fd(),
            ];
            if descriptors.iter().all(|fd| *fd > FD_SETSIZE) {
                return waiter;
            }
            // A parallel test freed a low number; keep it occupied.
            rejected.push(waiter);
        }
        panic!("could not allocate waiter descriptors above {FD_SETSIZE}");
    }
}

/// Only ever raises the soft limit, so parallel unit tests keep every
/// descriptor they could open before. Tests that lower the limit or
/// exhaust the table run in `tests/descriptor_limits.rs` instead.
#[cfg(unix)]
fn raise_descriptor_limit() {
    use nix::sys::resource::{Resource, getrlimit, setrlimit};

    static RAISE: std::sync::Once = std::sync::Once::new();
    RAISE.call_once(|| {
        let target = 4 * u64::try_from(FD_SETSIZE).unwrap();
        let (soft, hard) = getrlimit(Resource::RLIMIT_NOFILE).unwrap();
        assert!(
            hard >= target,
            "hard descriptor limit {hard} is below {target}"
        );
        if soft < target {
            setrlimit(Resource::RLIMIT_NOFILE, target, hard).unwrap();
        }
    });
}

/// Waits without a timeout on a worker thread. The worker signals just
/// before calling `wait`, so callers change readiness only after the
/// returned receiver exists.
#[cfg(unix)]
fn start_wait(
    waiter: ReadinessWaiter,
) -> (
    Receiver<std::io::Result<ReadinessOutcome>>,
    thread::JoinHandle<()>,
) {
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        started_tx.send(()).unwrap();
        let _ = done_tx.send(waiter.wait(None));
    });
    started_rx
        .recv_timeout(WAIT_DEADLINE)
        .expect("readiness worker did not start");
    (done_rx, worker)
}

#[cfg(unix)]
#[test]
fn write_readiness_above_fd_setsize_waits_for_the_slave_to_drain() {
    let mut pty = RawPty::open();
    let waiter = pty.high_descriptor_waiter(Readiness::Write);
    pty.fill_until_settled(&waiter);
    assert_eq!(
        waiter.wait(Some(Duration::ZERO)).unwrap(),
        ReadinessOutcome::TimedOut
    );

    let (done, worker) = start_wait(waiter);
    let deadline = Instant::now() + WAIT_DEADLINE;
    let outcome = loop {
        pty.drain();
        match done.recv_timeout(Duration::from_millis(10)) {
            Ok(outcome) => break outcome.unwrap(),
            Err(mpsc::RecvTimeoutError::Timeout) => assert!(
                Instant::now() < deadline,
                "draining the slave did not release the write wait"
            ),
            Err(error) => panic!("readiness worker failed: {error}"),
        }
    };
    assert_eq!(outcome, ReadinessOutcome::Ready);
    worker.join().unwrap();
}

#[cfg(unix)]
#[test]
fn cancellation_above_fd_setsize_releases_a_full_write_wait() {
    for cancel_first in [true, false] {
        let mut pty = RawPty::open();
        let waiter = pty.high_descriptor_waiter(Readiness::Write);
        pty.fill_until_settled(&waiter);
        assert_eq!(
            waiter.wait(Some(Duration::ZERO)).unwrap(),
            ReadinessOutcome::TimedOut
        );

        let cancel = waiter.cancellation();
        if cancel_first {
            cancel.cancel();
        }
        let (done, worker) = start_wait(waiter);
        if !cancel_first {
            cancel.cancel();
        }
        let outcome = done
            .recv_timeout(WAIT_DEADLINE)
            .expect("cancellation did not release the write wait")
            .unwrap();
        assert_eq!(
            outcome,
            ReadinessOutcome::Cancelled,
            "cancel_first: {cancel_first}"
        );
        worker.join().unwrap();
    }
}

#[cfg(unix)]
#[test]
fn read_readiness_above_fd_setsize_waits_for_slave_output() {
    let mut pty = RawPty::open();
    let waiter = pty.high_descriptor_waiter(Readiness::Read);
    assert_eq!(
        waiter.wait(Some(Duration::ZERO)).unwrap(),
        ReadinessOutcome::TimedOut
    );

    let (done, worker) = start_wait(waiter);
    pty.slave.write_all(b"x").unwrap();
    let outcome = done
        .recv_timeout(WAIT_DEADLINE)
        .expect("slave output did not release the read wait")
        .unwrap();
    assert_eq!(outcome, ReadinessOutcome::Ready);
    worker.join().unwrap();
}

#[cfg(unix)]
#[test]
fn dropping_writer_does_not_inject_terminal_input() {
    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows: 8,
            cols: 40,
            pixel_width: 320,
            pixel_height: 128,
        })
        .unwrap();
    set_nonblocking(pair.master.as_ref()).unwrap();
    let reader_waiter =
        ReadinessWaiter::new(pair.master.as_ref(), Readiness::Read).unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let writer = clone_writer(pair.master.as_ref()).unwrap();
    let mut probe_writer = clone_writer(pair.master.as_ref()).unwrap();
    let mut command = CommandBuilder::new("/bin/sh");
    command.args([
        "-c",
        "printf READY; IFS= read -r line; printf ':%s' \"$line\"",
    ]);
    let child = pair.slave.spawn_command(command).unwrap();
    let mut killer = child.clone_killer();
    drop(pair.slave);

    let mut ready = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ready.ends_with(b"READY") {
        let mut output = [0_u8; 8];
        match reader.read(&mut output) {
            Ok(count) => ready.extend_from_slice(&output[..count]),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                reader_waiter.wait(Some(POLL_INTERVAL)).unwrap();
            }
            result => panic!("shell readiness failed: {result:?}"),
        }
        assert!(Instant::now() < deadline, "shell did not become ready");
    }

    drop(writer);
    probe_writer.write_all(b"PROBE\n").unwrap();
    probe_writer.flush().unwrap();

    let mut observed = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !observed
        .windows(b":PROBE".len())
        .any(|window| window == b":PROBE")
        && Instant::now() < deadline
    {
        let mut output = [0_u8; 32];
        match reader.read(&mut output) {
            Ok(0) => break,
            Ok(count) => observed.extend_from_slice(&output[..count]),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                reader_waiter.wait(Some(POLL_INTERVAL)).unwrap();
            }
            Err(_) => break,
        }
    }
    let _ = killer.kill();
    drop(probe_writer);
    drop(reader);
    drop(pair.master);
    assert!(reap_child(child));
    assert!(
        observed
            .windows(b":PROBE".len())
            .any(|window| window == b":PROBE"),
        "writer drop changed the shell's next input before the probe: {:?}",
        String::from_utf8_lossy(&observed)
    );
}

#[cfg(unix)]
#[test]
fn child_starts_with_the_configured_open_file_limit() {
    use nix::sys::resource::{Resource, getrlimit, rlim_t};

    // Below any usable inherited limit, so the test never raises limits.
    const CHILD_SOFT_LIMIT: rlim_t = 200;
    let (soft, hard) = getrlimit(Resource::RLIMIT_NOFILE).unwrap();
    assert!(
        soft > CHILD_SOFT_LIMIT,
        "soft descriptor limit {soft} is not above {CHILD_SOFT_LIMIT}"
    );
    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows: 8,
            cols: 40,
            pixel_width: 320,
            pixel_height: 128,
        })
        .unwrap();
    set_nonblocking(pair.master.as_ref()).unwrap();
    let reader_waiter =
        ReadinessWaiter::new(pair.master.as_ref(), Readiness::Read).unwrap();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut command = CommandBuilder::new("/bin/sh");
    command.args(["-c", "ulimit -Sn"]);
    command.nofile_limit(Some((CHILD_SOFT_LIMIT, hard)));
    let child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);

    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut buffer = [0_u8; 64];
        match reader.read(&mut buffer) {
            Ok(count) if count > 0 => {
                output.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "shell did not exit: {:?}",
                    String::from_utf8_lossy(&output)
                );
                reader_waiter.wait(Some(POLL_INTERVAL)).unwrap();
            }
            // End-of-file, or EIO once the child has closed the slave.
            _ => break,
        }
    }
    drop(reader);
    drop(pair.master);
    assert!(reap_child(child));
    assert_eq!(
        String::from_utf8_lossy(&output).trim(),
        CHILD_SOFT_LIMIT.to_string()
    );
}

#[derive(Debug)]
struct ControlledChild {
    exit: Mutex<Receiver<()>>,
    events: Sender<&'static str>,
    failures: VecDeque<ErrorKind>,
}

impl Drop for ControlledChild {
    fn drop(&mut self) {
        let _ = self.events.send("dropped");
    }
}

#[derive(Debug)]
struct NoopKiller;

impl ChildKiller for NoopKiller {
    fn kill(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(NoopKiller)
    }
}
impl ChildKiller for ControlledChild {
    fn kill(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(NoopKiller)
    }
}
impl Child for ControlledChild {
    fn try_wait(
        &mut self,
    ) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        Ok(None)
    }
    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        self.events.send("waiting").unwrap();
        if let Some(error) = self.failures.pop_front() {
            return Err(error.into());
        }
        self.exit.lock().unwrap().recv().unwrap();
        self.events.send("reaped").unwrap();
        Ok(portable_pty::ExitStatus::with_exit_code(0))
    }
    fn process_id(&self) -> Option<u32> {
        None
    }
    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        None
    }
}

fn child(
    failures: VecDeque<ErrorKind>,
) -> (
    Box<dyn Child + Send + Sync>,
    Sender<()>,
    Receiver<&'static str>,
) {
    let (exit, receiver) = mpsc::channel();
    let (events, observed) = mpsc::channel();
    (
        Box::new(ControlledChild {
            exit: Mutex::new(receiver),
            events,
            failures,
        }),
        exit,
        observed,
    )
}

fn event(events: &Receiver<&'static str>, expected: &str) {
    assert_eq!(
        events.recv_timeout(Duration::from_secs(3)).unwrap(),
        expected
    );
}

#[test]
fn timed_out_child_is_retained_until_exit_without_blocking_sibling_reap() {
    let (child, exit, events) = child(VecDeque::new());
    let started = Instant::now();
    assert!(!reap_child_with(child, Duration::ZERO, spawn_reaper));
    assert!(started.elapsed() < Duration::from_secs(1));
    event(&events, "waiting");
    assert!(events.try_recv().is_err(), "child dropped before exit");

    let (sibling, sibling_exit, sibling_events) = self::child(VecDeque::new());
    sibling_exit.send(()).unwrap();
    assert!(!reap_child_with(sibling, Duration::ZERO, spawn_reaper));
    event(&sibling_events, "waiting");
    event(&sibling_events, "reaped");
    event(&sibling_events, "dropped");
    assert!(events.try_recv().is_err(), "sibling affected pending child");

    exit.send(()).unwrap();
    event(&events, "reaped");
    event(&events, "dropped");
}

#[test]
fn deferred_reaper_retains_child_across_interrupted_and_repeated_wait_errors() {
    let (child, exit, events) = child(VecDeque::from([
        ErrorKind::Interrupted,
        ErrorKind::Other,
        ErrorKind::Other,
    ]));
    assert!(!reap_child_with(child, Duration::ZERO, spawn_reaper));
    event(&events, "waiting");
    event(&events, "waiting");
    event(&events, "waiting");
    event(&events, "waiting");
    assert!(events.try_recv().is_err(), "wait errors dropped the child");
    exit.send(()).unwrap();
    event(&events, "reaped");
    event(&events, "dropped");
}

#[test]
fn reaper_spawn_failure_keeps_child_for_synchronous_emergency_wait() {
    let (child, exit, events) = child(VecDeque::new());
    let (returned, completion) = mpsc::channel();
    let worker = thread::spawn(move || {
        let result = reap_child_with(child, Duration::ZERO, |_| {
            Err(std::io::Error::other("injected thread limit"))
        });
        returned.send(result).unwrap();
    });
    event(&events, "waiting");
    assert!(completion.try_recv().is_err());
    assert!(events.try_recv().is_err());
    exit.send(()).unwrap();
    event(&events, "reaped");
    event(&events, "dropped");
    assert!(!completion.recv_timeout(Duration::from_secs(3)).unwrap());
    worker.join().unwrap();
}
