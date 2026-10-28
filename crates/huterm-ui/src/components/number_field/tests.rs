use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AccessibleAction, Context, Entity, FocusHandle, Focusable as _,
    IntoElement, Modifiers, Render, TestAppContext, VisualTestContext, Window,
    div, point, prelude::*, px,
};

use super::{Changed, Format, NumberField};
use crate::steps::Steps;
use crate::testing::{
    a11y_action, a11y_labelled, a11y_node, a11y_send, tokens,
};

/// The Quake width: greater than 0 and at most 1 in 5% steps, shown as a
/// percent.
fn quake() -> (Steps, Format) {
    (
        Steps::new(0.05, 1.0, 0.05),
        Format {
            scale: 100.0,
            decimals: 0,
        },
    )
}

/// A 200-point number field and a second focusable element to blur to.
struct Host {
    field: Entity<NumberField>,
    other: FocusHandle,
}

impl Render for Host {
    fn render(
        &mut self,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        div()
            .size(px(400.0))
            .child(crate::focus::tracker())
            .child(div().w(px(200.0)).child(self.field.clone()))
            .child(div().id("other").track_focus(&self.other).size(px(20.0)))
    }
}

struct Probe {
    field: Entity<NumberField>,
    other: FocusHandle,
    changes: Rc<RefCell<Vec<f64>>>,
}

fn number(
    cx: &mut TestAppContext,
    value: f64,
) -> (Probe, &mut VisualTestContext) {
    let (steps, format) = quake();
    let (host, cx) = cx.add_window_view(|window, cx| Host {
        field: cx.new(|cx| {
            let mut field = NumberField::new(
                "width",
                "Width",
                steps,
                format,
                tokens(),
                window,
                cx,
            );
            field.set_value(Some(value), cx);
            field
        }),
        other: cx.focus_handle(),
    });
    // Blur reaches focus listeners only in an active window.
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    let (field, other) = cx.update(|_, cx| {
        (host.read(cx).field.clone(), host.read(cx).other.clone())
    });
    let changes = Rc::new(RefCell::new(Vec::new()));
    let seen = changes.clone();
    cx.update(|_, cx| {
        cx.subscribe(&field, move |_, event: &Changed, _| {
            seen.borrow_mut().push(event.0);
        })
        .detach();
    });
    (
        Probe {
            field,
            other,
            changes,
        },
        cx,
    )
}

impl Probe {
    fn act(
        &self,
        cx: &mut VisualTestContext,
        act: impl FnOnce(&mut NumberField, &mut Context<'_, NumberField>),
    ) {
        cx.update(|_, cx| self.field.update(cx, act));
        cx.run_until_parked();
    }

    fn value(&self, cx: &mut VisualTestContext) -> Option<f64> {
        cx.update(|_, cx| self.field.read(cx).value())
    }

    fn text(&self, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| {
            self.field.read(cx).input().read(cx).text().to_owned()
        })
    }

    /// Replaces the field's text as typing over a full selection does.
    fn type_text(&self, text: &str, cx: &mut VisualTestContext) {
        let handle = cx.update(|_, cx| self.field.read(cx).focus_handle(cx));
        cx.update(|window, cx| handle.focus(window, cx));
        cx.run_until_parked();
        cx.update(|_, cx| {
            let input = self.field.read(cx).input().clone();
            input.update(cx, crate::components::input::Input::select_all);
        });
        cx.simulate_input(text);
        cx.run_until_parked();
    }

    fn changes(&self) -> Vec<f64> {
        self.changes.borrow().clone()
    }
}

#[gpui::test]
fn every_step_stays_in_the_range(cx: &mut TestAppContext) {
    // D7: Settings stepping added 1 and never clamped, so a Quake width or
    // height stepped out of (0, 1] into an error.
    let (probe, cx) = number(cx, 1.0);
    probe.act(cx, NumberField::increment);
    probe.act(cx, NumberField::page_up);
    assert_eq!(probe.value(cx), Some(1.0));
    assert!(probe.changes().is_empty(), "nothing changed at the maximum");
    probe.act(cx, NumberField::decrement);
    assert_eq!(probe.value(cx), Some(0.95));
    probe.act(cx, NumberField::to_min);
    probe.act(cx, NumberField::decrement);
    probe.act(cx, NumberField::page_down);
    assert_eq!(probe.value(cx), Some(0.05), "never 0");
    probe.act(cx, NumberField::page_up);
    assert_eq!(probe.value(cx), Some(0.55));
    probe.act(cx, NumberField::to_max);
    probe.act(cx, NumberField::page_down);
    assert_eq!(probe.value(cx), Some(0.5));
    assert_eq!(probe.text(cx), "50");
    assert_eq!(probe.changes(), [0.95, 0.05, 0.55, 1.0, 0.5]);
}

