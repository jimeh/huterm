//! Popover: a surface anchored to a trigger or the pointer, over the rest
//! of the window, such as a select's or a button's menu.
//!
//! Anatomy: content the owner supplies, placed by [`place`] below its
//! trigger (above it from the window's lower half), aligned to the
//! trigger's left edge (its right edge from the right half), clamped to the
//! window margin, and scrolling within the returned `max_height`. Content
//! draws through GPUI's `deferred` layer, above everything laid out in
//! normal flow; a modal popover also draws a transparent backdrop over the
//! whole window that occludes hover and wheel events beneath it.
//!
//! Dismissal: every close has a [`Dismissal`] reason, so the owner chooses
//! where focus goes. The owner mounts [`Popover::guard`] in its root view on
//! every frame. Its capture-phase mouse-down listener reads the live open
//! state, not the drawn frame, so it also catches a press that arrives
//! before the popover's first frame. For a modal popover, a press outside
//! the last painted content (or before any content has painted) dismisses
//! it and stops there: nothing under it takes focus or input, and a press
//! on the trigger closes instead of reopening. Releases are never stopped,
//! so a gesture that began before the popover opened still ends. A
//! non-modal popover closes on an outside press and lets the press proceed,
//! and on a wheel that moves outside its content, which still scrolls what
//! it is over (a trackpad touch that has not moved yet leaves it open),
//! as a browser's datalist and `NSComboBox`'s list do; a modal popover's
//! backdrop takes those wheels. Escape, Tab, a pick, focus leaving, and
//! window deactivation are the owner's to route; it calls
//! [`Popover::close`] and moves focus.
//!
//! Placement: an open popover's trigger probe asks for another frame when
//! the trigger paints somewhere new, as after a resize, so an owner that
//! places the content from [`Popover::trigger_bounds`] follows it.
//!
//! Accessibility: the popover reports no node of its own; its content does,
//! such as a menu or a listbox.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, DispatchPhase, IntoElement, MouseDownEvent,
    Pixels, Point, ScrollDelta, ScrollEvent, Size, Window, anchored, canvas,
    deferred, div, point, prelude::*, px,
};

use crate::focus;

/// Why a popover closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dismissal {
    /// Escape.
    Escape,
    /// Tab or Shift-Tab.
    Tab,
    /// An item or action in the content was picked.
    Pick,
    /// A press outside the content and the trigger.
    OutsidePress,
    /// A wheel outside a non-modal popover's content.
    OutsideWheel,
    /// A press on the trigger, which toggles the popover closed.
    TriggerPress,
    /// Focus left the popover and its trigger.
    FocusOut,
    /// The window deactivated.
    Deactivated,
    /// A dialog, the palette, or another popover opened.
    Superseded,
    /// The owner's state changed, such as a reload or a removed row.
    OwnerChanged,
}

#[derive(Debug, Default)]
struct State {
    anchor: Option<Anchor>,
    modal: bool,
    trigger: Option<Bounds<Pixels>>,
    content: Option<Bounds<Pixels>>,
}

/// One owner's popover: its open state and the bounds the guard tests.
/// Clones share the state.
#[derive(Clone, Debug, Default)]
pub struct Popover {
    state: Rc<RefCell<State>>,
}

impl Popover {
    /// A closed popover.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens at `anchor`. A `modal` popover draws a backdrop and stops
    /// outside presses; a non-modal one lets them proceed. The content's
    /// painted bounds clear, so the guard treats every press as outside
    /// until the content paints.
    pub fn open(&self, anchor: Anchor, modal: bool) {
        let mut state = self.state.borrow_mut();
        state.anchor = Some(anchor);
        state.modal = modal;
        state.content = None;
    }

    /// Closes; `false` when it was not open.
    #[must_use = "a toggle needs to know whether the popover was open"]
    pub fn close(&self) -> bool {
        let mut state = self.state.borrow_mut();
        state.content = None;
        state.anchor.take().is_some()
    }

