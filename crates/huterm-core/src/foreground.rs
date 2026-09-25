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
const MAX_DEADLINES: usize = 4;

/// When to probe, as a pure function of observed events and instants.
#[derive(Debug, Default)]
pub(crate) struct ProbeSchedule {
    deadlines: BTreeSet<Instant>,
    last_probe: Option<Instant>,
    last_output: Option<Instant>,
}

impl ProbeSchedule {
    /// Enter, interrupts, end-of-file, and suspends can change the job.
    pub(crate) fn input(&mut self, bytes: &[u8], now: Instant) {
        if bytes.iter().any(|byte| {
            matches!(byte, b'\r' | b'\n' | 0x03 | 0x04 | 0x1a | 0x1c)
        }) {
            self.arm(now + SETTLE);
            self.arm(now + FOLLOW_UP);
        }
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
        let first = *self.deadlines.first()?;
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
        self.deadlines.retain(|deadline| *deadline > now);
        if job {
            self.arm(now + JOB_POLL);
        }
    }

    pub(crate) fn stop(&mut self) {
        self.deadlines.clear();
    }

    fn arm(&mut self, at: Instant) {
        self.deadlines.insert(at);
        while self.deadlines.len() > MAX_DEADLINES {
            self.deadlines.pop_last();
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
    /// Whether a job holds the foreground and needs the slow poll.
    pub(crate) job: bool,
}

/// Names the foreground process, reusing the last name until the selected
/// process, its start time, or its kernel name changes.
#[derive(Debug, Default)]
pub(crate) struct ForegroundNames {
    cached: Option<(huterm_procinfo::Process, Option<String>)>,
}

impl ForegroundNames {
    pub(crate) fn probe(
        &mut self,
        group: Option<i32>,
        root: Option<u32>,
    ) -> Probe {
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
            self.cached = None;
            return Probe {
                group,
                name: None,
                directory,
                job: false,
            };
        };
        let display = match &self.cached {
            Some((cached, display)) if *cached == process => display.clone(),
            _ => huterm_procinfo::arguments(process.pid)
                .as_deref()
                .and_then(huterm_procinfo::display_name)
                .or_else(|| {
                    huterm_procinfo::display_name(std::slice::from_ref(
                        &process.name,
                    ))
                }),
        };
        let idle = Some(process.pid) == root
            && display.as_deref().is_none_or(huterm_procinfo::is_shell);
        self.cached = Some((process, display.clone()));
        Probe {
            group,
            name: display.filter(|_| !idle),
            directory,
            job: !idle,
        }
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

    /// Records an OSC 0/2 title from `foreground`. An empty title clears it.
    pub(crate) fn report_title(
        &mut self,
        title: String,
        foreground: Option<i32>,
    ) {
        self.foreground = foreground;
        self.title = (!title.is_empty()).then_some((foreground, title));
    }

    /// Whether `title` would change the recorded title's text.
    pub(crate) fn title_changes(&self, title: &str) -> bool {
        self.title
            .as_ref()
            .map_or(!title.is_empty(), |(_, current)| current != title)
    }

    pub(crate) fn probed(
        &mut self,
        foreground: Option<i32>,
        process_directory: Option<TerminalDirectory>,
    ) {
        self.foreground = foreground;
        self.process_directory = process_directory;
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
        assert!(schedule.deadlines.len() <= MAX_DEADLINES);
        assert_eq!(schedule.deadlines.first(), Some(&at(start, 50)));
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
        let (shell, job) = (Some(10), Some(20));
        let mut reports = Reports::default();
        reports.probed(shell, None);
        assert!(reports.title_changes("~/src"));
        reports.report_title("~/src".into(), shell);
        assert_eq!(reports.title().as_deref(), Some("~/src"));
        assert!(
            !reports.title_changes("~/src"),
            "repeats need no group read"
        );
        reports.probed(job, None);
        assert_eq!(reports.title(), None, "a job has not titled itself yet");
        reports.report_title("notes.txt - VIM".into(), job);
        assert_eq!(reports.title().as_deref(), Some("notes.txt - VIM"));
        reports.probed(shell, None);
        assert_eq!(reports.title(), None, "the job's title leaves with it");
        reports.report_title(String::new(), shell);
        assert_eq!(reports.title(), None);
        assert!(!reports.title_changes(""));
    }

    #[cfg(unix)]
    #[test]
    fn names_a_foreground_job_and_treats_the_root_shell_as_idle() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let mut child = Command::new("/bin/sh")
            .args(["-c", "echo ready; exec sleep 30"])
            .process_group(0)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let pid = child.id();
        let group = i32::try_from(pid).ok();
        let mut names = ForegroundNames::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        let job = loop {
            let probe = names.probe(group, None);
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
        assert!(names.probe(group, Some(pid)).job);
        let unknown = names.probe(None, Some(pid));
        assert_eq!((unknown.name, unknown.job), (None, false));
        assert!(unknown.directory.is_some(), "falls back to the root");
        let _ = child.kill();
        let _ = child.wait();
    }
}
