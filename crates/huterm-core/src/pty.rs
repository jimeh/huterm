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

/// Serializes PTY creation within the process. Concurrent `openpty` calls
/// from parallel test threads failed on macOS 27 with errno -6; Huterm's own
/// spawns already serialize under the Mux lock, so this costs nothing there.
static OPENPTY: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn spawn(
    command: &TerminalCommand,
) -> Result<PtyProcess, RuntimeError> {
    let pair = {
        let _serial = OPENPTY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        portable_pty::native_pty_system()
            .openpty(pty_size(command.grid_size, command.cell_size))
            .map_err(|error| RuntimeError::Pty(error.to_string()))?
    };
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
mod tests;