#[gpui::test]
fn typed_text_commits_and_a_bad_entry_restores_the_value(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = number(cx, 0.333);
    // Shown rounded, and focusing and committing it keeps its precision.
    assert_eq!(probe.text(cx), "33");
    probe.act(cx, NumberField::commit);
    assert_eq!(probe.value(cx), Some(0.333));
    probe.type_text("42", cx);
    assert_eq!(probe.value(cx), Some(0.333), "typing alone commits nothing");
    probe.act(cx, NumberField::commit);
    assert_eq!(probe.value(cx), Some(0.42));
    probe.type_text("abc", cx);
    probe.act(cx, NumberField::commit);
    assert_eq!(probe.text(cx), "42");
    probe.type_text("250", cx);
    probe.act(cx, NumberField::commit);
    assert_eq!(probe.value(cx), Some(1.0));
    assert_eq!(probe.text(cx), "100");
    probe.type_text("17", cx);
    probe.act(cx, NumberField::revert);
    assert_eq!(probe.text(cx), "100");
    // Leaving the field commits what was typed.
    probe.type_text("30", cx);
    cx.update(|window, cx| probe.other.focus(window, cx));
    cx.run_until_parked();
    assert_eq!(probe.value(cx), Some(0.3));
    assert_eq!(probe.changes(), [0.42, 1.0, 0.3]);
}

#[gpui::test]
fn the_steppers_step_and_disable_at_their_bound(cx: &mut TestAppContext) {
    let (probe, cx) = number(cx, 0.3);
    // A stepper commits typed text first, and focus stays in the field.
    probe.type_text("90", cx);
    let increase = |cx: &mut VisualTestContext| {
        a11y_send(
            cx,
            |window| a11y_labelled(window, "Increase"),
            AccessibleAction::Click,
        );
    };
    increase(cx);
    assert_eq!(probe.value(cx), Some(0.95));
    assert!(cx.update(|window, cx| {
        probe.field.read(cx).focus_handle(cx).is_focused(window)
    }));
    increase(cx);
    increase(cx);
    assert_eq!(probe.value(cx), Some(1.0));
    assert_eq!(probe.changes(), [0.9, 0.95, 1.0]);
    cx.update(|window, _| {
        assert_eq!(a11y_labelled(window, "Increase")["aria"]["disabled"], true);
        assert!(
            a11y_labelled(window, "Decrease")["aria"]["disabled"].is_null()
        );
    });
    // A pointer press on the decrement stepper, at the group's end.
    cx.simulate_click(point(px(162.0), px(12.0)), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(probe.value(cx), Some(0.95));
}

#[gpui::test]
fn disabled_and_read_only_fields_refuse_every_change(cx: &mut TestAppContext) {
    for read_only in [false, true] {
        let (probe, cx) = number(cx, 0.5);
        probe.act(cx, move |field, cx| {
            if read_only {
                field.set_read_only(true, cx);
            } else {
                field.set_disabled(true, None, cx);
            }
        });
        probe.act(cx, |field, cx| {
            field.increment(cx);
            field.decrement(cx);
            field.page_up(cx);
            field.page_down(cx);
            field.to_min(cx);
            field.to_max(cx);
            let input = field.input().clone();
            input.update(cx, |input, cx| input.set_text("90", cx));
            field.commit(cx);
        });
        a11y_action(cx, "width", AccessibleAction::Increment);
        a11y_send(
            cx,
            |window| a11y_labelled(window, "Increase"),
            AccessibleAction::Click,
        );
        assert_eq!(probe.value(cx), Some(0.5), "read-only: {read_only}");
        assert!(probe.changes().is_empty(), "read-only: {read_only}");
        cx.update(|window, _| {
            let aria = &a11y_node(window, "width")["aria"];
            let state = if read_only { "read_only" } else { "disabled" };
            assert_eq!(aria[state], true);
            assert_eq!(
                a11y_labelled(window, "Increase")["aria"]["disabled"],
                true
            );
        });
    }
}

#[gpui::test]
fn the_field_reports_a_spin_button(cx: &mut TestAppContext) {
    let (probe, cx) = number(cx, 0.5);
    cx.update(|window, _| {
        let aria = &a11y_node(window, "width")["aria"];
        assert_eq!(aria["role"], "SpinButton");
        assert_eq!(aria["label"], "Width");
        assert_eq!(aria["value"], "50");
        assert_eq!(aria["numeric_value"], 50.0);
        assert_eq!(aria["min_numeric_value"], 5.0);
        assert_eq!(aria["max_numeric_value"], 100.0);
        assert_eq!(aria["numeric_value_step"], 5.0);
    });
    a11y_action(cx, "width", AccessibleAction::Increment);
    assert_eq!(probe.value(cx), Some(0.55));
    a11y_action(cx, "width", AccessibleAction::Decrement);
    a11y_action(cx, "width", AccessibleAction::Decrement);
    assert_eq!(probe.value(cx), Some(0.45));
    cx.update(|window, _| {
        assert_eq!(a11y_node(window, "width")["aria"]["numeric_value"], 45.0);
    });
}

#[test]
fn formats_scale_round_and_parse_numbers() {
    let (_, percent) = quake();
    assert_eq!(percent.text(0.33), "33");
    assert_eq!(percent.parse(" 33 "), Some(0.33));
    assert_eq!(percent.parse("abc"), None);
    assert_eq!(percent.parse("inf"), None);
    let plain = Format::default();
    assert_eq!(plain.text(2.0), "2");
    assert_eq!(plain.text(0.5), "0.5");
    assert_eq!(plain.text(-0.001), "0");
}