    /// Whether it is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.state.borrow().anchor.is_some()
    }

    /// Where it is open from.
    #[must_use]
    pub fn anchor(&self) -> Option<Anchor> {
        self.state.borrow().anchor
    }

    /// The trigger's bounds from the last paint of [`Self::trigger_probe`].
    #[must_use]
    pub fn trigger_bounds(&self) -> Option<Bounds<Pixels>> {
        self.state.borrow().trigger
    }

    /// The open content's bounds from its last paint; `None` while closed
    /// or before the content first paints.
    #[must_use]
    pub fn content_bounds(&self) -> Option<Bounds<Pixels>> {
        self.state.borrow().content
    }

    /// An element that records the trigger's bounds as it paints, and
    /// asks for another frame when an open popover's trigger moved. Mount
    /// it inside the trigger, which must be relatively positioned. When
    /// several triggers share one popover, mount it only in the open one:
    /// the last to paint is the trigger the guard knows.
    #[must_use]
    pub fn trigger_probe(&self) -> impl IntoElement + use<> {
        let state = self.state.clone();
        canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let mut state = state.borrow_mut();
                let moved = state.trigger.replace(bounds) != Some(bounds);
                // A refresh is ignored while drawing, so it waits a frame.
                if moved && state.anchor.is_some() {
                    window.on_next_frame(|window, _| window.refresh());
                }
            },
        )
        .absolute()
        .inset_0()
    }

    /// `content` at `placement` in a window of `viewport` size, or nothing
    /// while closed.
    #[must_use]
    pub fn render(
        &self,
        placement: Placement,
        viewport: Size<Pixels>,
        content: impl IntoElement,
    ) -> Option<AnyElement> {
        let modal = {
            let state = self.state.borrow();
            state.anchor?;
            state.modal
        };
        let state = self.state.clone();
        let probe = canvas(
            |_, _, _| {},
            move |bounds, (), _, _| {
                state.borrow_mut().content = Some(bounds);
            },
        )
        .absolute()
        .inset_0();
        let content = div()
            .absolute()
            .left(placement.origin.x)
            .top(placement.origin.y)
            .child(content)
            .child(probe);
        let layer = div()
            .w(viewport.width)
            .h(viewport.height)
            .when(modal, gpui::InteractiveElement::occlude)
            .child(content);
        Some(
            deferred(anchored().position(point(px(0.0), px(0.0))).child(layer))
                .with_priority(1)
                .into_any_element(),
        )
    }

    /// The capture-phase press and wheel guard. Mount it in the owner's
    /// root view on every frame, open or not; `on_dismiss` receives the
    /// reason, and the owner closes the popover and moves focus.
    #[must_use]
    pub fn guard<F>(&self, on_dismiss: F) -> impl IntoElement + use<F>
    where
        F: Fn(Dismissal, &mut Window, &mut App) + 'static,
    {
        let state = self.state.clone();
        let on_dismiss = Rc::new(on_dismiss);
        canvas(
            |_, _, _| {},
            move |_, (), window, _| {
                let (wheel_state, wheel_dismiss) =
                    (state.clone(), on_dismiss.clone());
                let state = state.clone();
                let on_dismiss = on_dismiss.clone();
                window.on_mouse_event(
                    move |event: &MouseDownEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        let Some(reason) =
                            state.borrow().dismissal(event.position)
                        else {
                            return;
                        };
                        let modal = state.borrow().modal;
                        if modal {
                            // The focus tracker would miss a stopped press.
                            focus::note_pointer_press(window, cx);
                            cx.stop_propagation();
                            window.prevent_default();
                        }
                        on_dismiss(reason, window, cx);
                    },
                );
                window.on_mouse_event(
                    move |event: &ScrollEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        let outside = {
                            let state = wheel_state.borrow();
                            moves(event.delta)
                                && state.anchor.is_some()
                                && !state.modal
                                && !state.content.is_some_and(|content| {
                                    content.contains(&event.position)
                                })
                        };
                        if outside {
                            wheel_dismiss(Dismissal::OutsideWheel, window, cx);
                        }
                    },
                );
            },
        )
        .absolute()
        .size(px(0.0))
    }
}

