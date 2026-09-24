//! Native producers publish coalesced facts without entering GPUI.
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};

pub(crate) const CHANGED: u8 = 1;
pub(crate) const DISPLAY: u8 = 2;
pub(crate) const FRAME: u8 = 4;

#[derive(Clone)]
pub(crate) struct Signal(Arc<State>);
struct State {
    sender: async_channel::Sender<()>,
    flags: AtomicU8,
    epoch: AtomicU64,
}
impl Signal {
    pub fn new(sender: async_channel::Sender<()>) -> Self {
        Self(Arc::new(State {
            sender,
            flags: AtomicU8::new(DISPLAY),
            epoch: AtomicU64::new(0),
        }))
    }
    pub fn notify(&self, flags: u8) {
        self.0.flags.fetch_or(flags, Ordering::AcqRel);
        let _ = self.0.sender.try_send(());
    }
    pub fn take(&self) -> u8 {
        self.0.flags.swap(0, Ordering::AcqRel)
    }
    pub fn epoch(&self) -> u64 {
        self.0.epoch.load(Ordering::Acquire)
    }
    pub fn invalidate_frames(&self) {
        self.0.epoch.fetch_add(1, Ordering::AcqRel);
        self.0.flags.fetch_and(!FRAME, Ordering::AcqRel);
    }
    #[cfg(any(target_os = "macos", test))]
    pub fn frame(&self, epoch: u64) {
        if self.epoch() == epoch {
            self.notify(FRAME);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn facts_coalesce_and_stale_clock_epochs_cannot_admit_frames() {
        let (sender, receiver) = async_channel::bounded(1);
        let signal = Signal::new(sender);
        signal.take();
        let old = signal.epoch();
        signal.notify(CHANGED);
        signal.notify(DISPLAY);
        signal.frame(old);
        assert_eq!(receiver.len(), 1);
        assert_eq!(signal.take(), CHANGED | DISPLAY | FRAME);
        receiver.try_recv().unwrap();
        signal.invalidate_frames();
        signal.frame(old);
        assert!(receiver.is_empty());
        signal.frame(signal.epoch());
        assert_eq!(signal.take(), FRAME);
    }
}
