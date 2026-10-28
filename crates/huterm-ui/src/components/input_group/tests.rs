use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Entity, Focusable as _, Modifiers, TestAppContext, VisualTestContext, div,
    point, prelude::*, px,
};

use super::{Addon, InputGroup};
use crate::components::button::Button;
use crate::components::input::{Chrome, Input};
use crate::focus::ring_drawn;
use crate::testing::{a11y_labelled, host, tokens};

struct Probe {
    input: Entity<Input>,
    clears: Rc<Cell<usize>>,
}

/// A 200-point group: a "Size" text addon, the field, then a Clear button.
fn group(cx: &mut TestAppContext) -> (Probe, &mut VisualTestContext) {
    let input = cx.update(|cx| {
        cx.new(|cx| {
            Input::new("size", "Size", tokens(), cx)
                .with_chrome(Chrome::Grouped)
        })
    });
    let clears = Rc::new(Cell::new(0));
    let (drawn, counted) = (input.clone(), clears.clone());
    let (_, cx) = host(cx, move |_, _| {
        let counted = counted.clone();
        div()
            .w(px(200.0))
            .child(
                InputGroup::new("group", &drawn, tokens())
                    .leading(Addon::Text("Size".into()))
                    .trailing(Addon::Button(
                        Button::new("clear", "Clear", tokens()).on_click(
                            move |_, _| counted.set(counted.get() + 1),
                        ),
                    )),
            )
            .into_any_element()
    });
    (Probe { input, clears }, cx)
}

impl Probe {
    fn focused(&self, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| {
            self.input.read(cx).focus_handle(cx).is_focused(window)
        })
    }
}

/// The Size addon sits at the group's leading edge, the Clear button at
/// its trailing edge.
const ADDON: (f32, f32) = (12.0, 12.0);
const BUTTON: (f32, f32) = (186.0, 12.0);

#[gpui::test]
fn a_press_on_an_addon_focuses_the_field(cx: &mut TestAppContext) {
    let (probe, cx) = group(cx);
    cx.simulate_click(point(px(ADDON.0), px(ADDON.1)), Modifiers::none());
    cx.run_until_parked();
    assert!(probe.focused(cx));
    // A press on a button that takes no focus gives focus to the field.
    cx.update(gpui::Window::blur);
    cx.simulate_click(point(px(BUTTON.0), px(BUTTON.1)), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(probe.clears.get(), 1);
    assert!(probe.focused(cx));
}

#[gpui::test]
fn a_disabled_field_disables_the_whole_group(cx: &mut TestAppContext) {
    let (probe, cx) = group(cx);
    cx.update(|_, cx| {
        probe
            .input
            .update(cx, |input, cx| input.set_disabled(true, None, cx));
    });
    cx.run_until_parked();
    for (x, y) in [ADDON, BUTTON] {
        cx.simulate_click(point(px(x), px(y)), Modifiers::none());
        cx.run_until_parked();
    }
    assert!(!probe.focused(cx));
    assert_eq!(probe.clears.get(), 0);
    cx.update(|window, _| {
        assert_eq!(a11y_labelled(window, "Clear")["aria"]["disabled"], true);
    });
}

#[gpui::test]
fn the_group_rings_for_its_fields_keyboard_focus(cx: &mut TestAppContext) {
    let (probe, cx) = group(cx);
    let handle = cx.update(|_, cx| probe.input.read(cx).focus_handle(cx));
    cx.simulate_keystrokes("a");
    cx.update(|window, cx| handle.focus(window, cx));
    cx.run_until_parked();
    assert!(cx.update(|window, cx| ring_drawn(&handle, window, cx)));
}
