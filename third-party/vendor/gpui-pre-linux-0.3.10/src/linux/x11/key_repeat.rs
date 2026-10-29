//! Held-key detection for X11 key presses.
//!
//! With XKB detectable auto-repeat, the server stops sending the synthetic
//! release before each repeated press, so a press whose key is still down is
//! a repeat. Without it, every repeat arrives as a release and a fresh press,
//! and no press is reported as held.
//!
//! GPUI drops a release that a press of the same key follows within 20 ms,
//! taking it for a repeat's synthetic release. That holds only without
//! detectable auto-repeat; with it, every release is real and is kept, or a
//! fast fresh press would read as held.

/// The keycodes currently down, as this client last saw them.
#[derive(Debug)]
pub(super) struct HeldKeys {
    detectable: bool,
    down: [bool; 256],
}

impl HeldKeys {
    /// Tracks presses; `detectable` says whether the server agreed to
    /// detectable auto-repeat.
    pub(super) fn new(detectable: bool) -> Self {
        Self {
            detectable,
            down: [false; 256],
        }
    }

    /// Records a press of `keycode`, returning whether it repeats a press
    /// that has had no release.
    pub(super) fn press(&mut self, keycode: u8) -> bool {
        let was_down = std::mem::replace(&mut self.down[usize::from(keycode)], true);
        self.detectable && was_down
    }

    /// Whether to deliver a release that a press arrives right after, in the
    /// same event batch. Without detectable auto-repeat, a release of the
    /// same key within 20 ms of the press is a repeat's synthetic release.
    pub(super) fn keeps_release_before_press(
        &self,
        release_keycode: u8,
        release_time: u32,
        press_keycode: u8,
        press_time: u32,
    ) -> bool {
        self.detectable
            || release_keycode != press_keycode
            || press_time.saturating_sub(release_time) > 20
    }

    /// Records a release of `keycode`.
    pub(super) fn release(&mut self, keycode: u8) {
        self.down[usize::from(keycode)] = false;
    }

    /// Forgets every key, such as when the window loses keyboard focus and
    /// a release may go to another client.
    pub(super) fn clear(&mut self) {
        self.down = [false; 256];
    }
}
