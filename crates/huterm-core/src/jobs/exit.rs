use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::{JobProcess, JobState, Process, process_table};

#[derive(Debug)]
enum EvidenceState {
    Pinned(u32),
    Idle,
    Retired,
}

/// The unreaped session leader pins this SID until an empty scan is sealed.
/// Only workers lock state. The parser publishes exit and consumes `reap_ready`.
#[derive(Debug)]
pub(crate) struct ExitEvidence {
    state: Mutex<EvidenceState>,
    exited: AtomicBool,
    reap_ready: AtomicBool,
}

impl ExitEvidence {
    pub(crate) fn new(root: Option<u32>) -> Self {
        Self {
            state: Mutex::new(
                root.map_or(EvidenceState::Retired, EvidenceState::Pinned),
            ),
            exited: AtomicBool::new(false),
            reap_ready: AtomicBool::new(false),
        }
    }

    pub(crate) fn exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    pub(crate) fn observe_exit(&self) {
        self.exited.store(true, Ordering::Release);
    }

    pub(crate) fn reap_ready(&self) -> bool {
        self.reap_ready.load(Ordering::Acquire)
    }

    pub(crate) fn reaped(&self) {
        self.reap_ready.store(false, Ordering::Release);
    }

    pub(crate) fn assess(&self, live: impl FnOnce() -> JobState) -> JobState {
        self.assess_with(live, |root| {
            // A retry handles a transient ps/getsid race without blessing an
            // incomplete scan. Both attempts stay on the assessment worker.
            retry_scan(|| {
                let deadline = Instant::now() + Duration::from_secs(1);
                if let Some(table) = process_table()
                    && let Some(jobs) = session_jobs(
                        &table,
                        root,
                        |pid| {
                            if Instant::now() >= deadline {
                                return None;
                            }
                            let pid = nix::unistd::Pid::from_raw(
                                i32::try_from(pid).ok()?,
                            );
                            nix::unistd::getsid(Some(pid))
                                .ok()
                                .map(nix::unistd::Pid::as_raw)
                        },
                        |pid| {
                            if Instant::now() >= deadline {
                                return None;
                            }
                            let pid = nix::unistd::Pid::from_raw(
                                i32::try_from(pid).ok()?,
                            );
                            nix::unistd::getpgid(Some(pid))
                                .ok()
                                .map(nix::unistd::Pid::as_raw)
                        },
                    )
                {
                    return Some(jobs);
                }
                None
            })
        })
    }

    fn assess_with(
        &self,
        live: impl FnOnce() -> JobState,
        scan: impl FnOnce(u32) -> JobState,
    ) -> JobState {
        let Ok(mut state) = self.state.lock() else {
            return JobState::Unknown;
        };
        match *state {
            EvidenceState::Idle => JobState::Idle,
            EvidenceState::Retired => JobState::Unknown,
            EvidenceState::Pinned(_) if !self.exited() => live(),
            EvidenceState::Pinned(root) => {
                let jobs = scan(root);
                if jobs == JobState::Idle {
                    *state = EvidenceState::Idle;
                    // Seal before the parser is allowed to reap and release SID.
                    self.reap_ready.store(true, Ordering::Release);
                }
                jobs
            }
        }
    }

    /// Called after the parser loop, before any destructive wait or signal.
    pub(crate) fn retire(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let idle = matches!(*state, EvidenceState::Idle);
        *state = EvidenceState::Retired;
        idle
    }
}

fn retry_scan(mut scan: impl FnMut() -> Option<JobState>) -> JobState {
    for _ in 0..2 {
        if let Some(jobs) = scan() {
            return jobs;
        }
    }
    JobState::Unknown
}

fn session_jobs(
    table: &[Process],
    root: u32,
    mut session_id: impl FnMut(u32) -> Option<i32>,
    mut group_id: impl FnMut(u32) -> Option<i32>,
) -> Option<JobState> {
    let root = i32::try_from(root).ok()?;
    let mut jobs = Vec::new();
    for process in table.iter().filter(|process| {
        process.pid != 0
            && !process.zombie
            && i32::try_from(process.pid).ok() != Some(root)
    }) {
        // ESRCH is incomplete evidence too: a disappearing parent may have
        // forked after enumeration. Never turn a failed lookup into Idle.
        if session_id(process.pid)? == root {
            if group_id(process.pid)? != process.group
                || session_id(process.pid)? != root
            {
                return None;
            }
            jobs.push(JobProcess {
                pid: process.pid,
                group: process.group,
                group_started: table
                    .iter()
                    .find(|leader| {
                        i32::try_from(leader.pid).ok() == Some(process.group)
                    })
                    .map(|leader| leader.started.clone()),
                foreground: false,
                identity: process.identity.clone(),
            });
        }
    }
    jobs.sort_by_key(|process| process.pid);
    Some(if jobs.is_empty() {
        JobState::Idle
    } else {
        JobState::Running(jobs)
    })
}

pub(crate) struct ExitWatcher {
    cancel: mpsc::Sender<()>,
    join: JoinHandle<()>,
}

