use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    Entity, FocusHandle, KeyBinding, KeyDownEvent, KeyUpEvent, Keystroke,
    Modifiers, Subscription, TestAppContext, VisualTestContext, div, point,
    prelude::*, px,
};

use super::{CHORD_TIMEOUT, KeyRecorder, KeyRecorderButton, KeyRecorderEvent};
use crate::components::kbd::Glyph;
use crate::focus::ring_drawn;
use crate::testing::{Activate, KEYS, a11y_node, bind_keys, host, tokens};

gpui::actions!(key_recorder_test, [Bound]);

/// Where the button draws, and a point outside it.
const BUTTON: (f32, f32) = (20.0, 10.0);
const OUTSIDE: (f32, f32) = (300.0, 300.0);

struct Probe {
    recorder: Entity<KeyRecorder>,
    /// Another tab stop, to move focus to.
    other: FocusHandle,
    /// What the recorder reported.
    log: Rc<RefCell<Vec<String>>>,
    /// How often `ctrl-k` and `cmd-k`, bound outside the recorder, ran.
    bound: Rc<Cell<usize>>,
    _subscriptions: Vec<Subscription>,
}

impl Probe {
    fn recording(&self, cx: &mut VisualTestContext) -> bool {
        cx.update(|_, cx| self.recorder.read(cx).is_recording())
    }

    fn strokes(&self, cx: &mut VisualTestContext) -> Vec<String> {
        cx.update(|_, cx| self.recorder.read(cx).strokes().to_vec())
    }

    fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    fn start(&self, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            self.recorder
                .update(cx, |recorder, cx| recorder.start(window, cx));
        });
        cx.run_until_parked();
    }
}

/// A recorder of up to `max_strokes` strokes, its button showing `cmd-k`,
/// fed by an interceptor as an owner feeds it, beside another tab stop.
/// Space runs the activation command, which starts recording, and `ctrl-k`
/// and `cmd-k` are bound outside the recorder.
fn recorder(
    cx: &mut TestAppContext,
    max_strokes: usize,
    disabled: bool,
) -> (Probe, &mut VisualTestContext) {
    let focus = cx.update(|cx| cx.focus_handle());
    let other = cx.update(|cx| cx.focus_handle().tab_stop(true));
    let recorder = cx.new(|_| {
        KeyRecorder::new(focus, |shortcut| {
            vec![Glyph::Text(shortcut.to_uppercase().into())]
        })
        .with_max_strokes(max_strokes)
    });
    let bound = Rc::new(Cell::new(0));
    let (drawn, beside, counted) =
        (recorder.clone(), other.clone(), bound.clone());
    let (_, cx) = host(cx, move |_, _| {
        let start = drawn.clone();
        let counted = counted.clone();
        let button =
            KeyRecorderButton::new("recorder", "Shortcut", &drawn, tokens())
                .shortcut(Some("cmd-k"))
                .key_context(KEYS)
                .w(px(96.0));
        div()
            .on_action(move |_: &Bound, _, _| counted.set(counted.get() + 1))
            .child(
                div()
                    .on_action(move |_: &Activate, window, cx| {
                        start.update(cx, |recorder, cx| {
                            recorder.start(window, cx);
                        });
                    })
                    .child(if disabled {
                        button.disabled(Some("The file is invalid".into()))
                    } else {
                        button
                    }),
            )
            .child(div().track_focus(&beside).size(px(20.0)))
            .into_any_element()
    });
    // Focus events, which blur cancels on, need an active window.
    cx.update(|window, _| window.activate_window());
    bind_keys(cx);
    cx.update(|_, cx| {
        cx.bind_keys([
            KeyBinding::new("ctrl-k", Bound, None),
            KeyBinding::new("cmd-k", Bound, None),
        ]);
    });
    let log = Rc::new(RefCell::new(Vec::new()));
    let subscriptions = cx.update(|_, cx| {
        let logged = log.clone();
        let reported = cx.subscribe(&recorder, move |_, event, _| {
            logged.borrow_mut().push(match event {
                KeyRecorderEvent::Started => "started".to_owned(),
                KeyRecorderEvent::Recorded(keys) => format!("recorded {keys}"),
                KeyRecorderEvent::Canceled => "canceled".to_owned(),
            });
        });
        let fed = recorder.clone();
        let interceptor = cx.intercept_keystrokes(move |event, _, cx| {
            if fed.update(cx, |recorder, cx| recorder.intercept(event, cx)) {
                cx.stop_propagation();
            }
        });
        vec![reported, interceptor]
    });
    let probe = Probe {
        recorder,
        other,
        log,
        bound,
        _subscriptions: subscriptions,
    };
    (probe, cx)
}

fn key_down(cx: &mut VisualTestContext, key: &str, is_held: bool) {
    cx.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse(key).expect("a valid keystroke"),
        is_held,
        prefer_character_input: false,
    });
}