impl State {
    /// The dismissal a press at `position` causes, if any.
    fn dismissal(&self, position: Point<Pixels>) -> Option<Dismissal> {
        self.anchor?;
        if self
            .content
            .is_some_and(|content| content.contains(&position))
        {
            return None;
        }
        let on_trigger = self
            .trigger
            .is_some_and(|trigger| trigger.contains(&position));
        match (self.modal, on_trigger) {
            (true, true) => Some(Dismissal::TriggerPress),
            // A non-modal popover's trigger, such as a combobox's field,
            // decides for itself.
            (false, true) => None,
            (_, false) => Some(Dismissal::OutsidePress),
        }
    }
}

/// Where a popover opens from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// The bounds of the control that opened it, in window coordinates.
    Trigger(Bounds<Pixels>),
    /// The pointer position for a context menu.
    Pointer(Point<Pixels>),
}

/// Where a popover's content draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Top-left corner in window coordinates.
    pub origin: Point<Pixels>,
    /// The menu extends above its anchor.
    pub opens_up: bool,
    /// The menu's right edge sits on the anchor's right edge.
    pub align_right: bool,
    /// The height beyond which the menu scrolls.
    pub max_height: Pixels,
}

/// Distance kept between a popover and the window edges.
pub const MARGIN: Pixels = px(6.0);
/// Gap between a button anchor and its menu.
const ANCHOR_GAP: Pixels = px(4.0);

/// Whether a wheel event scrolls anything: a trackpad reports a touch that
/// has not moved yet, and the end of a gesture, with no delta.
fn moves(delta: ScrollDelta) -> bool {
    match delta {
        ScrollDelta::Pixels(delta) => delta != Point::default(),
        ScrollDelta::Lines(delta) => delta.x != 0.0 || delta.y != 0.0,
    }
}

