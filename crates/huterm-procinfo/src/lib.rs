//! Operating-system process facts for Huterm.
//!
//! Every query reads one process or one process group directly from the
//! kernel, without spawning helper programs. macOS calls `libproc` and
//! `sysctl` through a small unsafe module; Linux reads `/proc` with the
//! standard library.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod name;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;

pub use name::{display_name, is_shell};

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// When a process started, comparable only between reads on the same boot.
///
/// A process keeps its start time across `exec`, so a PID with a different
/// start time is a different process.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StartTime {
    seconds: u64,
    fraction: u64,
}

impl std::fmt::Display for StartTime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{:06}", self.seconds, self.fraction)
    }
}

/// Facts about one process at the time it was read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Process {
    /// Process ID.
    pub pid: u32,
    /// Parent process ID.
    pub parent: u32,
    /// Process group ID.
    pub group: u32,
    /// Device number of the controlling terminal. `None` without one, and on
    /// macOS for processes owned by another user.
    pub tty: Option<u64>,
    /// Whether the process has exited and awaits reaping by its parent.
    pub zombie: bool,
    /// Start time, used to tell a reused PID from the original process.
    /// `None` on macOS for processes owned by another user.
    pub started: Option<StartTime>,
    /// The kernel's short command name. It may be truncated and does not
    /// name scripts reliably; prefer [`display_name`] over [`arguments`].
    pub name: String,
}

/// Reads one process, or `None` when it does not exist or cannot be read.
#[must_use]
pub fn process(pid: u32) -> Option<Process> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        platform::process(pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

/// Reads a process's argument vector, or `None` when it cannot be read.
#[must_use]
pub fn arguments(pid: u32) -> Option<Vec<String>> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        platform::arguments(pid).filter(|arguments| !arguments.is_empty())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

/// Reads a process's working directory. The kernel only reveals it for the
/// caller's own processes unless it runs as root, so other users' processes,
/// such as `sudo` jobs, read as `None`.
#[must_use]
pub fn cwd(pid: u32) -> Option<PathBuf> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        platform::cwd(pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

/// Lists the PIDs in a process group, including zombies, or `None` when the
/// process list cannot be read.
#[must_use]
pub fn group_members(group: u32) -> Option<Vec<u32>> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        platform::group_members(group)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = group;
        None
    }
}

/// Every readable process, and which processes use each requested terminal.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcessTable {
    /// Processes that existed while the table was read.
    pub processes: Vec<Process>,
    ttys: BTreeMap<u64, BTreeSet<u32>>,
}

impl ProcessTable {
    /// Whether `pid` has the terminal `device` as its controlling terminal.
    /// Only devices requested from [`process_table`] are known.
    #[must_use]
    pub fn on_tty(&self, device: u64, pid: u32) -> bool {
        self.ttys
            .get(&device)
            .is_some_and(|members| members.contains(&pid))
    }
}

/// Reads every process and the members of each terminal in `ttys`, or
/// `None` when the process list cannot be read.
///
/// macOS asks the kernel for each terminal's members, because other users'
/// processes do not report their terminal. Linux reads every process's
/// terminal in the same scan.
#[must_use]
pub fn process_table(ttys: &[u64]) -> Option<ProcessTable> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        platform::process_table(ttys)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = ttys;
        None
    }
}

