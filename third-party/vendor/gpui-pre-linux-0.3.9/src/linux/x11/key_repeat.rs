//! Held-key detection for X11 key presses.
//!
//! With XKB detectable auto-repeat, the server stops sending the synthetic
//! release before each repeated press, so a press whose key is still down is
//! a repeat. Without it, every repeat arrives as a release and a fresh press,
//! and no press is reported as held.

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
