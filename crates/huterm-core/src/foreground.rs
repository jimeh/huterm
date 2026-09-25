//! Foreground process names and directories, probed on the terminal's own
//! runtime thread.
//!
//! Probes run only after events that can change the foreground process, plus
//! a slow poll while a job holds the foreground. An idle shell arms no
//! deadline, so it causes no wakeups.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use huterm_protocol::TerminalDirectory;

/// Delay after a trigger, so the shell can fork, exec, and hand over the PTY.
const SETTLE: Duration = Duration::from_millis(50);
/// A second look after input, for shells whose command startup is slower.
const FOLLOW_UP: Duration = Duration::from_millis(500);
/// Output after this much silence usually means a prompt redrawing.
const QUIET: Duration = Duration::from_millis(250);
/// Catches `exec` and silent exits while a job holds the foreground.
const JOB_POLL: Duration = Duration::from_secs(1);
const MIN_INTERVAL: Duration = Duration::from_millis(100);
const MAX_TRIGGERS: usize = 4;
/// How long a repeated title keeps its group attribution before the group is
/// read again. A new job can repeat an exited job's title text.
const TITLE_REREAD: Duration = Duration::from_millis(250);

/// When to probe, as a pure function of observed events and instants.
#[derive(Debug, Default)]
pub(crate) struct ProbeSchedule {
    triggers: BTreeSet<Instant>,
    /// The job poll, kept apart from triggers so each probe replaces it
    /// rather than starting another repeating poll.
    poll: Option<Instant>,
    last_probe: Option<Instant>,
    last_output: Option<Instant>,
}

impl ProbeSchedule {
    /// Enter, interrupts, end-of-file, and suspends can change the job.
    /// Returns whether `bytes` contained one of them.
    pub(crate) fn input(&mut self, bytes: &[u8], now: Instant) -> bool {
        let changes_job = bytes.iter().any(|byte| {
            matches!(byte, b'\r' | b'\n' | 0x03 | 0x04 | 0x1a | 0x1c)
        });
        if changes_job {
            self.arm(now + SETTLE);
            self.arm(now + FOLLOW_UP);
        }
        changes_job
    }

    pub(crate) fn output(&mut self, now: Instant) {
        if self
            .last_output
            .is_none_or(|last| now.saturating_duration_since(last) >= QUIET)
        {
            self.arm(now + SETTLE);
        }
        self.last_output = Some(now);
    }

    pub(crate) fn title(&mut self, now: Instant) {
        self.arm(now + SETTLE);
    }

    /// The earliest instant a probe may run, respecting the rate limit.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        let first = match (self.triggers.first().copied(), self.poll) {
            (Some(trigger), Some(poll)) => trigger.min(poll),
            (trigger, poll) => trigger.or(poll)?,
        };
        Some(
            self.last_probe
                .map_or(first, |last| first.max(last + MIN_INTERVAL)),
        )
    }

    pub(crate) fn due(&self, now: Instant) -> bool {
        self.deadline().is_some_and(|deadline| deadline <= now)
    }

    /// Records a probe at `now`. A foreground job keeps a slow poll armed.
    pub(crate) fn probed(&mut self, now: Instant, job: bool) {
        self.last_probe = Some(now);
        self.triggers.retain(|trigger| *trigger > now);
        self.poll = job.then_some(now + JOB_POLL);
    }

    pub(crate) fn stop(&mut self) {
        self.triggers.clear();
        self.poll = None;
    }

    fn arm(&mut self, at: Instant) {
        self.triggers.insert(at);
        while self.triggers.len() > MAX_TRIGGERS {
            self.triggers.pop_last();
        }
    }
}

/// Result of one probe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Probe {
    /// The foreground process group the probe observed.
    pub(crate) group: Option<i32>,
    /// The name to publish, or `None` while the root shell is idle.
    pub(crate) name: Option<String>,
    /// The foreground process's working directory, or the root's when that
    /// cannot be read, such as for another user's `sudo` job.
    pub(crate) directory: Option<TerminalDirectory>,
    /// Whether a job holds the foreground and needs the slow poll. A group
    /// with no live member counts: the shell may not have reclaimed the
    /// terminal from an exited job yet.
    pub(crate) job: bool,
}

