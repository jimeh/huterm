//! Coalesced fullscreen work, independent of display frames.
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;
use std::time::Instant;

use gpui::{App, Task};

#[derive(Clone)]
pub(crate) struct Wake(Rc<State>);
struct State {
    sender: async_channel::Sender<()>,
    stopped: Cell<bool>,
    deadline: Cell<Option<Instant>>,
    timer: RefCell<Option<Task<()>>>,
    diagnostics: bool,
    passes: Cell<u64>,
    timer_fires: Cell<u64>,
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
        let (sender, receiver) = async_channel::bounded(1);
        Self {
            wake: Wake(Rc::new(State {
                sender,
                stopped: Cell::new(false),
                deadline: Cell::new(None),
                timer: RefCell::new(None),
                diagnostics: std::env::var_os("HUTERM_FULLSCREEN_SMOKE")
                    .is_some()
                    || std::env::var_os("HUTERM_FULLSCREEN_STATS").is_some(),
                passes: Cell::new(0),
                timer_fires: Cell::new(0),
            })),
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
    /// Sending only schedules the receiver; it never runs window code inline.
    pub fn signal(&self) {
        if !self.0.stopped.get() {
            let _ = self.0.sender.try_send(());
        }
    }
    pub fn stop(&self) {
        self.0.stopped.set(true);
        self.0.sender.close();
        self.0.deadline.set(None);
        self.0.timer.borrow_mut().take();
    }
    pub fn passed(&self) {
        if self.0.diagnostics {
            self.0.passes.set(self.0.passes.get() + 1);
        }
    }
    pub fn counters(&self) -> (u64, u64, bool) {
        (
            self.0.passes.get(),
            self.0.timer_fires.get(),
            self.0.deadline.get().is_some(),
        )
    }
    pub fn arm(&self, deadline: Option<Instant>, cx: &App) {
        if self.0.stopped.get() {
            return;
        }
        // An earlier armed deadline may harmlessly re-evaluate a later target.
        // Retaining it avoids timer churn under coalesced geometry events.
        if keep_timer(self.0.deadline.get(), deadline) {
            return;
        }
        self.0.timer.borrow_mut().take();
        self.0.deadline.set(deadline);
        if let Some(deadline) = deadline {
            let weak = Rc::downgrade(&self.0);
            let timer = cx
                .background_executor()
                .timer(deadline.saturating_duration_since(Instant::now()));
            *self.0.timer.borrow_mut() =
                Some(cx.foreground_executor().spawn(async move {
                    timer.await;
                    if let Some(state) = weak.upgrade() {
                        state.deadline.set(None);
                        if state.diagnostics {
                            state.timer_fires.set(state.timer_fires.get() + 1);
                        }
                        Wake(state).signal();
                    }
                }));
        }
    }
}

fn keep_timer(armed: Option<Instant>, requested: Option<Instant>) -> bool {
    match (armed, requested) {
        (Some(armed), Some(requested)) => armed <= requested,
        (None, None) => true,
        _ => false,
    }
}

/// A pass consumes its wake before reconciliation, preserving concurrent signals.
/// Every continuation crosses the same foreground executor boundary as native work.
pub(crate) async fn run<F: Future<Output = ()>>(
    wakes: async_channel::Receiver<()>,
    mut reconcile: impl FnMut() -> Option<bool>,
    mut later_turn: impl FnMut() -> F,
) {
    while wakes.recv().await.is_ok() {
        let Some(continuation) = reconcile() else {
            return;
        };
        if continuation || !wakes.is_empty() {
            later_turn().await;
        }
        if continuation {
            // The producer may already have filled the one-slot channel.
            // Reconcile directly on this later turn without discarding that wake.
            loop {
                if wakes.is_closed() {
                    return;
                }
                let Some(more) = reconcile() else {
                    return;
                };
                if !more {
                    break;
                }
                later_turn().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(wake.0.sender.is_closed());
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
