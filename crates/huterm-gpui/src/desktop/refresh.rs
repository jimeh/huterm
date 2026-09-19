//! One frame registration per window, shared by all terminal views.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{AnyWindowHandle, App, WeakEntity, Window};
use huterm_config::RefreshMode;

use super::TerminalView;

#[derive(Debug)]
pub(super) struct SnapshotPacer {
    available: bool,
}

impl Default for SnapshotPacer {
    fn default() -> Self {
        Self { available: true }
    }
}

impl SnapshotPacer {
    pub(super) fn admits(&self, mode: RefreshMode) -> bool {
        mode == RefreshMode::Unlimited || self.available
    }

    pub(super) fn started(&mut self) {
        self.available = false;
    }
    pub(super) fn frame(&mut self) {
        self.available = true;
    }
}

pub(super) struct FrameClock {
    window: AnyWindowHandle,
    registered: Cell<bool>,
    waiting: RefCell<Vec<WeakEntity<TerminalView>>>,
}

impl FrameClock {
    pub(super) fn new(window: &Window) -> Rc<Self> {
        Rc::new(Self {
            window: window.window_handle(),
            registered: Cell::new(false),
            waiting: RefCell::new(Vec::new()),
        })
    }

    pub(super) fn schedule(
        self: &Rc<Self>,
        view: WeakEntity<TerminalView>,
        cx: &mut App,
    ) {
        let mut waiting = self.waiting.borrow_mut();
        if !waiting
            .iter()
            .any(|pending| pending.entity_id() == view.entity_id())
        {
            waiting.push(view);
        }
        drop(waiting);
        if self.registered.replace(true) {
            return;
        }
        let clock = Rc::downgrade(self);
        let window = self.window;
        // Snapshot admission is also called from view updates and input handlers.
        // Access the window only after those borrows have unwound.
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, _| {
                window.on_next_frame(move |_, cx| {
                    let Some(clock) = clock.upgrade() else {
                        return;
                    };
                    clock.registered.set(false);
                    let waiting = clock.waiting.take();
                    for view in waiting {
                        let _ = view.update(cx, |view, cx| {
                            view.snapshot_pacer.frame();
                            view.start_snapshot_if_needed(cx);
                        });
                    }
                });
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_admission_per_frame_with_immediate_first_update_after_idle() {
        let mut pacer = SnapshotPacer::default();
        assert!(pacer.admits(RefreshMode::Display));
        pacer.started();
        // Completion, another input, or another event cannot bypass the cap.
        assert!(!pacer.admits(RefreshMode::Display));
        pacer.frame();
        assert!(pacer.admits(RefreshMode::Display));
        pacer.started();
        assert!(!pacer.admits(RefreshMode::Display));
        pacer.frame();
        assert!(pacer.admits(RefreshMode::Display));
    }

    #[test]
    fn unlimited_reload_does_not_create_a_second_display_allowance() {
        let mut pacer = SnapshotPacer::default();
        pacer.started();
        assert!(pacer.admits(RefreshMode::Unlimited));
        pacer.started();
        assert!(!pacer.admits(RefreshMode::Display));
        pacer.frame();
        assert!(pacer.admits(RefreshMode::Display));
    }
}
