#[cfg(unix)]
use filedescriptor::{AsRawFileDescriptor, FileDescriptor, RawFileDescriptor};
use std::collections::BTreeSet;
use std::io::{Read, Write};
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
        let reader_waiter = ReaderWaiter::new(self.master.as_deref().ok_or(
            RuntimeError::Invariant("PTY process has no master handle"),
        )?)?;
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
        let _ = reap_child(child.as_mut());
    }
}

pub(crate) struct PtyParts {
    pub(crate) master: Box<dyn MasterPty + Send>,
    pub(crate) reader: Box<dyn Read + Send>,
    pub(crate) reader_waiter: ReaderWaiter,
    pub(crate) writer: Box<dyn Write + Send>,
    pub(crate) child: Box<dyn Child + Send + Sync>,
    pub(crate) killer: Box<dyn ChildKiller + Send + Sync>,
}

pub(crate) struct ReaderWaiter {
    #[cfg(unix)]
    fd: FileDescriptor,
}

#[cfg(unix)]
struct MasterDescriptor(RawFileDescriptor);

#[cfg(unix)]
impl AsRawFileDescriptor for MasterDescriptor {
    fn as_raw_file_descriptor(&self) -> RawFileDescriptor {
        self.0
    }
}

impl ReaderWaiter {
    fn new(master: &dyn MasterPty) -> Result<Self, RuntimeError> {
        #[cfg(unix)]
        {
            let master_fd =
                master.as_raw_fd().ok_or(RuntimeError::Invariant(
                    "Unix PTY master has no raw file descriptor",
                ))?;
            let fd = FileDescriptor::dup(&MasterDescriptor(master_fd))
                .map_err(|error| RuntimeError::Pty(error.to_string()))?;
            Ok(Self { fd })
        }
        #[cfg(not(unix))]
        {
            let _ = master;
            Ok(Self {})
        }
    }

    pub(crate) fn wait(&self, timeout: Duration) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            // filedescriptor uses select(2) on macOS, where poll(2) is not
            // reliable for PTY descriptors.
            let mut descriptors = [filedescriptor::pollfd {
                fd: self.fd.as_raw_file_descriptor(),
                events: filedescriptor::POLLIN,
                revents: 0,
            }];
            match filedescriptor::poll(&mut descriptors, Some(timeout)) {
                Ok(_) => Ok(()),
                Err(error) if poll_was_interrupted(&error) => Ok(()),
                Err(error) => Err(std::io::Error::other(error)),
            }
        }
        #[cfg(not(unix))]
        {
            thread::sleep(timeout);
            Ok(())
        }
    }
}

#[cfg(unix)]
fn poll_was_interrupted(error: &filedescriptor::Error) -> bool {
    match error {
        filedescriptor::Error::Poll(source)
        | filedescriptor::Error::Io(source) => {
            source.kind() == std::io::ErrorKind::Interrupted
        }
        _ => false,
    }
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
    for (key, value) in &command.environment {
        builder.env(key, value);
    }
    builder.env("TERM", "xterm-256color");
    builder.env("COLORTERM", "truecolor");
    builder.env("TERM_PROGRAM", "Huterm");

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
    let writer = process
        .master
        .as_ref()
        .ok_or(RuntimeError::Invariant("PTY process has no master handle"))?
        .take_writer()
        .map_err(|error| RuntimeError::Pty(error.to_string()))?;
    process.writer = Some(writer);

    Ok(process)
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
}

pub(crate) fn process_groups(child_pid: Option<u32>) -> ProcessGroups {
    ProcessGroups {
        shell: child_pid.and_then(|pid| i32::try_from(pid).ok()),
        foreground: None,
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
        let mut targets = BTreeSet::new();
        if let Some(foreground) = self.foreground {
            targets.insert(foreground);
        }
        if child_running && let Some(shell) = self.shell {
            targets.insert(shell);
        } else if let Some(shell) = self.shell {
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
pub(crate) fn reap_child(child: &mut dyn Child) -> bool {
    poll_child_exit(child, KILL_WAIT_TIMEOUT)
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
