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

    /// When the last probe ran.
    #[cfg(test)]
    pub(crate) fn last_probe(&self) -> Option<Instant> {
        self.last_probe
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
mod tests;
