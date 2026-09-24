//! Owned, coalesced foreground work with one reusable earliest deadline.
use gpui::{App, Task};
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;
use std::time::Instant;

#[derive(Clone)]
pub(crate) struct Wake(Rc<State>);
struct State {
    sender: async_channel::Sender<()>,
    deadline: Cell<Option<Instant>>,
    generation: Cell<u64>,
    timer: RefCell<Option<Task<()>>>,
    timer_fired: Option<Rc<dyn Fn()>>,
}

impl Wake {
    pub fn new(
        timer_fired: Option<Rc<dyn Fn()>>,
    ) -> (Self, async_channel::Receiver<()>) {
        let (sender, receiver) = async_channel::bounded(1);
        (
            Self(Rc::new(State {
                sender,
                deadline: Cell::new(None),
                generation: Cell::new(0),
                timer: RefCell::new(None),
                timer_fired,
            })),
            receiver,
        )
    }
    pub fn sender(&self) -> async_channel::Sender<()> {
        self.0.sender.clone()
    }
    pub fn signal(&self) {
        let _ = self.0.sender.try_send(());
    }
    pub fn stop(&self) {
        self.0.sender.close();
        self.0
            .generation
            .set(self.0.generation.get().wrapping_add(1));
        self.0.deadline.set(None);
        self.0.timer.borrow_mut().take();
    }
    fn timer_finished(&self, generation: u64) {
        if self.0.sender.is_closed() || self.0.generation.get() != generation {
            return;
        }
        self.0.deadline.set(None);
        if let Some(callback) = &self.0.timer_fired {
            callback();
        }
        self.signal();
    }
    pub fn armed(&self) -> bool {
        self.0.deadline.get().is_some()
    }
    pub fn arm(&self, deadline: Option<Instant>, cx: &App) {
        if self.0.sender.is_closed()
            || keep_timer(self.0.deadline.get(), deadline)
        {
            return;
        }
        self.0.timer.borrow_mut().take();
        let generation = self.0.generation.get().wrapping_add(1);
        self.0.generation.set(generation);
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
                        Wake(state).timer_finished(generation);
                    }
                }));
        }
    }
}

pub(crate) fn keep_timer(
    armed: Option<Instant>,
    requested: Option<Instant>,
) -> bool {
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
    #[test]
    fn canceled_or_replaced_timer_cannot_clear_current_deadline_or_wake() {
        let (wake, receiver) = Wake::new(None);
        let deadline = Instant::now();
        wake.0.generation.set(2);
        wake.0.deadline.set(Some(deadline));
        wake.timer_finished(1);
        assert!(receiver.is_empty());
        assert_eq!(wake.0.deadline.get(), Some(deadline));
        wake.timer_finished(2);
        assert!(receiver.try_recv().is_ok());
        assert!(!wake.armed());
        wake.stop();
        wake.timer_finished(3);
        wake.signal();
        assert!(receiver.try_recv().is_err());
    }
}
