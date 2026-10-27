//! When the runtime compresses idle scrollback, as a pure function of
//! observed events and instants.
//!
//! A pass starts once history has stopped changing for [`IDLE`], counted from
//! the last change in Ghostty's activity token or the last operation that
//! restored compressed pages without changing it. Input postpones a pending
//! pass but never starts one: typing at a prompt changes no history. A pass
//! runs in batches of bounded steps, [`GAP`] apart, and a completed pass
//! leaves no deadline, so an idle terminal causes no wakeups.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use huterm_ghostty::Compression;

/// How long history must stay unchanged before a pass starts. Ghostty's own
/// application uses the same interval.
pub(crate) const IDLE: Duration = Duration::from_millis(250);
/// The longest a batch keeps stepping before it yields.
pub(crate) const BATCH: Duration = Duration::from_millis(1);
/// The pause between batches, so a pass uses about half a core.
pub(crate) const GAP: Duration = Duration::from_millis(1);

#[derive(Debug)]
pub(crate) struct CompressionSchedule {
    enabled: bool,
    /// False once Ghostty reports that this target cannot compress.
    supported: bool,
    /// The activity token at the last observation, or `None` when history
    /// must be reconsidered whatever the token says.
    activity: Option<u64>,
    deadline: Option<Instant>,
    idle: Duration,
    /// A pass has run a batch and not yet completed.
    in_pass: bool,
}

impl CompressionSchedule {
    pub(crate) const fn new(enabled: bool) -> Self {
        Self::with_idle(enabled, IDLE)
    }

    pub(crate) const fn with_idle(enabled: bool, idle: Duration) -> Self {
        Self {
            enabled,
            supported: true,
            activity: None,
            deadline: None,
            idle,
            in_pass: false,
        }
    }

    const fn active(&self) -> bool {
        self.enabled && self.supported
    }

    /// Replaces the idle interval and arms a pass, for tests that cannot
    /// wait for the real one.
    #[cfg(test)]
    pub(crate) fn set_idle(&mut self, idle: Duration, now: Instant) {
        self.idle = idle;
        self.activity = None;
        if self.active() {
            self.deadline = Some(now + idle);
        }
    }

    /// Applies the configuration. Enabling forgets the last token, so
    /// history restored while compression was off is reconsidered without
    /// waiting for new activity; repeating the current value changes
    /// nothing.
    pub(crate) fn set_enabled(&mut self, enabled: bool, now: Instant) {
        if enabled == self.enabled {
            return;
        }
        self.enabled = enabled;
        self.activity = None;
        self.deadline = None;
        self.in_pass = false;
        if self.active() {
            self.deadline = Some(now + self.idle);
        }
    }

    /// Records the current activity token. A change restarts the idle
    /// interval.
    pub(crate) fn observe(&mut self, activity: u64, now: Instant) {
        if self.active() && self.activity != Some(activity) {
            self.activity = Some(activity);
            self.deadline = Some(now + self.idle);
        }
    }

    /// Records an operation that restored compressed pages without changing
    /// the token, such as selection or link lookup.
    pub(crate) fn restored(&mut self, now: Instant) {
        if self.active() {
            self.deadline = Some(now + self.idle);
        }
    }

    /// Records input, which postpones a pending pass but never starts one.
    pub(crate) fn input(&mut self, now: Instant) {
        if let Some(deadline) = &mut self.deadline {
            *deadline = (*deadline).max(now + self.idle);
        }
    }

    /// The instant the next batch may run, if one is pending.
    pub(crate) const fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Whether a pass has started and not yet completed.
    pub(crate) const fn in_pass(&self) -> bool {
        self.in_pass
    }

    pub(crate) fn due(&self, now: Instant) -> bool {
        self.deadline.is_some_and(|deadline| deadline <= now)
    }

    /// Records the outcome of a batch.
    pub(crate) fn finished(&mut self, result: Compression, now: Instant) {
        self.in_pass = result == Compression::Pending;
        match result {
            Compression::Pending => self.deadline = Some(now + GAP),
            Compression::Complete => self.deadline = None,
            Compression::Unsupported => {
                self.supported = false;
                self.deadline = None;
            }
        }
    }
}

/// Running totals the runtime publishes for benchmarks, readable from any
/// thread without disturbing the runtime.
#[derive(Debug, Default)]
pub(crate) struct CompressionCounters {
    steps: AtomicU64,
    step_nanos: AtomicU64,
    inputs_during_pass: AtomicU64,
}

