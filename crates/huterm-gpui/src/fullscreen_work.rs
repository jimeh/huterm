//! Fullscreen ownership and diagnostics around shared deferred work.
pub(crate) use crate::deferred_work::run;
use gpui::{App, Task};
use std::{cell::Cell, rc::Rc, time::Instant};

#[derive(Default)]
struct Counters {
    enabled: bool,
    passes: Cell<u64>,
    timers: Cell<u64>,
}
#[derive(Clone)]
pub(crate) struct Wake {
    inner: crate::deferred_work::Wake,
    counters: Rc<Counters>,
}
pub(crate) struct Work {
    pub wake: Wake,
    pub receiver: Option<async_channel::Receiver<()>>,
    pub task: Option<Task<()>>,
    /// Construction outcome, never inferred from later adapter removal.
    pub fallback: bool,
}
impl Work {
    pub fn new(fallback: bool) -> Self {
        let counters = Rc::new(Counters {
            enabled: std::env::var_os("HUTERM_FULLSCREEN_SMOKE").is_some()
                || std::env::var_os("HUTERM_FULLSCREEN_STATS").is_some(),
            ..Counters::default()
        });
        let observed = Rc::downgrade(&counters);
        let (inner, receiver) =
            crate::deferred_work::Wake::new(Some(Rc::new(move || {
                if let Some(counters) = observed.upgrade()
                    && counters.enabled
                {
                    counters.timers.set(counters.timers.get() + 1);
                }
            })));
        Self {
            wake: Wake { inner, counters },
            receiver: Some(receiver),
            task: None,
            fallback,
        }
    }
}
impl Drop for Work {
    fn drop(&mut self) {
        self.wake.stop();
    }
}
impl Wake {
    pub fn signal(&self) {
        self.inner.signal();
    }
    pub fn stop(&self) {
        self.inner.stop();
    }
    pub fn arm(&self, deadline: Option<Instant>, cx: &App) {
        self.inner.arm(deadline, cx);
    }
    pub fn passed(&self) {
        if self.counters.enabled {
            self.counters.passes.set(self.counters.passes.get() + 1);
        }
    }
    pub fn counters(&self) -> (u64, u64, bool) {
        (
            self.counters.passes.get(),
            self.counters.timers.get(),
            self.inner.armed(),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::deferred_work::keep_timer;
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    async fn later_turn() {
        let mut yielded = false;
        std::future::poll_fn(move |cx| {
            if yielded {
                Poll::Ready(())
            } else {
                yielded = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await;
    }

    #[test]
    fn driver_preserves_wakes_during_reconciliation_and_defers_continuations() {
        let mut work = Work::new(false);
        let passes = Cell::new(0);
        work.wake.signal();
        work.wake.signal(); // Coalesces before the receiver starts.
        let wake = work.wake.clone();
        let mut task = Box::pin(run(
            work.receiver.take().unwrap(),
            || {
                let pass = passes.get() + 1;
                passes.set(pass);
                if pass == 1 {
                    wake.signal();
                }
                Some(pass == 1)
            },
            later_turn,
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(task.as_mut().poll(&mut cx).is_pending());
        assert_eq!(passes.get(), 1, "continuation must leave the current poll");
        assert!(task.as_mut().poll(&mut cx).is_pending());
        assert_eq!(
            passes.get(),
            3,
            "deferred continuation and in-pass producer both progress"
        );
        assert!(task.as_mut().poll(&mut cx).is_pending());
        assert_eq!(passes.get(), 3, "settled work has no recurring pass");
        wake.signal();
        assert!(task.as_mut().poll(&mut cx).is_pending());
        assert_eq!(passes.get(), 4, "wake after parking is observed");
        wake.stop();
        assert!(task.as_mut().poll(&mut cx).is_ready());
        wake.signal();
        assert!(wake.inner.sender().is_closed());
    }

    #[test]
    fn close_cancels_a_continuation_already_waiting_for_its_turn() {
        let mut work = Work::new(false);
        let passes = Cell::new(0);
        work.wake.signal();
        let mut task = Box::pin(run(
            work.receiver.take().unwrap(),
            || {
                passes.set(passes.get() + 1);
                Some(true)
            },
            later_turn,
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(task.as_mut().poll(&mut cx).is_pending());
        assert_eq!(passes.get(), 1);
        work.wake.stop();
        assert!(task.as_mut().poll(&mut cx).is_ready());
        assert_eq!(passes.get(), 1);
    }

    #[test]
    fn dropping_owner_closes_receiver_even_with_retained_native_wake() {
        let mut work = Work::new(false);
        let wake = work.wake.clone();
        let receiver = work.receiver.take().unwrap();
        drop(work);
        wake.signal();
        assert!(receiver.is_closed());
        assert!(receiver.is_empty());
    }

    #[test]
    fn timer_reuses_earlier_deadline_but_cancels_idle_or_replaces_later_one() {
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        assert!(keep_timer(Some(now), Some(later)));
        assert!(!keep_timer(Some(later), Some(now)));
        assert!(!keep_timer(Some(now), None));
        assert!(!keep_timer(None, Some(now)));
        assert!(keep_timer(None, None));
    }
}
