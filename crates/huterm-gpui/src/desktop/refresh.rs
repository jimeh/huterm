//! One frame registration and earliest animation deadline per window.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use gpui::{AnyWindowHandle, App, Context, EntityId, Task, WeakEntity, Window};

use huterm_config::RefreshMode;
use huterm_protocol::Viewport;

use crate::scroll::ScrollController;
use crate::ui::animation::AnimationSchedule;

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

pub(super) trait Animated: Sized + 'static {
    fn animation_schedule(&self, now: Instant) -> AnimationSchedule;
    fn advance_animation(
        &mut self,
        now: Instant,
        frame: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    );
}

type AnimationTick =
    Rc<dyn Fn(Instant, bool, &mut Window, &mut App) -> AnimationSchedule>;

struct Animation {
    id: EntityId,
    schedule: AnimationSchedule,
    tick: AnimationTick,
}

// Counts owned native/deferred frame callbacks, independently of timer weak refs.
struct PendingFrame(Rc<Cell<usize>>);
impl Drop for PendingFrame {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

pub(super) struct FrameClock {
    window: AnyWindowHandle,
    registered: Cell<bool>,
    frame_callbacks: Rc<Cell<usize>>,
    waiting: RefCell<Vec<WeakEntity<TerminalView>>>,
    animations: RefCell<Vec<Animation>>,
    deadline: Cell<Option<Instant>>,
    timer: RefCell<Option<Task<()>>>,
}

impl FrameClock {
    pub(super) fn new(window: &Window) -> Rc<Self> {
        Rc::new(Self {
            window: window.window_handle(),
            registered: Cell::new(false),
            frame_callbacks: Rc::new(Cell::new(0)),
            waiting: RefCell::new(Vec::new()),
            animations: RefCell::new(Vec::new()),
            deadline: Cell::new(None),
            timer: RefCell::new(None),
        })
    }

    pub(super) fn pending_callbacks(&self) -> usize {
        self.frame_callbacks.get()
    }

    /// Entity notifications arm animation work even when its window cannot paint.
    pub(super) fn observe<V: Animated>(
        self: &Rc<Self>,
        cx: &mut Context<'_, V>,
    ) {
        let clock = Rc::downgrade(self);
        let id = cx.entity_id();
        cx.on_release(move |_, cx| {
            if let Some(clock) = clock.upgrade() {
                clock.animations.borrow_mut().retain(|entry| entry.id != id);
                clock.arm(cx);
            }
        })
        .detach();
        let clock = Rc::clone(self);
        cx.observe_self(move |view, cx| {
            clock.animate(
                cx.entity().downgrade(),
                view.animation_schedule(Instant::now()),
                cx,
            );
        })
        .detach();
        let clock = Rc::clone(self);
        let entity = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = entity.update(cx, |view, cx| {
                clock.animate(
                    cx.entity().downgrade(),
                    view.animation_schedule(Instant::now()),
                    cx,
                );
            });
        });
    }

    pub(super) fn animate<V: Animated>(
        self: &Rc<Self>,
        view: WeakEntity<V>,
        schedule: AnimationSchedule,
        cx: &mut App,
    ) {
        let id = view.entity_id();
        let mut animations = self.animations.borrow_mut();
        if schedule == AnimationSchedule::IDLE {
            animations.retain(|entry| entry.id != id);
        } else if let Some(entry) =
            animations.iter_mut().find(|entry| entry.id == id)
        {
            entry.schedule = schedule;
        } else {
            let tick = Rc::new(
                move |now, frame, window: &mut Window, cx: &mut App| {
                    view.update(cx, |view, cx| {
                        view.advance_animation(now, frame, window, cx);
                        view.animation_schedule(now)
                    })
                    .unwrap_or_default()
                },
            );
            animations.push(Animation { id, schedule, tick });
        }
        drop(animations);
        self.arm(cx);
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
        self.arm(cx);
    }

    fn arm(self: &Rc<Self>, cx: &mut App) {
        let frames = !self.waiting.borrow().is_empty()
            || self
                .animations
                .borrow()
                .iter()
                .any(|entry| entry.schedule.frame);
        if frames && !self.registered.replace(true) {
            self.frame_callbacks.set(self.frame_callbacks.get() + 1);
            let registration = PendingFrame(Rc::clone(&self.frame_callbacks));
            let clock = Rc::downgrade(self);
            let window = self.window;
            // Input and entity observers can run while the window is borrowed.
            cx.defer(move |cx| {
                let _ = window.update(cx, |_, window, _| {
                    window.on_next_frame(move |window, cx| {
                        drop(registration);
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
                        clock.tick(true, window, cx);
                    });
                });
            });
        }
        let next = self
            .animations
            .borrow()
            .iter()
            .filter_map(|entry| entry.schedule.deadline)
            .min();
        let next = deadline_to_arm(self.deadline.get(), next);
        if self.deadline.replace(next) == next {
            return;
        }
        self.timer.borrow_mut().take();
        if let Some(at) = next {
            let clock = Rc::downgrade(self);
            let window = self.window;
            *self.timer.borrow_mut() = Some(cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(at.saturating_duration_since(Instant::now()))
                    .await;
                let Some(clock) = clock.upgrade() else {
                    return;
                };
                clock.deadline.set(None);
                let _ = window
                    .update(cx, |_, window, cx| clock.tick(false, window, cx));
            }));
        }
    }

    fn tick(self: &Rc<Self>, frame: bool, window: &mut Window, cx: &mut App) {
        let now = Instant::now();
        let due: Vec<_> = self
            .animations
            .borrow()
            .iter()
            .filter(|entry| {
                (frame && entry.schedule.frame)
                    || entry.schedule.deadline.is_some_and(|at| at <= now)
            })
            .map(|entry| (entry.id, Rc::clone(&entry.tick)))
            .collect();
        for (id, tick) in due {
            let schedule = tick(now, frame, window, cx);
            if let Some(entry) = self
                .animations
                .borrow_mut()
                .iter_mut()
                .find(|entry| entry.id == id)
            {
                entry.schedule = schedule;
            }
        }
        self.animations
            .borrow_mut()
            .retain(|entry| entry.schedule != AnimationSchedule::IDLE);
        self.arm(cx);
    }
}

fn deadline_to_arm(
    armed: Option<Instant>,
    required: Option<Instant>,
) -> Option<Instant> {
    // Extending a hold can reuse an earlier wake. It will recheck current work
    // and arm the later deadline, avoiding one platform timer per input event.
    match (armed, required) {
        (Some(armed), Some(required)) => Some(armed.min(required)),
        (_, required) => required,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_extensions_reuse_the_pending_wake_but_earlier_work_preempts_it() {
        use std::time::Duration;
        let now = Instant::now();
        let first = now + Duration::from_secs(2);
        let mut armed = deadline_to_arm(None, Some(first));
        for offset in 1..100 {
            let extended = first + Duration::from_millis(offset);
            armed = deadline_to_arm(armed, Some(extended));
            assert_eq!(
                armed,
                Some(first),
                "extensions must reuse the pending timer"
            );
        }
        let earlier = now + Duration::from_millis(100);
        assert_eq!(deadline_to_arm(armed, Some(earlier)), Some(earlier));
        assert_eq!(deadline_to_arm(armed, None), None);
        // Once the old wake fires, the still-required later deadline is armed.
        let later = first + Duration::from_secs(1);
        assert_eq!(deadline_to_arm(None, Some(later)), Some(later));
    }

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
