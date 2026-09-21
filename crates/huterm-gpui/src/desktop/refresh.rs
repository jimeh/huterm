//! One frame registration per window, shared by all terminal views.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{AnyWindowHandle, App, WeakEntity, Window};
use huterm_config::RefreshMode;
use huterm_protocol::Viewport;

use crate::scroll::ScrollController;

use super::TerminalView;

// A viewport change may use one extra allowance instead of waiting behind an
// output snapshot started during native presentation. Only a delivered frame
// replenishes either allowance; stalled displays cannot accumulate requests.
#[derive(Debug)]
pub(super) struct SnapshotPacer {
    available: bool,
    scroll_available: bool,
}

impl Default for SnapshotPacer {
    fn default() -> Self {
        Self {
            available: true,
            scroll_available: true,
        }
    }
}

impl SnapshotPacer {
    pub(super) fn begin(
        &mut self,
        scroll: &mut ScrollController,
        visible: bool,
        mode: RefreshMode,
    ) -> Option<Viewport> {
        if !visible || !self.admits(mode, scroll.has_pending_scroll()) {
            return None;
        }
        let viewport = scroll.begin_request()?;
        self.started();
        Some(viewport)
    }

    fn admits(&self, mode: RefreshMode, scrolling: bool) -> bool {
        mode == RefreshMode::Unlimited
            || self.available
            || (scrolling && self.scroll_available)
    }

    fn started(&mut self) {
        // Normal credit goes first, so output without pending viewport work
        // cannot consume the extra allowance. A scroll snapshot includes output.
        if self.available {
            self.available = false;
        } else {
            self.scroll_available = false;
        }
    }
    pub(super) fn frame(&mut self) {
        self.available = true;
        self.scroll_available = true;
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
    fn output_admission_per_frame_with_immediate_first_update_after_idle() {
        let mut pacer = SnapshotPacer::default();
        assert!(pacer.admits(RefreshMode::Display, false));
        pacer.started();
        // Completion and another output event cannot bypass the output cap.
        assert!(!pacer.admits(RefreshMode::Display, false));
        pacer.frame();
        assert!(pacer.admits(RefreshMode::Display, false));
        pacer.started();
        assert!(!pacer.admits(RefreshMode::Display, false));
        pacer.frame();
        assert!(pacer.admits(RefreshMode::Display, false));
    }

    #[test]
    fn pending_scroll_can_start_after_output_but_a_third_request_waits() {
        let mut pacer = SnapshotPacer::default();
        let mut scroll = ScrollController::default();
        scroll.complete(Viewport { bottom_offset: 0 }, 100);
        scroll.invalidate();
        let output = pacer
            .begin(&mut scroll, true, RefreshMode::Display)
            .unwrap();
        scroll.scroll_rows(1);
        // An in-flight retry must preserve the pending viewport allowance.
        assert!(
            pacer
                .begin(&mut scroll, true, RefreshMode::Display)
                .is_none()
        );
        scroll.complete(output, 100);
        scroll.invalidate();
        let viewport = pacer
            .begin(&mut scroll, true, RefreshMode::Display)
            .expect("pending scroll uses its additional allowance");
        assert_eq!(viewport.bottom_offset, 1);
        scroll.complete(viewport, 100);
        scroll.scroll_rows(1);
        for _ in 0..10 {
            scroll.invalidate();
            assert!(
                pacer
                    .begin(&mut scroll, true, RefreshMode::Display)
                    .is_none()
            );
        }
        pacer.frame();
        assert_eq!(
            pacer
                .begin(&mut scroll, true, RefreshMode::Display)
                .unwrap()
                .bottom_offset,
            2
        );
    }

    #[test]
    fn repeated_scrolls_do_not_let_output_use_the_viewport_allowance() {
        let mut pacer = SnapshotPacer::default();
        let mut scroll = ScrollController::default();
        scroll.complete(Viewport { bottom_offset: 0 }, 100);
        scroll.scroll_rows(1);
        let first = pacer
            .begin(&mut scroll, true, RefreshMode::Display)
            .unwrap();
        scroll.complete(first, 100);
        scroll.invalidate();
        assert!(
            pacer
                .begin(&mut scroll, true, RefreshMode::Display)
                .is_none()
        );
        scroll.scroll_rows(1);
        let second = pacer
            .begin(&mut scroll, true, RefreshMode::Display)
            .unwrap();
        scroll.complete(second, 100);
        scroll.scroll_rows(1);
        assert!(
            pacer
                .begin(&mut scroll, true, RefreshMode::Display)
                .is_none()
        );
    }

    #[test]
    fn hidden_and_idle_attempts_preserve_allowances_for_visible_work() {
        let mut pacer = SnapshotPacer::default();
        let mut scroll = ScrollController::default();
        scroll.complete(Viewport { bottom_offset: 0 }, 100);
        assert!(
            pacer
                .begin(&mut scroll, true, RefreshMode::Display)
                .is_none()
        );
        scroll.invalidate();
        assert!(
            pacer
                .begin(&mut scroll, false, RefreshMode::Display)
                .is_none()
        );
        let output = pacer
            .begin(&mut scroll, true, RefreshMode::Display)
            .unwrap();
        scroll.complete(output, 100);
        scroll.scroll_rows(1);
        for _ in 0..10 {
            assert!(
                pacer
                    .begin(&mut scroll, false, RefreshMode::Display)
                    .is_none()
            );
        }
        assert!(
            pacer
                .begin(&mut scroll, true, RefreshMode::Display)
                .is_some()
        );
        assert_eq!(scroll.diagnostics().requests_started, 2);
    }

    #[test]
    fn unlimited_reload_preserves_consumed_output_and_scroll_allowances() {
        let mut pacer = SnapshotPacer::default();
        pacer.started();
        assert!(!pacer.admits(RefreshMode::Display, false));
        assert!(pacer.admits(RefreshMode::Display, true));
        assert!(pacer.admits(RefreshMode::Unlimited, false));
        pacer.started();
        assert!(!pacer.admits(RefreshMode::Display, true));
        assert!(pacer.admits(RefreshMode::Unlimited, false));
        pacer.frame();
        assert!(pacer.admits(RefreshMode::Display, false));
    }
}