impl CompressionCounters {
    pub(crate) fn record_batch(&self, steps: u64, elapsed: Duration) {
        self.steps.fetch_add(steps, Ordering::Relaxed);
        self.step_nanos.fetch_add(
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    pub(crate) fn record_input_during_pass(&self) {
        self.inputs_during_pass.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn stats(&self) -> CompressionStats {
        CompressionStats {
            steps: self.steps.load(Ordering::Relaxed),
            step_time: Duration::from_nanos(
                self.step_nanos.load(Ordering::Relaxed),
            ),
            inputs_during_pass: self.inputs_during_pass.load(Ordering::Relaxed),
        }
    }
}

/// Totals of a terminal's idle compression since it started, for
/// benchmarks.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompressionStats {
    /// Incremental steps run.
    pub steps: u64,
    /// Time spent in those steps.
    pub step_time: Duration,
    /// Input messages handled while a pass was in progress, each of which
    /// may have waited for the step running when it arrived.
    pub inputs_during_pass: u64,
}

/// Steps until a pass finishes, the batch has run for `budget`, or work is
/// waiting, and returns the last step's result. `elapsed` reports the time
/// since the batch started.
pub(crate) fn run_batch<E>(
    mut step: impl FnMut() -> Result<Compression, E>,
    mut interrupted: impl FnMut() -> bool,
    mut elapsed: impl FnMut() -> Duration,
    budget: Duration,
) -> Result<Compression, E> {
    loop {
        let result = step()?;
        if result != Compression::Pending
            || interrupted()
            || elapsed() >= budget
        {
            return Ok(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const IDLE_TEST: Duration = Duration::from_millis(250);

    fn schedule() -> (CompressionSchedule, Instant) {
        (
            CompressionSchedule::with_idle(true, IDLE_TEST),
            Instant::now(),
        )
    }

    #[test]
    fn a_changed_token_restarts_the_idle_interval() {
        let (mut schedule, start) = schedule();
        schedule.observe(1, start);
        assert_eq!(schedule.deadline(), Some(start + IDLE_TEST));
        let later = start + Duration::from_millis(100);
        schedule.observe(2, later);
        assert_eq!(schedule.deadline(), Some(later + IDLE_TEST));
        assert!(!schedule.due(later + Duration::from_millis(249)));
        assert!(schedule.due(later + IDLE_TEST));
    }

    #[test]
    fn an_unchanged_token_after_a_completed_pass_schedules_nothing() {
        let (mut schedule, start) = schedule();
        schedule.observe(1, start);
        schedule.finished(Compression::Complete, start + IDLE_TEST);
        assert_eq!(schedule.deadline(), None);
        schedule.observe(1, start + IDLE_TEST * 2);
        assert_eq!(schedule.deadline(), None);
    }

    #[test]
    fn a_restore_without_a_token_change_starts_a_pass() {
        let (mut schedule, start) = schedule();
        schedule.observe(1, start);
        schedule.finished(Compression::Complete, start);
        let later = start + Duration::from_secs(5);
        schedule.restored(later);
        assert_eq!(schedule.deadline(), Some(later + IDLE_TEST));
    }

    #[test]
    fn input_postpones_a_pending_pass_but_never_starts_one() {
        let (mut schedule, start) = schedule();
        schedule.input(start);
        assert_eq!(schedule.deadline(), None, "input before any activity");

        schedule.observe(1, start);
        let typing = start + Duration::from_millis(200);
        schedule.input(typing);
        assert_eq!(schedule.deadline(), Some(typing + IDLE_TEST));

        // A pass in progress yields to typing the same way.
        schedule.finished(Compression::Pending, typing + IDLE_TEST);
        let more = typing + IDLE_TEST + Duration::from_micros(500);
        schedule.input(more);
        assert_eq!(schedule.deadline(), Some(more + IDLE_TEST));

        schedule.finished(Compression::Complete, more + IDLE_TEST);
        schedule.input(more + IDLE_TEST * 2);
        assert_eq!(schedule.deadline(), None, "input after a pass");
    }

    #[test]
    fn a_pending_pass_continues_after_the_gap() {
        let (mut schedule, start) = schedule();
        schedule.observe(1, start);
        let batch = start + IDLE_TEST;
        schedule.finished(Compression::Pending, batch);
        assert_eq!(schedule.deadline(), Some(batch + GAP));
    }

    #[test]
    fn a_pass_is_in_progress_from_its_first_pending_batch_until_it_ends() {
        let (mut schedule, start) = schedule();
        schedule.observe(1, start);
        assert!(!schedule.in_pass());
        schedule.finished(Compression::Pending, start + IDLE_TEST);
        assert!(schedule.in_pass());
        schedule.input(start + IDLE_TEST);
        assert!(schedule.in_pass(), "input postpones, it does not end it");
        schedule.finished(Compression::Complete, start + IDLE_TEST * 3);
        assert!(!schedule.in_pass());
        schedule.observe(2, start + IDLE_TEST * 3);
        schedule.finished(Compression::Pending, start + IDLE_TEST * 4);
        schedule.set_enabled(false, start + IDLE_TEST * 4);
        assert!(!schedule.in_pass());
    }

    #[test]
    fn an_unsupported_target_stops_scheduling() {
        let (mut schedule, start) = schedule();
        schedule.observe(1, start);
        schedule.finished(Compression::Unsupported, start + IDLE_TEST);
        schedule.observe(2, start + IDLE_TEST);
        schedule.restored(start + IDLE_TEST);
        assert_eq!(schedule.deadline(), None);
    }

    #[test]
    fn a_disabled_schedule_never_sets_a_deadline() {
        let start = Instant::now();
        let mut schedule = CompressionSchedule::with_idle(false, IDLE_TEST);
        schedule.observe(1, start);
        schedule.restored(start);
        schedule.input(start);
        assert_eq!(schedule.deadline(), None);
    }

    #[test]
    fn enabling_arms_a_pass_even_with_an_unchanged_token() {
        let (mut schedule, start) = schedule();
        schedule.observe(1, start);
        schedule.finished(Compression::Complete, start);

        let off = start + Duration::from_secs(1);
        schedule.set_enabled(false, off);
        assert_eq!(schedule.deadline(), None);
        schedule.observe(1, off);
        assert_eq!(schedule.deadline(), None);

        let on = off + Duration::from_secs(1);
        schedule.set_enabled(true, on);
        assert_eq!(schedule.deadline(), Some(on + IDLE_TEST));
        // The token is reconsidered, so it does not restart the interval
        // again once it has been seen.
        schedule.observe(1, on);
        assert_eq!(schedule.deadline(), Some(on + IDLE_TEST));
    }

    #[test]
    fn repeating_the_current_setting_changes_nothing() {
        let (mut schedule, start) = schedule();
        schedule.observe(1, start);
        schedule.finished(Compression::Complete, start);
        schedule.set_enabled(true, start + IDLE_TEST);
        assert_eq!(schedule.deadline(), None);

        schedule.observe(2, start + IDLE_TEST);
        let deadline = schedule.deadline();
        schedule.set_enabled(true, start + IDLE_TEST * 3);
        assert_eq!(schedule.deadline(), deadline);
    }

    #[test]
    fn a_batch_stops_when_the_pass_ends() {
        let steps = Cell::new(0);
        let result = run_batch::<()>(
            || {
                steps.set(steps.get() + 1);
                Ok(if steps.get() < 3 {
                    Compression::Pending
                } else {
                    Compression::Complete
                })
            },
            || false,
            || Duration::ZERO,
            BATCH,
        );
        assert_eq!(result, Ok(Compression::Complete));
        assert_eq!(steps.get(), 3);
    }

    #[test]
    fn a_batch_yields_after_the_step_that_sees_queued_work() {
        let steps = Cell::new(0);
        let result = run_batch::<()>(
            || {
                steps.set(steps.get() + 1);
                Ok(Compression::Pending)
            },
            || steps.get() == 2,
            || Duration::ZERO,
            BATCH,
        );
        assert_eq!(result, Ok(Compression::Pending));
        assert_eq!(steps.get(), 2);
    }

    #[test]
    fn a_batch_that_starts_with_work_queued_runs_one_step() {
        // The wake flag can still be set by a message handled earlier in the
        // same loop pass, since only a wait clears it.
        let steps = Cell::new(0);
        let result = run_batch::<()>(
            || {
                steps.set(steps.get() + 1);
                Ok(Compression::Pending)
            },
            || true,
            || Duration::ZERO,
            BATCH,
        );
        assert_eq!(result, Ok(Compression::Pending));
        assert_eq!(steps.get(), 1);
    }

    #[test]
    fn a_batch_yields_after_the_step_that_crosses_its_budget() {
        let steps = Cell::new(0);
        let result = run_batch::<()>(
            || {
                steps.set(steps.get() + 1);
                Ok(Compression::Pending)
            },
            || false,
            || Duration::from_micros(400) * steps.get(),
            BATCH,
        );
        assert_eq!(result, Ok(Compression::Pending));
        assert_eq!(steps.get(), 3, "1.2 ms after the third step");
    }

    #[test]
    fn a_step_error_ends_the_batch() {
        let result = run_batch(
            || Err::<Compression, _>("poisoned"),
            || false,
            || Duration::ZERO,
            BATCH,
        );
        assert_eq!(result, Err("poisoned"));
    }
}