/// Reads the device number of a terminal such as `/dev/ttys003`, comparable
/// with [`Process::tty`] and [`ProcessTable::on_tty`].
#[must_use]
pub fn tty_device(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|metadata| metadata.rdev())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    /// Kills and reaps its child even when an assertion fails.
    struct Reaped(Child);

    impl Drop for Reaped {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Spawns a child that reports readiness on stdout after it has exec'd.
    fn ready_child(command: &mut Command) -> Reaped {
        let mut child = Reaped(command.stdout(Stdio::piped()).spawn().unwrap());
        let mut line = String::new();
        BufReader::new(child.0.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line, "ready\n");
        child
    }

    fn sleeper() -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo ready; exec sleep 30"]);
        command
    }

    fn eventually<T>(mut read: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(value) = read() {
                return value;
            }
            assert!(Instant::now() < deadline, "condition never held");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn reads_a_live_process_group_leader_and_its_arguments() {
        let child = ready_child(sleeper().process_group(0));
        let pid = child.0.id();
        // The shell prints before exec; wait for sleep to replace it.
        let facts = eventually(|| process(pid).filter(|p| p.name == "sleep"));
        assert_eq!(facts.pid, pid);
        assert_eq!(facts.parent, std::process::id());
        assert_eq!(facts.group, pid);
        assert!(!facts.zombie);
        assert!(facts.started.is_some());
        assert_eq!(arguments(pid).unwrap(), ["sleep", "30"]);
        assert_eq!(process(pid).unwrap().started, facts.started);
    }

    #[test]
    fn lists_members_of_a_process_group() {
        let leader = ready_child(sleeper().process_group(0));
        let group = leader.0.id();
        let member =
            ready_child(sleeper().process_group(i32::try_from(group).unwrap()));
        let members = group_members(group).unwrap();
        assert!(members.contains(&group), "{members:?}");
        assert!(members.contains(&member.0.id()), "{members:?}");
        assert_eq!(process(member.0.id()).unwrap().group, group);
    }

    #[test]
    fn names_a_shebang_script_rather_than_its_interpreter() {
        let directory = std::env::temp_dir()
            .join(format!("huterm-procinfo-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let script = directory.join("probe-script");
        std::fs::write(
            &script,
            "#!/bin/sh\necho ready\nwhile :; do sleep 1; done\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, PermissionsExt::from_mode(0o755))
            .unwrap();
        let child = ready_child(&mut Command::new(&script));
        let arguments = arguments(child.0.id()).unwrap();
        assert_eq!(
            display_name(&arguments).as_deref(),
            Some("probe-script"),
            "{arguments:?}"
        );
        drop(child);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn exited_children_are_absent_or_reported_as_zombies() {
        let mut child = Command::new("/bin/sh").args(["-c", "exit 0"]).spawn();
        let child = child.as_mut().unwrap();
        let pid = child.id();
        // Unreaped, the child stays a zombie until wait below.
        eventually(|| {
            process(pid).is_none_or(|facts| facts.zombie).then_some(())
        });
        child.wait().unwrap();
    }

    #[test]
    fn reads_the_working_directory_of_a_child() {
        let directory = std::env::temp_dir()
            .join(format!("huterm-procinfo-cwd-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let child = ready_child(sleeper().current_dir(&directory));
        // macOS resolves the temporary directory through /private.
        let expected = directory.canonicalize().unwrap();
        assert_eq!(
            cwd(child.0.id()).unwrap().canonicalize().unwrap(),
            expected
        );
        drop(child);
        let _ = std::fs::remove_dir_all(directory);
        assert_eq!(cwd(u32::try_from(i32::MAX).unwrap()), None);
    }

    #[test]
    fn process_tables_include_every_process_and_known_terminals() {
        let shell = ready_child(
            Command::new("/bin/sh")
                .args(["-c", "sleep 30 & echo ready; wait"])
                .process_group(0),
        );
        let pid = shell.0.id();
        let own = process(std::process::id()).unwrap();
        let ttys: Vec<u64> = own.tty.into_iter().collect();
        let child = eventually(|| {
            let table = process_table(&ttys)?;
            assert!(table.processes.iter().any(|p| p.pid == 1));
            if let Some(device) = own.tty {
                assert!(table.on_tty(device, own.pid));
            }
            table
                .processes
                .into_iter()
                .find(|p| p.parent == pid && p.name == "sleep")
        });
        assert_eq!(child.group, pid);
        assert!(!process_table(&[]).unwrap().on_tty(0, child.pid));
    }

    #[test]
    fn tty_devices_come_from_device_nodes() {
        assert!(tty_device(Path::new("/dev/null")).is_some());
        assert_eq!(tty_device(Path::new("/nonexistent/tty")), None);
    }

    #[test]
    fn reads_processes_owned_by_other_users() {
        // PID 1 (launchd, or the container's init) runs as another user or
        // as root, like a `sudo` job would.
        let init = process(1).unwrap();
        assert_eq!(init.pid, 1);
        assert_eq!(init.parent, 0);
        assert!(!init.name.is_empty());
        assert!(!init.zombie);
    }

    #[test]
    fn missing_processes_read_as_none() {
        // PIDs are bounded well below this on macOS and Linux.
        let missing = u32::try_from(i32::MAX).unwrap();
        assert_eq!(process(missing), None);
        assert_eq!(arguments(missing), None);
    }
}
