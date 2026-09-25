//! Foreground process names, probed on the terminal's own runtime thread.
//!
//! Probes run only after events that can change the foreground process, plus
//! a slow poll while a job holds the foreground. An idle shell arms no
//! deadline, so it causes no wakeups.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

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
    /// The name to publish, or `None` while the root shell is idle.
    pub(crate) name: Option<String>,
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
        let Some(process) = group
            .and_then(|group| u32::try_from(group).ok())
            .filter(|group| *group > 0)
            .and_then(selected_process)
        else {
            self.cached = None;
            return Probe {
                name: None,
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
            name: display.filter(|_| !idle),
            job: !idle,
        }
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
        assert_eq!(
            job,
            Probe {
                name: Some("sleep".into()),
                job: true
            }
        );
        // The same process as root reads as a non-shell program, not idle.
        assert!(names.probe(group, Some(pid)).job);
        assert_eq!(
            names.probe(None, Some(pid)),
            Probe {
                name: None,
                job: false
            }
        );
        let _ = child.kill();
        let _ = child.wait();
    }
}
