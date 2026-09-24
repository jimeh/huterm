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
    diagnostics: bool,
    fact_wakes: AtomicU64,
    frame_wakes: AtomicU64,
    raw_frames: AtomicU64,
}
impl Signal {
    pub fn new(sender: async_channel::Sender<()>) -> Self {
        Self(Arc::new(State {
            sender,
            flags: AtomicU8::new(DISPLAY),
            epoch: AtomicU64::new(0),
            diagnostics: std::env::var_os("HUTERM_QUAKE_SMOKE").is_some(),
            fact_wakes: AtomicU64::new(0),
            frame_wakes: AtomicU64::new(0),
            raw_frames: AtomicU64::new(0),
        }))
    }
    #[cfg(target_os = "macos")]
    pub fn raw_frame(&self) {
        if self.0.diagnostics {
            self.0.raw_frames.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn raw_frames(&self) -> u64 {
        self.0.raw_frames.load(Ordering::Relaxed)
    }
    pub fn counts(&self) -> (u64, u64) {
        (
            self.0.fact_wakes.load(Ordering::Relaxed),
            self.0.frame_wakes.load(Ordering::Relaxed),
        )
    }
    pub fn notify(&self, flags: u8) {
        if self.0.diagnostics {
            if flags & FRAME != 0 {
                self.0.frame_wakes.fetch_add(1, Ordering::Relaxed);
            }
            if flags & (CHANGED | DISPLAY) != 0 {
                self.0.fact_wakes.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.0.flags.fetch_or(flags, Ordering::AcqRel);
        let _ = self.0.sender.try_send(());
    }
    pub fn take(&self) -> u8 {
        self.0.flags.swap(0, Ordering::AcqRel)
    }
    pub fn current_flags(&self, captured: u8, epoch: u64) -> u8 {
        if epoch == self.epoch() {
            captured
        } else {
            captured & !FRAME
        }
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
    fn display_invalidation_rejects_already_taken_frame_flags() {
        let (sender, _) = async_channel::bounded(1);
        let signal = Signal::new(sender);
        signal.take();
        let epoch = signal.epoch();
        signal.frame(epoch);
        signal.notify(DISPLAY);
        let captured = signal.take();
        signal.invalidate_frames();
        assert_eq!(signal.current_flags(captured, epoch), DISPLAY);
    }
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
