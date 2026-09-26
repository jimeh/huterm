#[cfg(unix)]
use filedescriptor::{AsRawFileDescriptor, FileDescriptor, RawFileDescriptor};
use std::collections::BTreeSet;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::{fd::AsFd, unix::net::UnixStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use huterm_protocol::{CellSize, GridSize, TerminalCommand};
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize};

use crate::terminal::RuntimeError;

const POLL_INTERVAL: Duration = Duration::from_millis(2);
const SIGNAL_GRACE_PERIOD: Duration = Duration::from_millis(100);
const KILL_WAIT_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) struct PtyProcess {
    master: Option<Box<dyn MasterPty + Send>>,
    reader: Option<Box<dyn Read + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn Child + Send + Sync>>,
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
}

impl std::fmt::Debug for PtyProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PtyProcess")
            .field("child", &self.child)
            .finish_non_exhaustive()
    }
}

impl PtyProcess {
    pub(crate) fn into_parts(mut self) -> Result<PtyParts, RuntimeError> {
        if self.master.is_none()
            || self.reader.is_none()
            || self.writer.is_none()
            || self.child.is_none()
            || self.killer.is_none()
        {
            return Err(RuntimeError::Invariant(
                "PTY process is missing an owned component",
            ));
        }
        let master = self.master.as_deref().ok_or(RuntimeError::Invariant(
            "PTY process has no master handle",
        ))?;
        let reader_waiter = ReadinessWaiter::new(master, Readiness::Read)?;
        let writer_waiter = ReadinessWaiter::new(master, Readiness::Write)?;
        match (
            self.master.take(),
            self.reader.take(),
            self.writer.take(),
            self.child.take(),
            self.killer.take(),
        ) {
            (
                Some(master),
                Some(reader),
                Some(writer),
                Some(child),
                Some(killer),
            ) => Ok(PtyParts {
                master,
                reader,
                reader_waiter,
                writer_waiter,
                writer,
                child,
                killer,
            }),
            _ => Err(RuntimeError::Invariant(
                "PTY process ownership changed during decomposition",
            )),
        }
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        let Some(killer) = self.killer.as_mut() else {
            return;
        };
        let mut groups = process_groups(child.process_id());
        if let Some(master) = self.master.as_ref() {
            record_foreground_group(master.as_ref(), &mut groups);
        }
        terminate_child(child.as_mut(), killer.as_mut(), &groups);
        drop(self.reader.take());
        drop(self.writer.take());
        drop(self.master.take());
        if let Some(child) = self.child.take() {
            let _ = reap_child(child);
        }
    }
}

pub(crate) struct PtyParts {
    pub(crate) master: Box<dyn MasterPty + Send>,
    pub(crate) reader: Box<dyn Read + Send>,
    pub(crate) reader_waiter: ReadinessWaiter,
    pub(crate) writer_waiter: ReadinessWaiter,
    pub(crate) writer: Box<dyn Write + Send>,
    pub(crate) child: Box<dyn Child + Send + Sync>,
    pub(crate) killer: Box<dyn ChildKiller + Send + Sync>,
}

#[derive(Clone, Copy)]
enum Readiness {
    Read,
    Write,
}

/// Which event released a readiness wait. Runtime workers retry their I/O
/// after every outcome; tests use it to tell the events apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadinessOutcome {
    /// The descriptor reported readiness, hangup, or an error, or a signal
    /// interrupted the wait.
    Ready,
    Cancelled,
    TimedOut,
}

pub(crate) struct ReadinessWaiter {
    #[cfg(unix)]
    interest: Readiness,
    #[cfg(unix)]
    fd: FileDescriptor,
    #[cfg(unix)]
    cancellation: UnixStream,
    cancel: IoCancellation,
}

/// A separate descriptor interrupts readiness without closing a descriptor
/// underneath poll, which is not a reliable cross-thread wakeup.
#[derive(Clone)]
pub(crate) struct IoCancellation {
    #[cfg(unix)]
    stream: Arc<UnixStream>,
}

impl IoCancellation {
    pub(crate) fn cancel(&self) {
        #[cfg(unix)]
        let _ = self.stream.shutdown(std::net::Shutdown::Write);
    }
}

#[cfg(unix)]
struct MasterDescriptor(RawFileDescriptor);