/// Probes the foreground process. Its argv is read on every probe: `exec`
/// of the same interpreter with another script keeps the PID, start time,
/// and kernel name.
pub(crate) fn probe(group: Option<i32>, root: Option<u32>) -> Probe {
    let selected = group
        .and_then(|group| u32::try_from(group).ok())
        .filter(|group| *group > 0)
        .and_then(selected_process);
    let directory = selected
        .as_ref()
        .and_then(|process| huterm_procinfo::cwd(process.pid))
        .or_else(|| root.and_then(huterm_procinfo::cwd))
        .and_then(|path| path.into_os_string().into_string().ok())
        .map(|path| TerminalDirectory::new(None, path, true));
    let Some(process) = selected else {
        let root_group = root.and_then(|root| i32::try_from(root).ok());
        return Probe {
            group,
            name: None,
            directory,
            job: group
                .is_some_and(|group| group > 0 && Some(group) != root_group),
        };
    };
    let display = huterm_procinfo::arguments(process.pid)
        .as_deref()
        .and_then(huterm_procinfo::display_name)
        .or_else(|| {
            huterm_procinfo::display_name(std::slice::from_ref(&process.name))
        });
    let idle = Some(process.pid) == root
        && display.as_deref().is_none_or(huterm_procinfo::is_shell);
    Probe {
        group,
        name: display.filter(|_| !idle),
        directory,
        job: !idle,
    }
}

/// Reports scoped to the process group that held the foreground when they
/// arrived. Each applies only while that group keeps the foreground, so a
/// shell's reports apply at its prompt, a running job's apply while it runs,
/// and a remote shell's reports under `ssh` stop applying once `ssh` exits.
/// Without a current OSC 7 report, the probed process directory applies.
#[derive(Debug, Default)]
pub(crate) struct Reports {
    foreground: Option<i32>,
    process_directory: Option<TerminalDirectory>,
    directory: Option<(Option<i32>, TerminalDirectory)>,
    title: Option<(Option<i32>, String)>,
    title_read: Option<Instant>,
    /// Job-control input arrived since the last probe, so a new job may hold
    /// the foreground before a probe observes it.
    unprobed_input: bool,
}

impl Reports {
    /// Records an OSC 7 report, or its clearing, from `foreground`.
    pub(crate) fn report_directory(
        &mut self,
        directory: Option<TerminalDirectory>,
        foreground: Option<i32>,
    ) {
        self.foreground = foreground;
        self.directory = directory.map(|directory| (foreground, directory));
    }

    /// Records an OSC 0/2 title read with `foreground` at `now`. An empty
    /// title clears it.
    pub(crate) fn report_title(
        &mut self,
        title: String,
        foreground: Option<i32>,
        now: Instant,
    ) {
        self.foreground = foreground;
        self.title_read = Some(now);
        self.title = (!title.is_empty()).then_some((foreground, title));
    }

    /// Whether `title` differs from the recorded title's text.
    pub(crate) fn title_text_changes(&self, title: &str) -> bool {
        self.title
            .as_ref()
            .map_or(!title.is_empty(), |(_, current)| current != title)
    }

    /// Whether reporting `title` needs a fresh foreground group. A program
    /// re-sending its title needs at most one read per [`TITLE_REREAD`]. A
    /// repeated title is read again at once when its attribution no longer
    /// applies, or after job-control input that may have started a new job
    /// the probe has not observed yet.
    pub(crate) fn title_needs_group(&self, title: &str, now: Instant) -> bool {
        self.title_text_changes(title)
            || self.title.is_some()
                && (self.unprobed_input
                    || self.title().is_none()
                    || self.title_read.is_none_or(|read| {
                        now.saturating_duration_since(read) >= TITLE_REREAD
                    }))
    }

    /// Records job-control input, which can hand the foreground to a new job.
    pub(crate) fn job_control_input(&mut self) {
        self.unprobed_input = true;
    }

    pub(crate) fn probed(
        &mut self,
        foreground: Option<i32>,
        process_directory: Option<TerminalDirectory>,
    ) {
        self.foreground = foreground;
        self.process_directory = process_directory;
        self.unprobed_input = false;
    }

