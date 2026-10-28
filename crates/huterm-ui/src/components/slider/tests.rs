use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    AccessibleAction, FocusHandle, Modifiers, MouseButton, MouseDownEvent,
    MouseUpEvent, PlatformInput, TestAppContext, VisualTestContext, div, point,
    prelude::*, px,
};

use super::Slider;
use crate::focus::ring_drawn;
use crate::steps::Steps;
use crate::testing::{a11y_action, a11y_node, host, tokens};

struct Probe {
    focus: FocusHandle,
    changes: Rc<RefCell<Vec<f64>>>,
}

/// A 220-point Quake-width slider at `value` that takes the values it
/// reports, as a controlled owner does. Its track runs from x 7 to 213, the
/// thumb's radius in from each end, along y 12.
fn slider(
    cx: &mut TestAppContext,
    value: f64,
    build: impl Fn(Slider) -> Slider + 'static,
) -> (Probe, &mut VisualTestContext) {
    let focus = cx.update(|cx| cx.focus_handle());
    let changes = Rc::new(RefCell::new(Vec::new()));
    let value = Rc::new(Cell::new(value));
    let (drawn, seen) = (focus.clone(), changes.clone());
    let (_, cx) = host(cx, move |_, _| {
        let (seen, value) = (seen.clone(), value.clone());
        let steps = Steps::new(0.05, 1.0, 0.05);
        let slider =
            Slider::new("width", "Width", value.get(), steps, &drawn, tokens())
                .on_change(move |next, window, _| {
                    seen.borrow_mut().push(next);
                    value.set(next);
                    window.refresh();
                });
        div().w(px(220.0)).child(build(slider)).into_any_element()
    });
    (Probe { focus, changes }, cx)
}

/// The point `fraction` of the way along the track.
fn along(fraction: f32) -> gpui::Point<gpui::Pixels> {
    point(px(7.0 + 206.0 * fraction), px(12.0))
}

/// A move to `fraction` along the track with the left button held, as a
/// drag sends.
fn drag_to(cx: &mut VisualTestContext, fraction: f32) {
    cx.simulate_mouse_move(
        along(fraction),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    cx.run_until_parked();
}

impl Probe {
    fn changes(&self) -> Vec<f64> {
        self.changes.borrow().clone()
    }
}

#[gpui::test]
fn a_press_and_a_drag_pick_grid_values_in_the_range(cx: &mut TestAppContext) {
    let (probe, cx) = slider(cx, 0.5, |slider| slider);
    cx.simulate_mouse_down(along(0.25), MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    assert_eq!(probe.changes(), [0.3]);
    assert!(cx.update(|window, _| probe.focus.is_focused(window)));
    // Dragging past either end stops at the bound, never below the floor.
    for x in [250.0, 400.0, -50.0] {
        cx.simulate_mouse_move(
            point(px(x), px(30.0)),
            Some(MouseButton::Left),
            Modifiers::none(),
        );
        cx.run_until_parked();
    }
    assert_eq!(probe.changes(), [0.3, 1.0, 0.05]);
    cx.simulate_mouse_up(
        point(px(-50.0), px(30.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();
    // After the release, the pointer no longer moves it, even with the
    // button held.
    drag_to(cx, 0.9);
    assert_eq!(probe.changes(), [0.3, 1.0, 0.05]);
}

#[gpui::test]
fn disabling_a_slider_ends_its_drag(cx: &mut TestAppContext) {
    let disabled = Rc::new(Cell::new(false));
    let off = disabled.clone();
    let (probe, cx) = slider(cx, 0.5, move |slider| {
        if off.get() {
            slider.disabled(None)
        } else {
            slider
        }
    });
    let set_disabled = |cx: &mut VisualTestContext, value: bool| {
        disabled.set(value);
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    };
    // Released while disabled, then re-enabled.
    cx.simulate_mouse_down(along(0.25), MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    set_disabled(cx, true);
    cx.simulate_mouse_up(
        point(px(-50.0), px(30.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.run_until_parked();
    set_disabled(cx, false);
    drag_to(cx, 0.9);
    assert_eq!(probe.changes(), [0.3]);
    // Re-enabled before the release.
    cx.simulate_mouse_down(along(0.25), MouseButton::Left, Modifiers::none());
    cx.run_until_parked();
    set_disabled(cx, true);
    set_disabled(cx, false);
    drag_to(cx, 0.9);
    assert_eq!(probe.changes(), [0.3]);
}

#[gpui::test]
fn a_press_and_release_in_one_frame_ends_the_drag(cx: &mut TestAppContext) {
    let (probe, cx) = slider(cx, 0.5, |slider| slider);
    // Both events dispatch before a frame paints the window-level release
    // listener.
    cx.update(|window, cx| {
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: along(0.25),
                modifiers: Modifiers::none(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: along(0.25),
                modifiers: Modifiers::none(),
                click_count: 1,
            }),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(probe.changes(), [0.3]);
    drag_to(cx, 0.9);
    assert_eq!(probe.changes(), [0.3]);
}

#[gpui::test]
fn assistive_technology_steps_within_the_range(cx: &mut TestAppContext) {
    let (probe, cx) = slider(cx, 0.95, |slider| slider.readout("95%"));
    cx.update(|window, _| {
        let aria = &a11y_node(window, "width")["aria"];
        assert_eq!(aria["role"], "Slider");
        assert_eq!(aria["label"], "Width");
        assert_eq!(aria["value"], "95%");
        assert_eq!(aria["orientation"], "Horizontal");
        assert_eq!(aria["numeric_value"], 0.95);
        assert_eq!(aria["min_numeric_value"], 0.05);
        assert_eq!(aria["max_numeric_value"], 1.0);
        assert_eq!(aria["numeric_value_step"], 0.05);
    });
    a11y_action(cx, "width", AccessibleAction::Increment);
    a11y_action(cx, "width", AccessibleAction::Increment);
    assert_eq!(probe.changes(), [1.0], "nothing past the maximum");
    a11y_action(cx, "width", AccessibleAction::Decrement);
    assert_eq!(probe.changes(), [1.0, 0.95]);
}

#[gpui::test]
fn a_disabled_slider_neither_moves_nor_takes_focus(cx: &mut TestAppContext) {
    let (probe, cx) = slider(cx, 0.5, |slider| {
        slider.disabled(Some("Quake is off".into()))
    });
    cx.simulate_click(along(0.9), Modifiers::none());
    cx.run_until_parked();
    a11y_action(cx, "width", AccessibleAction::Increment);
    assert_eq!(probe.changes(), [] as [f64; 0]);
    assert!(!cx.update(|window, _| probe.focus.is_focused(window)));
    cx.update(|window, _| {
        let aria = &a11y_node(window, "width")["aria"];
        assert_eq!(aria["disabled"], true);
        assert_eq!(aria["description"], "Quake is off");
    });
}

#[gpui::test]
fn keyboard_focus_rings_the_thumb(cx: &mut TestAppContext) {
    let (probe, cx) = slider(cx, 0.5, |slider| slider);
    cx.simulate_keystrokes("a");
    cx.update(|window, cx| probe.focus.focus(window, cx));
    cx.run_until_parked();
    assert!(cx.update(|window, cx| ring_drawn(&probe.focus, window, cx)));
}