fn key_up(cx: &mut VisualTestContext, key: &str) {
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse(key).expect("a valid keystroke"),
    });
}

fn click(cx: &mut VisualTestContext, (x, y): (f32, f32)) {
    cx.simulate_click(point(px(x), px(y)), Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn interceptors_see_whether_a_key_down_is_held(cx: &mut TestAppContext) {
    let (_, cx) = host(cx, |_, _| div().into_any_element());
    let seen = Rc::new(RefCell::new(Vec::new()));
    let _interceptor = cx.update(|_, cx| {
        let seen = seen.clone();
        cx.intercept_keystrokes(move |event, _, _| {
            seen.borrow_mut()
                .push((event.keystroke.unparse(), event.is_held));
        })
    });
    key_down(cx, "a", false);
    key_down(cx, "a", true);
    assert_eq!(
        seen.borrow().as_slice(),
        [("a".to_owned(), false), ("a".to_owned(), true)]
    );
}

#[gpui::test]
fn a_held_key_records_one_stroke(cx: &mut TestAppContext) {
    let (probe, cx) = recorder(cx, 2, false);
    probe.start(cx);
    key_down(cx, "ctrl-k", false);
    for _ in 0..3 {
        key_down(cx, "ctrl-k", true);
    }
    assert_eq!(probe.strokes(cx), ["ctrl-k"]);
    // The repeats reached no binding either.
    assert_eq!(probe.bound.get(), 0);
    cx.executor().advance_clock(CHORD_TIMEOUT);
    cx.run_until_parked();
    assert_eq!(probe.log(), ["started", "recorded ctrl-k"]);
}

#[gpui::test]
fn a_fast_double_press_of_the_same_key_records_a_chord(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = recorder(cx, 2, false);
    probe.start(cx);
    key_down(cx, "ctrl-k", false);
    key_up(cx, "ctrl-k");
    key_down(cx, "ctrl-k", false);
    assert_eq!(probe.log(), ["started", "recorded ctrl-k ctrl-k"]);
}

#[gpui::test]
fn a_held_cmd_chord_then_a_fresh_press_of_it_records_both_strokes(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = recorder(cx, 2, false);
    probe.start(cx);
    key_down(cx, "cmd-k", false);
    key_down(cx, "cmd-k", true);
    key_down(cx, "cmd-k", true);
    key_up(cx, "cmd-k");
    assert_eq!(probe.strokes(cx), ["cmd-k"]);
    key_down(cx, "cmd-k", false);
    assert_eq!(probe.log(), ["started", "recorded cmd-k cmd-k"]);
    assert_eq!(probe.bound.get(), 0);
}

#[gpui::test]
fn a_held_key_acts_nowhere_after_recording_until_a_fresh_press(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = recorder(cx, 1, false);
    probe.start(cx);
    key_down(cx, "ctrl-k", false);
    assert_eq!(probe.log(), ["started", "recorded ctrl-k"]);
    // The key stays down: its repeats neither record again nor reach the
    // binding the key has outside the recorder.
    key_down(cx, "ctrl-k", true);
    key_down(cx, "ctrl-k", true);
    assert!(!probe.recording(cx));
    assert_eq!(probe.bound.get(), 0);
    key_up(cx, "ctrl-k");
    key_down(cx, "ctrl-k", false);
    assert_eq!(probe.bound.get(), 1);
    assert_eq!(probe.log(), ["started", "recorded ctrl-k"]);
}

#[gpui::test]
fn a_click_or_the_activation_command_starts_recording(cx: &mut TestAppContext) {
    let (probe, cx) = recorder(cx, 2, false);
    click(cx, BUTTON);
    assert!(probe.recording(cx));
    assert!(cx.update(|window, cx| {
        probe.recorder.read(cx).focus_handle().is_focused(window)
    }));
    key_down(cx, "escape", false);
    assert!(!probe.recording(cx));
    // GPUI's implicit Enter click on key-up does not start it; nothing binds
    // Enter here.
    key_down(cx, "enter", false);
    key_up(cx, "enter");
    assert!(!probe.recording(cx));
    // The owner's activation command does.
    cx.simulate_keystrokes("space");
    assert!(probe.recording(cx));
    assert_eq!(probe.log(), ["started", "canceled", "started"]);
}

#[gpui::test]
fn escape_cancels_at_any_point(cx: &mut TestAppContext) {
    let (probe, cx) = recorder(cx, 2, false);
    probe.start(cx);
    key_down(cx, "escape", false);
    probe.start(cx);
    key_down(cx, "ctrl-k", false);
    key_down(cx, "escape", false);
    assert!(!probe.recording(cx));
    cx.executor().advance_clock(CHORD_TIMEOUT);
    cx.run_until_parked();
    assert_eq!(probe.log(), ["started", "canceled", "started", "canceled"]);
}

#[gpui::test]
fn return_saves_the_strokes_so_far_and_is_a_stroke_before_any(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = recorder(cx, 2, false);
    probe.start(cx);
    key_down(cx, "ctrl-k", false);
    key_down(cx, "enter", false);
    assert_eq!(probe.log(), ["started", "recorded ctrl-k"]);
    probe.start(cx);
    key_down(cx, "enter", false);
    assert_eq!(probe.strokes(cx), ["enter"]);
}

#[gpui::test]
fn the_chord_timeout_ends_a_recording_after_its_latest_stroke(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = recorder(cx, 2, false);
    probe.start(cx);
    key_down(cx, "ctrl-k", false);
    cx.executor().advance_clock(CHORD_TIMEOUT / 5);
    // Starting over leaves the first stroke's timer nothing to end.
    probe.start(cx);
    key_down(cx, "ctrl-j", false);
    cx.executor().advance_clock(CHORD_TIMEOUT * 9 / 10);
    cx.run_until_parked();
    assert_eq!(probe.strokes(cx), ["ctrl-j"]);
    cx.executor().advance_clock(CHORD_TIMEOUT / 10);
    cx.run_until_parked();
    assert_eq!(probe.log(), ["started", "started", "recorded ctrl-j"]);
    // A Space meant to save, just after the timeout saved, does not start
    // another recording; a later one does.
    cx.simulate_keystrokes("space");
    assert!(!probe.recording(cx));
    cx.executor().advance_clock(CHORD_TIMEOUT / 2);
    cx.simulate_keystrokes("space");
    assert!(probe.recording(cx));
}

#[gpui::test]
fn focus_leaving_the_button_cancels(cx: &mut TestAppContext) {
    let (probe, cx) = recorder(cx, 2, false);
    probe.start(cx);
    cx.update(|window, cx| probe.other.focus(window, cx));
    cx.run_until_parked();
    assert!(!probe.recording(cx), "blur cancels");
    assert_eq!(probe.log(), ["started", "canceled"]);
}

#[gpui::test]
fn window_deactivation_cancels(cx: &mut TestAppContext) {
    let (probe, cx) = recorder(cx, 2, false);
    probe.start(cx);
    cx.deactivate_window();
    assert!(!probe.recording(cx), "deactivation cancels");
    assert_eq!(probe.log(), ["started", "canceled"]);
}

#[gpui::test]
fn an_outside_press_cancels(cx: &mut TestAppContext) {
    let (probe, cx) = recorder(cx, 2, false);
    probe.start(cx);
    click(cx, OUTSIDE);
    assert!(!probe.recording(cx), "an outside press cancels");
    assert_eq!(probe.log(), ["started", "canceled"]);
}

#[gpui::test]
fn the_ring_shows_after_keyboard_focus_only(cx: &mut TestAppContext) {
    let (probe, cx) = recorder(cx, 2, false);
    let focus =
        cx.update(|_, cx| probe.recorder.read(cx).focus_handle().clone());
    cx.simulate_keystrokes("a");
    cx.update(|window, cx| focus.focus(window, cx));
    cx.run_until_parked();
    assert!(cx.update(|window, cx| ring_drawn(&focus, window, cx)));
    click(cx, BUTTON);
    assert!(cx.update(|window, _| focus.is_focused(window)));
    assert!(!cx.update(|window, cx| ring_drawn(&focus, window, cx)));
}

#[gpui::test]
fn the_button_reports_its_shortcut_and_while_recording_a_hint(
    cx: &mut TestAppContext,
) {
    let (probe, cx) = recorder(cx, 2, false);
    let aria = |cx: &mut VisualTestContext| {
        cx.update(|window, _| a11y_node(window, "recorder")["aria"].clone())
    };
    let idle = aria(cx);
    assert_eq!(idle["role"], "Button");
    assert_eq!(idle["label"], "Shortcut");
    assert_eq!(idle["value"], "CMD-K");
    assert!(idle["description"].is_null());
    probe.start(cx);
    let waiting = aria(cx);
    assert!(waiting["value"].is_null() || waiting["value"] == "");
    assert_eq!(
        waiting["description"],
        "Press the new shortcut. Esc cancels."
    );
    key_down(cx, "ctrl-j", false);
    let chord = aria(cx);
    assert_eq!(chord["value"], "CTRL-J");
    assert_eq!(
        chord["description"],
        "Saves in a moment. Press a second key for a chord, Return to save \
         now, or Esc to cancel."
    );
}

#[gpui::test]
fn a_disabled_button_neither_records_nor_takes_focus(cx: &mut TestAppContext) {
    let (probe, cx) = recorder(cx, 2, true);
    click(cx, BUTTON);
    assert!(!probe.recording(cx));
    assert!(!cx.update(|window, cx| {
        probe.recorder.read(cx).focus_handle().is_focused(window)
    }));
    cx.update(|window, _| {
        let aria = &a11y_node(window, "recorder")["aria"];
        assert_eq!(aria["disabled"], true);
        assert_eq!(aria["description"], "The file is invalid");
    });
}
