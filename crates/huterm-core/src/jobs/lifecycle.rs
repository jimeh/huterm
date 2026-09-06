use std::sync::atomic::{AtomicU8, Ordering};

use super::JobState;

const RUNNING: u8 = 0;
const EXITED: u8 = 1;
const RETIRED: u8 = 2;

/// Keeps queued assessments aware of exit and teardown without retaining PIDs.
#[derive(Debug, Default)]
pub(crate) struct JobLifecycle {
    state: AtomicU8,
}

impl JobLifecycle {
    pub(crate) fn running(&self) -> bool {
        self.state.load(Ordering::Acquire) == RUNNING
    }

    pub(crate) fn observe_exit(&self) {
        self.state.store(EXITED, Ordering::Release);
    }

    pub(crate) fn retire(&self) {
        self.state.store(RETIRED, Ordering::Release);
    }

    pub(crate) fn assess(&self, live: impl FnOnce() -> JobState) -> JobState {
        match self.state.load(Ordering::Acquire) {
            EXITED => JobState::Idle,
            RUNNING => {
                let jobs = live();
                // Exit or teardown may have overtaken the live assessment.
                match self.state.load(Ordering::Acquire) {
                    RUNNING => jobs,
                    EXITED => JobState::Idle,
                    _ => JobState::Unknown,
                }
            }
            _ => JobState::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exited_and_retired_contexts_never_inspect_old_process_ids() {
        let lifecycle = JobLifecycle::default();
        assert_eq!(lifecycle.assess(|| JobState::Unknown), JobState::Unknown);
        lifecycle.observe_exit();
        assert_eq!(
            lifecycle.assess(|| panic!("lookup after exit")),
            JobState::Idle
        );
        lifecycle.retire();
        assert_eq!(
            lifecycle.assess(|| panic!("lookup after retirement")),
            JobState::Unknown
        );
    }

    #[test]
    fn exit_and_retirement_override_an_in_flight_live_assessment() {
        let lifecycle = JobLifecycle::default();
        assert_eq!(
            lifecycle.assess(|| {
                lifecycle.observe_exit();
                JobState::Unknown
            }),
            JobState::Idle
        );
        let lifecycle = JobLifecycle::default();
        assert_eq!(
            lifecycle.assess(|| {
                lifecycle.retire();
                JobState::Idle
            }),
            JobState::Unknown
        );
    }
}