impl ExitWatcher {
    pub(crate) fn start(evidence: Arc<ExitEvidence>) -> std::io::Result<Self> {
        let (cancel, receiver) = mpsc::channel();
        let join = thread::Builder::new()
            .name("huterm-exit-watch".into())
            .spawn(move || {
                let mut delay = Duration::from_millis(10);
                loop {
                    if receiver.try_recv().is_ok() {
                        break;
                    }
                    if evidence.assess(|| JobState::Unknown) == JobState::Idle {
                        break;
                    }
                    if receiver.recv_timeout(delay)
                        != Err(mpsc::RecvTimeoutError::Timeout)
                    {
                        break;
                    }
                    delay = (delay * 2).min(Duration::from_secs(1));
                }
            })?;
        Ok(Self { cancel, join })
    }

    pub(crate) fn stop(self, evidence: &ExitEvidence) -> bool {
        let _ = self.cancel.send(());
        let idle = evidence.retire();
        let _ = self.join.join();
        idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, group: i32) -> Process {
        Process {
            pid,
            group,
            parent: 1,
            zombie: false,
            tty: "?".into(),
            command: "helper".into(),
            started: "birth".into(),
            identity: "birth helper".into(),
        }
    }

    #[test]
    fn incomplete_scans_retry_once_and_watcher_cancellation_joins() {
        let mut attempts = 0;
        assert_eq!(
            retry_scan(|| {
                attempts += 1;
                (attempts == 2).then_some(JobState::Idle)
            }),
            JobState::Idle
        );
        assert_eq!(attempts, 2);
        attempts = 0;
        assert_eq!(
            retry_scan(|| {
                attempts += 1;
                None
            }),
            JobState::Unknown
        );
        assert_eq!(attempts, 2);
        let guard = Arc::new(ExitEvidence::new(None));
        guard.observe_exit();
        let watcher = ExitWatcher::start(Arc::clone(&guard)).unwrap();
        assert!(!watcher.stop(&guard));
        assert_eq!(
            guard.assess_with(|| panic!(), |_| panic!()),
            JobState::Unknown
        );
    }

    #[test]
    fn idle_is_sealed_before_reap_and_queued_guards_never_scan_reused_ids() {
        let guard = Arc::new(ExitEvidence::new(Some(10)));
        let queued = Arc::clone(&guard);
        guard.observe_exit();
        assert_eq!(
            guard.assess_with(|| panic!(), |_| JobState::Unknown),
            JobState::Unknown
        );
        assert!(!guard.reap_ready());
        assert_eq!(
            guard.assess_with(
                || panic!(),
                |_| {
                    assert!(!guard.reap_ready());
                    JobState::Idle
                }
            ),
            JobState::Idle
        );
        assert!(guard.reap_ready());
        guard.reaped();
        assert_eq!(
            queued.assess_with(|| panic!(), |_| panic!("lookup after reap")),
            JobState::Idle
        );
        assert!(guard.retire());
        assert_eq!(
            queued.assess_with(
                || panic!(),
                |_| panic!("lookup after retirement")
            ),
            JobState::Unknown
        );
    }

    #[test]
    fn session_scan_handles_kernel_rows_and_rejects_incomplete_or_retargeted_processes()
     {
        let table = [
            process(0, 0),
            process(2, 0),
            process(10, 10),
            process(11, 11),
        ];
        let state = session_jobs(
            &table,
            10,
            |pid| match pid {
                2 => Some(0),
                11 => Some(10),
                _ => panic!("kernel zero or root lookup"),
            },
            |_| Some(11),
        )
        .unwrap();
        assert!(
            matches!(state, JobState::Running(jobs) if jobs.len() == 1 && jobs[0].pid == 11)
        );
        assert!(session_jobs(&table, 10, |_| None, |_| None).is_none());
        assert!(
            session_jobs(&[process(11, 11)], 10, |_| Some(10), |_| Some(12))
                .is_none()
        );
        let mut calls = 0;
        assert!(
            session_jobs(
                &[process(11, 11)],
                10,
                |_| {
                    calls += 1;
                    Some(if calls == 1 { 10 } else { 20 })
                },
                |_| Some(11)
            )
            .is_none()
        );
    }

    #[test]
    fn retirement_waits_for_the_current_scan_then_blocks_later_lookups() {
        let guard = Arc::new(ExitEvidence::new(Some(10)));
        guard.observe_exit();
        let (entered, receiver) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let scan_guard = Arc::clone(&guard);
        let scan = thread::spawn(move || {
            scan_guard.assess_with(
                || panic!(),
                |_| {
                    entered.send(()).unwrap();
                    wait.recv().unwrap();
                    JobState::Unknown
                },
            )
        });
        receiver.recv().unwrap();
        let retire_guard = Arc::clone(&guard);
        let retire = thread::spawn(move || retire_guard.retire());
        release.send(()).unwrap();
        assert_eq!(scan.join().unwrap(), JobState::Unknown);
        assert!(!retire.join().unwrap());
        assert_eq!(
            guard.assess_with(|| panic!(), |_| panic!("retired guard scanned")),
            JobState::Unknown
        );
    }
}
