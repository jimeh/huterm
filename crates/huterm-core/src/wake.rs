//! Coalesced wakeups for the runtime's separately bounded client, output,
//! and control queues.

use std::sync::{Arc, Condvar, Mutex, mpsc};

#[derive(Debug, Default)]
pub(crate) struct Wake {
    pending: Mutex<bool>,
    ready: Condvar,
}

impl Wake {
    pub(crate) fn notify(&self) {
        *self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.ready.notify_one();
    }

    /// Waits for a notification, or until `deadline` when one is given.
    pub(crate) fn wait_until(&self, deadline: Option<std::time::Instant>) {
        let Some(deadline) = deadline else {
            self.wait();
            return;
        };
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let timeout =
            deadline.saturating_duration_since(std::time::Instant::now());
        let mut pending = self
            .ready
            .wait_timeout_while(pending, timeout, |pending| !*pending)
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0;
        *pending = false;
    }

    pub(crate) fn wait(&self) {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        #[cfg(unix)]
        let mut pending = self
            .ready
            .wait_while(pending, |pending| !*pending)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        #[cfg(not(unix))]
        let mut pending = self
            .ready
            .wait_timeout_while(
                pending,
                std::time::Duration::from_millis(2),
                |pending| !*pending,
            )
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0;
        *pending = false;
    }
}

#[derive(Debug)]
pub(crate) struct Sender<T> {
    sender: mpsc::Sender<T>,
    pub(crate) wake: Arc<Wake>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            wake: Arc::clone(&self.wake),
        }
    }
}

impl<T> Sender<T> {
    pub(crate) fn new(sender: mpsc::Sender<T>, wake: Arc<Wake>) -> Self {
        Self { sender, wake }
    }
    pub(crate) fn send(&self, value: T) -> Result<(), mpsc::SendError<T>> {
        let result = self.sender.send(value);
        self.wake.notify();
        result
    }
}

#[derive(Debug)]
pub(crate) struct SyncSender<T> {
    sender: mpsc::SyncSender<T>,
    wake: Arc<Wake>,
}

impl<T> Clone for SyncSender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            wake: Arc::clone(&self.wake),
        }
    }
}

impl<T> SyncSender<T> {
    pub(crate) fn new(sender: mpsc::SyncSender<T>, wake: Arc<Wake>) -> Self {
        Self { sender, wake }
    }
    pub(crate) fn send(&self, value: T) -> Result<(), mpsc::SendError<T>> {
        let result = self.sender.send(value);
        self.wake.notify();
        result
    }
    pub(crate) fn try_send(
        &self,
        value: T,
    ) -> Result<(), mpsc::TrySendError<T>> {
        let result = self.sender.try_send(value);
        self.wake.notify();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn wake_is_retained_before_wait_and_after_worker_starts() {
        let wake = Arc::new(Wake::default());
        wake.notify();
        wake.wait();
        let (started, ready) = mpsc::channel();
        let (done, observed) = mpsc::channel();
        let waiting = Arc::clone(&wake);
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            waiting.wait();
            done.send(()).unwrap();
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        wake.notify();
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
    }
}