    pub(crate) fn directory(&self) -> Option<TerminalDirectory> {
        self.current(self.directory.as_ref())
            .or_else(|| self.process_directory.clone())
    }

    pub(crate) fn title(&self) -> Option<String> {
        self.current(self.title.as_ref())
    }

    fn current<T: Clone>(
        &self,
        report: Option<&(Option<i32>, T)>,
    ) -> Option<T> {
        report
            .filter(|(reporter, _)| *reporter == self.foreground)
            .map(|(_, value)| value.clone())
    }
}

/// The group's leader, or its lowest live member once the leader has gone.
fn selected_process(group: u32) -> Option<huterm_procinfo::Process> {
    let live = |pid| {
        huterm_procinfo::process(pid)
            .filter(|process| process.group == group && !process.zombie)
    };
    live(group).or_else(|| {
        let mut members = huterm_procinfo::group_members(group)?;
        members.sort_unstable();
        members.into_iter().find_map(live)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, milliseconds: u64) -> Instant {
        start + Duration::from_millis(milliseconds)
    }

    #[test]
    fn nothing_is_armed_until_an_event_and_idle_shells_rearm_nothing() {
        // A probe at spawn could see the child before it execs its program.
        let start = Instant::now();
        let mut schedule = ProbeSchedule::default();
        assert_eq!(schedule.deadline(), None);
        schedule.output(start);
        schedule.probed(at(start, 50), false);
        assert_eq!(schedule.deadline(), None, "an idle shell arms nothing");
    }

    #[test]
    fn command_input_probes_after_settling_and_again_later() {
        let start = Instant::now();
        let mut schedule = ProbeSchedule::default();
        schedule.input(b"ls", start);
        assert_eq!(schedule.deadline(), None, "typing alone arms nothing");
        schedule.input(b"\r", start);
        assert_eq!(schedule.deadline(), Some(at(start, 50)));
        schedule.probed(at(start, 50), false);
        assert_eq!(schedule.deadline(), Some(at(start, 500)));
        schedule.probed(at(start, 500), false);
        assert_eq!(schedule.deadline(), None);
        for control in [0x03, 0x04, 0x1a, 0x1c, b'\n'] {
            schedule.input(&[control], start);
            assert!(schedule.deadline().is_some(), "{control:#x}");
            schedule.stop();
        }
    }

    #[test]
    fn output_probes_only_after_silence() {
        let start = Instant::now();
        let mut schedule = ProbeSchedule::default();
        schedule.output(start);
        assert_eq!(schedule.deadline(), Some(at(start, 50)));
        schedule.probed(at(start, 50), false);
        for milliseconds in (60..1_000).step_by(10) {
            schedule.output(at(start, milliseconds));
        }
        assert_eq!(schedule.deadline(), None, "a flood arms one probe");
        schedule.output(at(start, 1_300));
        assert_eq!(schedule.deadline(), Some(at(start, 1_350)));
    }

    #[test]
    fn titles_probe_and_jobs_keep_a_slow_poll() {
        let start = Instant::now();
        let mut schedule = ProbeSchedule::default();
        schedule.title(start);
        assert_eq!(schedule.deadline(), Some(at(start, 50)));
        schedule.probed(at(start, 50), true);
        assert_eq!(schedule.deadline(), Some(at(start, 1_050)));
        schedule.probed(at(start, 1_050), true);
        assert_eq!(schedule.deadline(), Some(at(start, 2_050)));
        schedule.probed(at(start, 2_050), false);
        assert_eq!(
            schedule.deadline(),
            None,
            "returning to the shell stops polling"
        );
    }

    #[test]
    fn probes_are_rate_limited_and_deadlines_bounded() {
        let start = Instant::now();
        let mut schedule = ProbeSchedule::default();
        schedule.probed(start, false);
        schedule.title(start);
        assert_eq!(schedule.deadline(), Some(at(start, 100)));
        assert!(!schedule.due(at(start, 99)));
        assert!(schedule.due(at(start, 100)));
        for milliseconds in 0..20 {
            schedule.title(at(start, milliseconds));
        }
        assert!(schedule.triggers.len() <= MAX_TRIGGERS);
        assert_eq!(schedule.triggers.first(), Some(&at(start, 50)));
    }

    fn directory(path: &str, local: bool) -> TerminalDirectory {
        TerminalDirectory::new(None, path.into(), local)
    }

    #[test]
    fn reports_apply_while_their_reporter_holds_the_foreground() {
        let (shell, job) = (Some(10), Some(20));
        let mut directories = Reports::default();
        directories.probed(shell, Some(directory("/home/me", true)));
        assert_eq!(directories.directory(), Some(directory("/home/me", true)));
        directories
            .report_directory(Some(directory("/home/me/src", true)), shell);
        assert_eq!(
            directories.directory(),
            Some(directory("/home/me/src", true)),
            "the shell's report wins at its prompt"
        );
        directories.probed(job, Some(directory("/tmp", true)));
        assert_eq!(
            directories.directory(),
            Some(directory("/tmp", true)),
            "a running job shows its own directory"
        );
        directories.probed(shell, Some(directory("/home/me", true)));
        assert_eq!(
            directories.directory(),
            Some(directory("/home/me/src", true))
        );
    }

    #[test]
    fn a_jobs_report_expires_with_the_job_and_clearing_falls_back() {
        let (shell, ssh) = (Some(10), Some(30));
        let mut directories = Reports::default();
        directories.probed(ssh, Some(directory("/home/me", true)));
        directories
            .report_directory(Some(directory("/srv/remote", false)), ssh);
        assert_eq!(
            directories.directory(),
            Some(directory("/srv/remote", false))
        );
        directories.probed(shell, Some(directory("/home/me", true)));
        assert_eq!(
            directories.directory(),
            Some(directory("/home/me", true)),
            "a remote report stops applying after ssh exits"
        );
        directories
            .report_directory(Some(directory("/home/me/src", true)), shell);
        directories.report_directory(None, shell);
        assert_eq!(directories.directory(), Some(directory("/home/me", true)));
        assert_eq!(Reports::default().directory(), None);
    }

    #[test]
    fn titles_apply_only_while_their_setter_holds_the_foreground() {
        let start = Instant::now();
        let (shell, job) = (Some(10), Some(20));
        let mut reports = Reports::default();
        reports.probed(shell, None);
        assert!(reports.title_needs_group("~/src", start));
        reports.report_title("~/src".into(), shell, start);
        assert_eq!(reports.title().as_deref(), Some("~/src"));
        reports.probed(job, None);
        assert_eq!(reports.title(), None, "a job has not titled itself yet");
        reports.report_title("notes.txt - VIM".into(), job, start);
        assert_eq!(reports.title().as_deref(), Some("notes.txt - VIM"));
        reports.probed(shell, None);
        assert_eq!(reports.title(), None, "the job's title leaves with it");
        reports.report_title(String::new(), shell, start);
        assert_eq!(reports.title(), None);
        assert!(!reports.title_needs_group("", start));
    }

    #[test]
    fn repeated_titles_are_reattributed_when_stale_or_inapplicable() {
        let start = Instant::now();
        let (first, second) = (Some(20), Some(30));
        let mut reports = Reports::default();
        reports.report_title("vim".into(), first, start);
        assert!(!reports.title_text_changes("vim"));
        assert!(
            !reports.title_needs_group("vim", at(start, 100)),
            "a program re-sending its title reads the group at most every 250 ms"
        );
        assert!(reports.title_needs_group("vim", at(start, 250)));
        // A second vim starts after the first exits and repeats its title.
        reports.probed(second, None);
        assert!(
            reports.title_needs_group("vim", at(start, 120)),
            "the recorded attribution no longer applies"
        );
        reports.report_title("vim".into(), second, at(start, 120));
        assert_eq!(reports.title().as_deref(), Some("vim"));
    }

    #[test]
    fn repeated_titles_after_job_control_input_read_the_group() {
        let start = Instant::now();
        let (shell, job) = (Some(20), Some(30));
        let mut reports = Reports::default();
        reports.probed(shell, None);
        reports.report_title("vim".into(), shell, start);
        // Enter starts a job that repeats the title before any probe.
        reports.job_control_input();
        assert!(reports.title_needs_group("vim", at(start, 10)));
        reports.report_title("vim".into(), job, at(start, 10));
        reports.probed(job, None);
        assert_eq!(reports.title().as_deref(), Some("vim"));
        assert!(
            !reports.title_needs_group("vim", at(start, 60)),
            "the probe settles the attribution"
        );
    }

    #[test]
    fn a_running_job_keeps_one_poll_despite_other_triggers() {
        let start = Instant::now();
        let mut schedule = ProbeSchedule::default();
        schedule.input(b"\r", start);
        schedule.probed(at(start, 50), true);
        assert_eq!(schedule.deadline(), Some(at(start, 500)));
        schedule.probed(at(start, 500), true);
        assert_eq!(schedule.deadline(), Some(at(start, 1_500)));
        schedule.input(b"\r", at(start, 700));
        schedule.probed(at(start, 750), true);
        schedule.probed(at(start, 1_200), true);
        assert_eq!(
            (schedule.triggers.len(), schedule.deadline()),
            (0, Some(at(start, 2_200))),
            "each probe replaces the poll instead of adding another"
        );
    }

    /// Kills a fixture's process group when the test ends, including on a
    /// failed assertion. A leaked child keeps the runner's output pipes open.
    #[cfg(unix)]
    struct GroupGuard(std::process::Child);

    #[cfg(unix)]
    impl Drop for GroupGuard {
        fn drop(&mut self) {
            if let Ok(group) = i32::try_from(self.0.id()) {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(group),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            let _ = self.0.wait();
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_idle_root_shell_that_execs_a_script_is_renamed() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let directory = std::env::temp_dir()
            .join(format!("huterm-exec-rename-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let script = directory.join("second-script");
        std::fs::write(&script, "echo second\nwhile :; do sleep 1; done\n")
            .unwrap();
        let mut child = GroupGuard(
            Command::new("/bin/sh")
                .arg("-c")
                .arg(format!(
                    "echo first; read line; exec /bin/sh '{}'",
                    script.display()
                ))
                .process_group(0)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut output = BufReader::new(child.0.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let root = child.0.id();
        let group = i32::try_from(root).ok();
        let idle = probe(group, Some(root));
        assert_eq!((idle.name, idle.job), (None, false));
        // Same PID, start time, and (on macOS) kernel name after the exec.
        std::io::Write::write_all(child.0.stdin.as_mut().unwrap(), b"go\n")
            .unwrap();
        line.clear();
        output.read_line(&mut line).unwrap();
        assert_eq!(line, "second\n");
        let job = probe(group, Some(root));
        assert_eq!(
            (job.name.as_deref(), job.job),
            (Some("second-script"), true)
        );
        drop(child);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[cfg(unix)]
    #[test]
    fn names_a_foreground_job_and_treats_the_root_shell_as_idle() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let mut child = GroupGuard(
            Command::new("/bin/sh")
                .args(["-c", "echo ready; exec sleep 30"])
                .process_group(0)
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut line = String::new();
        BufReader::new(child.0.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let pid = child.0.id();
        let group = i32::try_from(pid).ok();
        let deadline = Instant::now() + Duration::from_secs(5);
        let job = loop {
            let probe = probe(group, None);
            if probe.name.as_deref() == Some("sleep")
                || Instant::now() > deadline
            {
                break probe;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(job.name.as_deref(), Some("sleep"));
        assert!(job.job);
        assert_eq!(job.group, group);
        let expected = std::env::current_dir().unwrap().canonicalize().unwrap();
        let directory = job.directory.unwrap();
        assert!(directory.is_local());
        assert_eq!(
            std::path::Path::new(directory.path())
                .canonicalize()
                .unwrap(),
            expected
        );
        // The same process as root reads as a non-shell program, not idle.
        assert!(probe(group, Some(pid)).job);
        let unknown = probe(None, Some(pid));
        assert_eq!((unknown.name, unknown.job), (None, false));
        // An exited job's empty group can hold the terminal until the shell
        // reclaims it; keep polling until then.
        let empty = probe(Some(i32::MAX), Some(pid));
        assert_eq!((empty.name, empty.job), (None, true));
        assert!(unknown.directory.is_some(), "falls back to the root");
    }
}