#[cfg(unix)]
impl AsRawFileDescriptor for MasterDescriptor {
    fn as_raw_file_descriptor(&self) -> RawFileDescriptor {
        self.0
    }
}

impl ReadinessWaiter {
    fn new(
        master: &dyn MasterPty,
        interest: Readiness,
    ) -> Result<Self, RuntimeError> {
        #[cfg(unix)]
        {
            let master_fd =
                master.as_raw_fd().ok_or(RuntimeError::Invariant(
                    "Unix PTY master has no raw file descriptor",
                ))?;
            let fd = FileDescriptor::dup(&MasterDescriptor(master_fd))
                .map_err(|error| RuntimeError::Pty(error.to_string()))?;
            let (cancellation, cancel) = UnixStream::pair()
                .map_err(|error| RuntimeError::Pty(error.to_string()))?;
            Ok(Self {
                interest,
                fd,
                cancellation,
                cancel: IoCancellation {
                    stream: Arc::new(cancel),
                },
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (master, interest);
            Ok(Self {
                cancel: IoCancellation {},
            })
        }
    }

    pub(crate) fn cancellation(&self) -> IoCancellation {
        self.cancel.clone()
    }

    pub(crate) fn wait(
        &self,
        timeout: Option<Duration>,
    ) -> std::io::Result<ReadinessOutcome> {
        #[cfg(unix)]
        {
            use nix::poll::{PollFd, PollFlags, poll};

            // poll(2) handles PTY masters on current macOS and Linux, and
            // unlike select(2) accepts descriptors at or above FD_SETSIZE.
            let mut descriptors = [
                PollFd::new(
                    self.fd.as_fd(),
                    match self.interest {
                        Readiness::Read => PollFlags::POLLIN,
                        Readiness::Write => PollFlags::POLLOUT,
                    },
                ),
                PollFd::new(self.cancellation.as_fd(), PollFlags::POLLIN),
            ];
            match poll(&mut descriptors, poll_timeout(timeout)) {
                Ok(0) => Ok(ReadinessOutcome::TimedOut),
                Ok(_) if descriptors[1].any().unwrap_or(true) => {
                    Ok(ReadinessOutcome::Cancelled)
                }
                // A signal interruption is a spurious wake; callers retry.
                Ok(_) | Err(nix::errno::Errno::EINTR) => {
                    Ok(ReadinessOutcome::Ready)
                }
                Err(error) => Err(error.into()),
            }
        }
        #[cfg(not(unix))]
        {
            thread::sleep(timeout.unwrap_or(POLL_INTERVAL));
            Ok(ReadinessOutcome::Ready)
        }
    }
}

/// Rounds up to whole milliseconds so a short timeout cannot become a
/// zero-timeout spin, and clamps durations beyond `poll(2)`'s range.
#[cfg(unix)]
fn poll_timeout(timeout: Option<Duration>) -> nix::poll::PollTimeout {
    use nix::poll::PollTimeout;

    timeout.map_or(PollTimeout::NONE, |timeout| {
        PollTimeout::try_from(timeout.as_nanos().div_ceil(1_000_000))
            .unwrap_or(PollTimeout::MAX)
    })
}

pub(crate) fn spawn(
    command: &TerminalCommand,
) -> Result<PtyProcess, RuntimeError> {
    let pair = portable_pty::native_pty_system()
        .openpty(pty_size(command.grid_size, command.cell_size))
        .map_err(|error| RuntimeError::Pty(error.to_string()))?;
    set_nonblocking(pair.master.as_ref())?;

    let mut builder = CommandBuilder::new(&command.program);
    builder.args(&command.arguments);
    builder.cwd(&command.working_directory);
    builder.env("TERM", "xterm-256color");
    builder.env("COLORTERM", "truecolor");
    builder.env("TERM_PROGRAM", "Huterm");
    for (key, value) in &command.environment {
        builder.env(key, value);
    }
    // Children start with the limit Huterm had before raising its own.
    #[cfg(unix)]
    builder.nofile_limit(crate::limits::original_open_file_limit());

    let child = pair
        .slave
        .spawn_command(builder)
        .map_err(|error| RuntimeError::Spawn(error.to_string()))?;
    let killer = child.clone_killer();
    drop(pair.slave);
    let mut process = PtyProcess {
        master: Some(pair.master),
        reader: None,
        writer: None,
        child: Some(child),
        killer: Some(killer),
    };

    let reader = process
        .master
        .as_ref()
        .ok_or(RuntimeError::Invariant("PTY process has no master handle"))?
        .try_clone_reader()
        .map_err(|error| RuntimeError::Pty(error.to_string()))?;
    process.reader = Some(reader);
    let writer = clone_writer(
        process
            .master
            .as_ref()
            .ok_or(RuntimeError::Invariant("PTY process has no master handle"))?
            .as_ref(),
    )?;
    process.writer = Some(writer);

    Ok(process)
}

#[cfg(unix)]
fn clone_writer(
    master: &dyn MasterPty,
) -> Result<Box<dyn Write + Send>, RuntimeError> {
    let master_fd = master.as_raw_fd().ok_or(RuntimeError::Invariant(
        "Unix PTY master has no raw file descriptor",
    ))?;
    // portable-pty's Unix writer injects a newline and EOF when dropped.
    // Huterm owns shutdown explicitly, so keep teardown bytes out of history.
    FileDescriptor::dup(&MasterDescriptor(master_fd))
        .map(|writer| Box::new(writer) as Box<dyn Write + Send>)
        .map_err(|error| RuntimeError::Pty(error.to_string()))
}

#[cfg(not(unix))]
fn clone_writer(
    master: &dyn MasterPty,
) -> Result<Box<dyn Write + Send>, RuntimeError> {
    master
        .take_writer()
        .map_err(|error| RuntimeError::Pty(error.to_string()))
}

#[cfg(unix)]
fn set_nonblocking(master: &dyn MasterPty) -> Result<(), RuntimeError> {
    use nix::fcntl::{FcntlArg, OFlag, fcntl};

    let fd = master.as_raw_fd().ok_or(RuntimeError::Invariant(
        "Unix PTY master has no raw file descriptor",
    ))?;
    let flags = fcntl(fd, FcntlArg::F_GETFL)
        .map(OFlag::from_bits_truncate)
        .map_err(|error| RuntimeError::Pty(error.to_string()))?;
    fcntl(fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))
        .map_err(|error| RuntimeError::Pty(error.to_string()))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_nonblocking(_: &dyn MasterPty) -> Result<(), RuntimeError> {
    Ok(())
}

#[derive(Debug)]
pub(crate) struct ProcessGroups {
    shell: Option<i32>,
    foreground: Option<i32>,
    pub(crate) assessed: BTreeSet<i32>,
}

pub(crate) fn process_groups(child_pid: Option<u32>) -> ProcessGroups {
    ProcessGroups {
        shell: child_pid.and_then(|pid| i32::try_from(pid).ok()),
        foreground: None,
        assessed: BTreeSet::new(),
    }
}

pub(crate) fn record_foreground_group(
    _master: &dyn MasterPty,
    _groups: &mut ProcessGroups,
) {
    #[cfg(unix)]
    {
        let own_group = nix::unistd::getpgrp().as_raw();
        _groups.foreground = _master
            .process_group_leader()
            .filter(|group| *group > 0 && *group != own_group);
    }
}

pub(crate) fn terminate_child(
    child: &mut dyn Child,
    _killer: &mut dyn ChildKiller,
    groups: &ProcessGroups,
) -> bool {
    let child_running = !matches!(child.try_wait(), Ok(Some(_)));

    #[cfg(unix)]
    {
        use nix::sys::signal::Signal;

        let targets = groups.signal_targets(child_running);
        if targets.is_empty() {
            return !child_running;
        }
        for (signal, timeout) in [
            (Signal::SIGHUP, SIGNAL_GRACE_PERIOD),
            (Signal::SIGTERM, SIGNAL_GRACE_PERIOD),
            (Signal::SIGKILL, KILL_WAIT_TIMEOUT),
        ] {
            signal_groups(&targets, signal);
            if poll_termination(child, &targets, timeout) {
                return true;
            }
        }
        false
    }

    #[cfg(not(unix))]
    {
        let _ = _killer.kill();
        poll_child_exit(child, KILL_WAIT_TIMEOUT)
    }
}

impl ProcessGroups {
    #[cfg(unix)]
    fn signal_targets(&self, child_running: bool) -> BTreeSet<i32> {
        let mut targets: BTreeSet<_> = self
            .assessed
            .iter()
            .copied()
            .filter(|group| *group > 0)
            .collect();
        if let Some(foreground) = self.foreground {
            targets.insert(foreground);
        }
        if child_running && let Some(shell) = self.shell {
            targets.insert(shell);
        } else if let Some(shell) = self.shell
            && !self.assessed.contains(&shell)
        {
            targets.remove(&shell);
        }
        targets
    }
}

#[cfg(unix)]
fn signal_groups(groups: &BTreeSet<i32>, signal: nix::sys::signal::Signal) {
    use nix::sys::signal::killpg;
    use nix::unistd::Pid;

    let own_group = nix::unistd::getpgrp().as_raw();
    for group in groups.iter().copied().filter(|group| *group != own_group) {
        let _ = killpg(Pid::from_raw(group), signal);
    }
}

#[cfg(unix)]
fn poll_termination(
    child: &mut dyn Child,
    groups: &BTreeSet<i32>,
    timeout: Duration,
) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::killpg;
    use nix::unistd::Pid;

    let deadline = Instant::now() + timeout;
    loop {
        let child_done = matches!(child.try_wait(), Ok(Some(_)));
        let groups_done = groups.iter().all(|group| {
            matches!(killpg(Pid::from_raw(*group), None), Err(Errno::ESRCH))
        });
        if child_done && groups_done {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

// The final master descriptor can deliver the hangup that actually exits a
// child. Reap after I/O workers have released their descriptor clones as well.
pub(crate) fn reap_child(child: Box<dyn Child + Send + Sync>) -> bool {
    reap_child_with(child, KILL_WAIT_TIMEOUT, spawn_reaper)
}

fn spawn_reaper(reap: ReapTask) -> std::io::Result<()> {
    thread::Builder::new()
        .name("huterm-child-reaper".into())
        .spawn(reap)
        .map(drop)
}

type ReapTask = Box<dyn FnOnce() + Send>;

fn reap_child_with(
    mut child: Box<dyn Child + Send + Sync>,
    timeout: Duration,
    spawn: impl FnOnce(ReapTask) -> std::io::Result<()>,
) -> bool {
    if poll_child_exit(child.as_mut(), timeout) {
        return true;
    }
    // Keep a second owner until spawn succeeds: Builder::spawn drops its
    // closure on failure, which must not drop the only unreaped child handle.
    let retained = Arc::new(Mutex::new(Some(child)));
    let deferred = Arc::clone(&retained);
    let reap = Box::new(move || {
        if let Some(child) = deferred
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            wait_until_reaped(child);
        }
    });
    if let Err(error) = spawn(reap) {
        eprintln!("Cannot start child reaper ({error}); waiting synchronously");
        if let Some(child) = retained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            wait_until_reaped(child);
        }
    }
    false
}

fn wait_until_reaped(mut child: Box<dyn Child + Send + Sync>) {
    let mut reported_error = false;
    loop {
        match child.wait() {
            Ok(_) => return,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                if !reported_error {
                    eprintln!("Deferred child wait failed ({error}); retrying");
                    reported_error = true;
                }
                // Preserve ownership even if a platform wait fails transiently.
                thread::sleep(SIGNAL_GRACE_PERIOD);
            }
        }
    }
}

fn poll_child_exit(child: &mut dyn Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
    child.try_wait().ok().flatten().is_some()
}

pub(crate) fn pty_size(grid: GridSize, cell: CellSize) -> PtySize {
    let grid = GridSize::clamped(grid.columns, grid.rows);
    PtySize {
        rows: grid.rows,
        cols: grid.columns,
        pixel_width: cell.width.saturating_mul(grid.columns),
        pixel_height: cell.height.saturating_mul(grid.rows),
    }
}

#[cfg(test)]
mod tests {
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
        fn high_descriptor_waiter(
            &self,
            interest: Readiness,
        ) -> ReadinessWaiter {
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
                    ReadinessWaiter::new(self.master.as_ref(), interest)
                        .unwrap();
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
            ReadinessWaiter::new(pair.master.as_ref(), Readiness::Read)
                .unwrap();
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
            ReadinessWaiter::new(pair.master.as_ref(), Readiness::Read)
                .unwrap();
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

        let (sibling, sibling_exit, sibling_events) =
            self::child(VecDeque::new());
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
    fn deferred_reaper_retains_child_across_interrupted_and_repeated_wait_errors()
     {
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
}