/// Places content of `menu` size in a window of `window` size. Trigger
/// anchors open below, or above from the lower half, and right-align from
/// the right half; `max_height` is the space left in the opening direction.
/// Pointer anchors open at the pointer, upward from the lower half, and
/// slide along the window to show every row: they scroll only when taller
/// than the window within its margins, which is then their `max_height`.
#[must_use]
pub fn place(
    anchor: Anchor,
    menu: Size<Pixels>,
    window: Size<Pixels>,
) -> Placement {
    let clamp = |value: Pixels, extent: Pixels, limit: Pixels| {
        value.min(limit - extent - MARGIN).max(MARGIN)
    };
    let (opens_up, align_right, x, top_edge, bottom_edge) = match anchor {
        Anchor::Trigger(bounds) => {
            let centre = bounds.center();
            let align_right = centre.x > window.width / 2.0;
            let x = if align_right {
                bounds.right() - menu.width
            } else {
                bounds.left()
            };
            (
                centre.y > window.height / 2.0,
                align_right,
                x,
                bounds.top() - ANCHOR_GAP,
                bounds.bottom() + ANCHOR_GAP,
            )
        }
        Anchor::Pointer(position) => {
            let opens_up = position.y > window.height / 2.0;
            let max_height = (window.height - MARGIN * 2.0).max(px(0.0));
            let height = menu.height.min(max_height);
            let y = if opens_up {
                position.y - height
            } else {
                position.y
            };
            return Placement {
                origin: point(
                    clamp(position.x, menu.width, window.width),
                    clamp(y, height, window.height),
                ),
                opens_up,
                align_right: false,
                max_height,
            };
        }
    };
    let max_height = if opens_up {
        top_edge - MARGIN
    } else {
        window.height - bottom_edge - MARGIN
    }
    .max(px(0.0));
    let height = menu.height.min(max_height);
    let y = if opens_up {
        top_edge - height
    } else {
        bottom_edge
    };
    Placement {
        origin: point(
            clamp(x, menu.width, window.width),
            clamp(y, height, window.height),
        ),
        opens_up,
        align_right,
        max_height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    fn window() -> Size<Pixels> {
        size(px(1000.0), px(600.0))
    }

    #[test]
    fn a_button_in_the_upper_left_opens_down_and_left_aligned() {
        let anchor =
            Bounds::new(point(px(20.0), px(4.0)), size(px(26.0), px(26.0)));
        let placement = place(
            Anchor::Trigger(anchor),
            size(px(264.0), px(300.0)),
            window(),
        );
        assert!(!placement.opens_up);
        assert!(!placement.align_right);
        assert_eq!(placement.origin, point(px(20.0), px(34.0)));
        assert_eq!(placement.max_height, px(600.0 - 34.0 - 6.0));
    }

    #[test]
    fn a_button_in_the_lower_right_opens_up_and_right_aligned() {
        let anchor =
            Bounds::new(point(px(960.0), px(570.0)), size(px(26.0), px(26.0)));
        let placement = place(
            Anchor::Trigger(anchor),
            size(px(264.0), px(300.0)),
            window(),
        );
        assert!(placement.opens_up);
        assert!(placement.align_right);
        assert_eq!(
            placement.origin,
            point(px(986.0 - 264.0), px(566.0 - 300.0))
        );
        assert_eq!(placement.max_height, px(560.0));
    }

    #[test]
    fn a_tall_menu_scrolls_within_the_available_space() {
        let anchor =
            Bounds::new(point(px(20.0), px(4.0)), size(px(26.0), px(26.0)));
        let placement = place(
            Anchor::Trigger(anchor),
            size(px(264.0), px(900.0)),
            window(),
        );
        assert_eq!(placement.max_height, px(560.0));
        assert_eq!(placement.origin.y, px(34.0));
        let below = place(
            Anchor::Trigger(Bounds::new(
                point(px(20.0), px(570.0)),
                size(px(26.0), px(26.0)),
            )),
            size(px(264.0), px(900.0)),
            window(),
        );
        assert!(below.opens_up);
        assert_eq!(below.max_height, px(560.0));
        assert_eq!(below.origin.y, px(6.0));
    }

    #[test]
    fn a_pointer_menu_clamps_to_the_window_margin() {
        let placement = place(
            Anchor::Pointer(point(px(990.0), px(590.0))),
            size(px(264.0), px(200.0)),
            window(),
        );
        assert!(placement.opens_up);
        assert!(!placement.align_right);
        assert_eq!(
            placement.origin,
            point(px(1000.0 - 264.0 - 6.0), px(390.0))
        );
        let top_left = place(
            Anchor::Pointer(point(px(2.0), px(100.0))),
            size(px(264.0), px(200.0)),
            window(),
        );
        assert!(!top_left.opens_up);
        assert_eq!(top_left.origin, point(px(6.0), px(100.0)));
        assert_eq!(top_left.max_height, px(588.0));
    }

    #[test]
    fn a_pointer_menu_slides_to_fit_rather_than_scroll() {
        // From the upper half it would pass the bottom margin, so it rises
        // above the pointer just far enough to show every row.
        let fitted = place(
            Anchor::Pointer(point(px(300.0), px(250.0))),
            size(px(264.0), px(400.0)),
            window(),
        );
        assert!(!fitted.opens_up);
        assert_eq!(fitted.origin.y, px(600.0 - 6.0 - 400.0));
        assert!(fitted.max_height >= px(400.0), "{fitted:?}");
        // From the lower half it would pass the top margin, so it drops.
        let dropped = place(
            Anchor::Pointer(point(px(300.0), px(350.0))),
            size(px(264.0), px(400.0)),
            window(),
        );
        assert!(dropped.opens_up);
        assert_eq!(dropped.origin.y, px(6.0));
        assert!(dropped.max_height >= px(400.0), "{dropped:?}");
        // Only a menu taller than the window scrolls, filling it.
        let tall = place(
            Anchor::Pointer(point(px(300.0), px(250.0))),
            size(px(264.0), px(900.0)),
            window(),
        );
        assert_eq!(tall.origin.y, px(6.0));
        assert_eq!(tall.max_height, px(588.0));
    }
}

#[cfg(all(test, feature = "test-support"))]
mod guard_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{
        Context, IntoElement, Modifiers, MouseButton, Render, ScrollDelta,
        ScrollEvent, TestAppContext, TouchPhase, VisualTestContext, Window,
        div, point, prelude::*, px,
    };

    use super::{Anchor, Placement, Popover};

    type Log = Rc<RefCell<Vec<String>>>;

    /// A trigger at the top left that opens the popover, a control lower
    /// down that the open popover's backdrop covers, and the popover's
    /// content just below the trigger.
    struct Owner {
        popover: Popover,
        log: Log,
    }

    impl Render for Owner {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut Context<'_, Self>,
        ) -> impl IntoElement {
            let log = self.log.clone();
            let entry = move |text: &'static str| {
                let log = log.clone();
                move || log.borrow_mut().push(text.to_owned())
            };
            let (trigger_click, under_down, under_click, content_click) = (
                entry("trigger-click"),
                entry("under-down"),
                entry("under-click"),
                entry("content-click"),
            );
            let popover = self.popover.clone();
            let opener = self.popover.clone();
            let log = self.log.clone();
            let placement = Placement {
                origin: point(px(0.0), px(30.0)),
                opens_up: false,
                align_right: false,
                max_height: px(100.0),
            };
            let content = div()
                .id("content")
                .w(px(100.0))
                .h(px(60.0))
                .on_click(move |_, _, _| content_click());
            div()
                .size(px(400.0))
                .relative()
                .child(popover.guard(move |reason, _, _| {
                    let _ = opener.close();
                    log.borrow_mut().push(format!("dismissed:{reason:?}"));
                }))
                .child(
                    div()
                        .id("trigger")
                        .relative()
                        .w(px(100.0))
                        .h(px(24.0))
                        .child(self.popover.trigger_probe())
                        .on_click(cx.listener(move |owner, _, _, cx| {
                            trigger_click();
                            owner.open();
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("under")
                        .absolute()
                        .top(px(200.0))
                        .w(px(100.0))
                        .h(px(40.0))
                        .on_mouse_down(MouseButton::Left, move |_, _, _| {
                            under_down();
                        })
                        .on_click(move |_, _, _| under_click()),
                )
                .children(self.popover.render(
                    placement,
                    window.viewport_size(),
                    content,
                ))
        }
    }

    impl Owner {
        fn open(&self) {
            self.popover
                .open(Anchor::Pointer(point(px(0.0), px(30.0))), true);
        }
    }

    fn owner(
        cx: &mut TestAppContext,
    ) -> (gpui::Entity<Owner>, Log, &mut VisualTestContext) {
        let log = Log::default();
        let shared = log.clone();
        let (owner, cx) = cx.add_window_view(move |_, _| Owner {
            popover: Popover::new(),
            log: shared,
        });
        cx.run_until_parked();
        (owner, log, cx)
    }

    fn click(cx: &mut VisualTestContext, x: f32, y: f32) {
        cx.simulate_click(point(px(x), px(y)), Modifiers::none());
        cx.run_until_parked();
    }

    fn open_and_draw(owner: &gpui::Entity<Owner>, cx: &mut VisualTestContext) {
        owner.update(cx, |owner, cx| {
            owner.open();
            cx.notify();
        });
        cx.run_until_parked();
    }

    fn taken(log: &Log) -> Vec<String> {
        std::mem::take(&mut *log.borrow_mut())
    }

    #[gpui::test]
    fn an_outside_press_closes_a_modal_popover_and_stops(
        cx: &mut TestAppContext,
    ) {
        let (owner, log, cx) = owner(cx);
        open_and_draw(&owner, cx);
        click(cx, 50.0, 220.0);
        assert_eq!(taken(&log), ["dismissed:OutsidePress"]);
        assert!(!owner.read_with(cx, |owner, _| owner.popover.is_open()));
    }

    #[gpui::test]
    fn a_wheel_outside_a_modal_popover_leaves_it_open(cx: &mut TestAppContext) {
        let (owner, log, cx) = owner(cx);
        open_and_draw(&owner, cx);
        cx.simulate_event(ScrollEvent {
            position: point(px(50.0), px(220.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-30.0))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        });
        cx.run_until_parked();
        assert_eq!(taken(&log), Vec::<String>::new());
        assert!(owner.read_with(cx, |owner, _| owner.popover.is_open()));
    }

    #[gpui::test]
    fn a_trigger_press_closes_rather_than_reopens(cx: &mut TestAppContext) {
        let (owner, log, cx) = owner(cx);
        click(cx, 50.0, 12.0);
        assert_eq!(taken(&log), ["trigger-click"]);
        assert!(owner.read_with(cx, |owner, _| owner.popover.is_open()));
        click(cx, 50.0, 12.0);
        assert_eq!(taken(&log), ["dismissed:TriggerPress"]);
        assert!(!owner.read_with(cx, |owner, _| owner.popover.is_open()));
    }

    #[gpui::test]
    fn a_press_inside_the_content_reaches_it(cx: &mut TestAppContext) {
        let (owner, log, cx) = owner(cx);
        open_and_draw(&owner, cx);
        click(cx, 50.0, 50.0);
        assert_eq!(taken(&log), ["content-click"]);
        assert!(owner.read_with(cx, |owner, _| owner.popover.is_open()));
    }

    #[gpui::test]
    fn a_press_before_the_first_frame_is_caught(cx: &mut TestAppContext) {
        let (owner, log, cx) = owner(cx);
        // Open without drawing: the last frame has no backdrop or content.
        owner.update(cx, |owner, _| owner.open());
        cx.simulate_mouse_down(
            point(px(50.0), px(220.0)),
            MouseButton::Left,
            Modifiers::none(),
        );
        assert_eq!(taken(&log), ["dismissed:OutsidePress"]);
        assert!(!owner.read_with(cx, |owner, _| owner.popover.is_open()));
        // Drawing and releasing does not act on the control underneath.
        cx.run_until_parked();
        cx.simulate_mouse_up(
            point(px(50.0), px(220.0)),
            MouseButton::Left,
            Modifiers::none(),
        );
        cx.run_until_parked();
        assert_eq!(taken(&log), Vec::<String>::new());
    }

    #[gpui::test]
    fn a_non_modal_popover_lets_the_press_proceed(cx: &mut TestAppContext) {
        let (owner, log, cx) = owner(cx);
        owner.update(cx, |owner, cx| {
            owner
                .popover
                .open(Anchor::Pointer(point(px(0.0), px(30.0))), false);
            cx.notify();
        });
        cx.run_until_parked();
        click(cx, 50.0, 220.0);
        assert_eq!(
            taken(&log),
            ["dismissed:OutsidePress", "under-down", "under-click"]
        );
    }

    /// A guard mounted before the focus tracker, whose capture listener
    /// then never sees a press the guard stops, and a focusable trigger.
    struct Guarded {
        popover: Popover,
        focus: gpui::FocusHandle,
    }

    impl Render for Guarded {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut Context<'_, Self>,
        ) -> impl IntoElement {
            let popover = self.popover.clone();
            div()
                .size(px(400.0))
                .child(self.popover.guard(move |_, _, _| {
                    let _ = popover.close();
                }))
                .child(crate::focus::tracker())
                .child(
                    div()
                        .id("trigger")
                        .track_focus(&self.focus)
                        .w(px(100.0))
                        .h(px(24.0)),
                )
        }
    }

    #[gpui::test]
    fn a_stopped_press_still_hides_a_keyboard_focus_ring(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| Guarded {
            popover: Popover::new(),
            focus: cx.focus_handle(),
        });
        cx.run_until_parked();
        let focus = view.read_with(cx, |view, _| view.focus.clone());
        cx.simulate_keystrokes("a");
        cx.update(|window, cx| focus.focus(window, cx));
        cx.run_until_parked();
        let visible = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| crate::focus::visible(&focus, window, cx))
        };
        assert!(visible(cx));
        view.update(cx, |view, cx| {
            view.popover
                .open(Anchor::Pointer(point(px(0.0), px(30.0))), true);
            cx.notify();
        });
        cx.run_until_parked();
        click(cx, 50.0, 220.0);
        assert!(!view.read_with(cx, |view, _| view.popover.is_open()));
        assert!(!visible(cx));
    }
}
