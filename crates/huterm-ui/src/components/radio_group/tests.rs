use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AccessibleAction, FocusHandle, KeyUpEvent, Keystroke, SharedString,
    TestAppContext, VisualTestContext, Window, div, prelude::*, px,
};

use super::RadioGroup;
use crate::focus::ring_drawn;
use crate::roving::focused;
use crate::testing::{
    Activate, KEYS, a11y_labelled, a11y_node, a11y_send, bind_keys, host,
    route_moves, tokens,
};

const THEMES: [&str; 3] = ["Dark", "Light", "Solarized"];

struct Probe {
    radios: Vec<FocusHandle>,
    before: FocusHandle,
    picks: Rc<RefCell<Vec<SharedString>>>,
}

/// How a test group draws.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Rows,
    Cards,
    Disabled,
}

/// A focusable stop, then a group over [`THEMES`] with Light checked,
/// which routes the test keys as an owner does: every arrow, Home, and End
/// move and check, and Space checks the focused radio.
fn themes(
    cx: &mut TestAppContext,
    kind: Kind,
) -> (Probe, &mut VisualTestContext) {
    let radios: Vec<FocusHandle> =
        cx.update(|cx| THEMES.iter().map(|_| cx.focus_handle()).collect());
    let before = cx.update(|cx| cx.focus_handle().tab_stop(true));
    let picks = Rc::new(RefCell::new(Vec::new()));
    let checked = Rc::new(RefCell::new(SharedString::from(THEMES[1])));
    let (drawn, drawn_before, seen) =
        (radios.clone(), before.clone(), picks.clone());
    let (_, cx) = host(cx, move |_, _| {
        let pick = {
            let (seen, checked) = (seen.clone(), checked.clone());
            move |value: SharedString, window: &mut Window| {
                if *checked.borrow() != value {
                    seen.borrow_mut().push(value.clone());
                    *checked.borrow_mut() = value;
                }
                window.refresh();
            }
        };
        let current = checked.borrow().clone();
        let group = THEMES.iter().zip(&drawn).fold(
            RadioGroup::new("themes", "Theme", tokens()).key_context(KEYS),
            |group, (theme, focus)| {
                let group =
                    group.option(*theme, *theme, current == *theme, focus);
                if kind == Kind::Cards {
                    group.content(div().h(px(40.0)))
                } else {
                    group
                }
            },
        );
        let group = match kind {
            Kind::Rows => group,
            Kind::Cards => group.card(px(116.0)),
            Kind::Disabled => {
                group.disabled(Some("The file is invalid".into()))
            }
        };
        let group = if kind == Kind::Disabled {
            group
        } else {
            let pick = pick.clone();
            group.on_select(move |value, window, _| pick(value, window))
        };
        let activate = {
            let (pick, handles) = (pick.clone(), drawn.clone());
            move |_: &Activate, window: &mut Window, _: &mut gpui::App| {
                if let Some(index) = focused(&handles, window) {
                    pick(THEMES[index].into(), window);
                }
            }
        };
        let routed = route_moves(
            div().w(px(400.0)).child(group),
            &drawn,
            None,
            move |index, window, _| pick(THEMES[index].into(), window),
        )
        .on_action(activate);
        div()
            .child(div().id("before").track_focus(&drawn_before).size(px(10.0)))
            .child(routed)
            .into_any_element()
    });
    bind_keys(cx);
    (
        Probe {
            radios,
            before,
            picks,
        },
        cx,
    )
}

fn click(cx: &mut VisualTestContext, label: &'static str) {
    a11y_send(
        cx,
        |window| a11y_labelled(window, label),
        AccessibleAction::Click,
    );
}

fn is_focused(cx: &mut VisualTestContext, handle: &FocusHandle) -> bool {
    cx.update(|window, _| handle.is_focused(window))
}

fn picks(probe: &Probe) -> Vec<SharedString> {
    probe.picks.borrow().clone()
}

#[gpui::test]
fn a_group_reports_radio_buttons_and_their_state(cx: &mut TestAppContext) {
    let (_, cx) = themes(cx, Kind::Cards);
    cx.update(|window, _| {
        let group = &a11y_node(window, "themes")["aria"];
        assert_eq!(group["role"], "RadioGroup");
        assert_eq!(group["label"], "Theme");
        assert_eq!(group["size_of_set"], 3);
        let light = &a11y_labelled(window, "Light")["aria"];
        assert_eq!(light["role"], "RadioButton");
        assert_eq!(light["toggled"], "True");
        assert_eq!(light["position_in_set"], 1);
        let dark = &a11y_labelled(window, "Dark")["aria"];
        assert_eq!(dark["toggled"], "False");
        assert_eq!(dark["position_in_set"], 0);
    });
}

#[gpui::test]
fn a_pointer_click_checks_and_focuses_a_radio(cx: &mut TestAppContext) {
    let (probe, cx) = themes(cx, Kind::Cards);
    click(cx, "Solarized");
    assert_eq!(picks(&probe), ["Solarized"]);
    assert!(is_focused(cx, &probe.radios[2]));
    cx.update(|window, _| {
        let solarized = &a11y_labelled(window, "Solarized")["aria"];
        assert_eq!(solarized["toggled"], "True");
        assert_eq!(a11y_labelled(window, "Light")["aria"]["toggled"], "False");
    });
    // GPUI clicks a focused element on Enter's key-up; nothing routes
    // Enter here, so only that implicit click could check.
    cx.update(|window, cx| probe.radios[0].focus(window, cx));
    cx.simulate_keystrokes("enter");
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").expect("a valid keystroke"),
    });
    cx.run_until_parked();
    assert_eq!(picks(&probe), ["Solarized"]);
}

#[gpui::test]
fn every_arrow_moves_and_checks_wrapping(cx: &mut TestAppContext) {
    let (probe, cx) = themes(cx, Kind::Rows);
    cx.update(|window, cx| probe.radios[1].focus(window, cx));
    cx.simulate_keystrokes("down");
    assert!(is_focused(cx, &probe.radios[2]));
    cx.simulate_keystrokes("right");
    assert!(is_focused(cx, &probe.radios[0]));
    cx.simulate_keystrokes("left up");
    assert!(is_focused(cx, &probe.radios[1]));
    cx.simulate_keystrokes("end home");
    assert!(is_focused(cx, &probe.radios[0]));
    assert_eq!(
        picks(&probe),
        [
            "Solarized",
            "Dark",
            "Solarized",
            "Light",
            "Solarized",
            "Dark"
        ]
    );
}

#[gpui::test]
fn space_checks_the_focused_radio(cx: &mut TestAppContext) {
    let (probe, cx) = themes(cx, Kind::Rows);
    // Focus moved by Tab alone checks nothing.
    cx.update(|window, cx| probe.radios[2].focus(window, cx));
    cx.run_until_parked();
    assert!(probe.picks.borrow().is_empty());
    cx.simulate_keystrokes("space");
    assert_eq!(picks(&probe), ["Solarized"]);
}

#[gpui::test]
fn the_group_is_one_tab_stop_at_its_checked_radio(cx: &mut TestAppContext) {
    let (probe, cx) = themes(cx, Kind::Cards);
    cx.update(|window, cx| probe.before.focus(window, cx));
    cx.update(Window::focus_next);
    cx.run_until_parked();
    assert!(is_focused(cx, &probe.radios[1]));
    cx.update(Window::focus_next);
    cx.run_until_parked();
    assert!(is_focused(cx, &probe.before));
}

#[gpui::test]
fn keyboard_focus_rings_a_card_and_a_click_does_not(cx: &mut TestAppContext) {
    let (probe, cx) = themes(cx, Kind::Cards);
    cx.update(|window, cx| probe.radios[1].focus(window, cx));
    cx.simulate_keystrokes("right");
    cx.update(|window, cx| {
        assert!(ring_drawn(&probe.radios[2], window, cx));
        assert!(!ring_drawn(&probe.radios[1], window, cx));
    });
    click(cx, "Dark");
    assert!(is_focused(cx, &probe.radios[0]));
    assert!(!cx.update(|window, cx| ring_drawn(&probe.radios[0], window, cx)));
}

#[gpui::test]
fn a_disabled_group_neither_checks_nor_takes_focus(cx: &mut TestAppContext) {
    let (probe, cx) = themes(cx, Kind::Disabled);
    click(cx, "Dark");
    assert!(probe.picks.borrow().is_empty());
    assert!(!is_focused(cx, &probe.radios[0]));
    cx.update(|window, cx| probe.before.focus(window, cx));
    cx.update(Window::focus_next);
    cx.run_until_parked();
    assert!(is_focused(cx, &probe.before), "no radio is a tab stop");
    cx.update(|window, _| {
        let group = &a11y_node(window, "themes")["aria"];
        assert_eq!(group["disabled"], true);
        assert_eq!(group["description"], "The file is invalid");
        assert_eq!(a11y_labelled(window, "Dark")["aria"]["disabled"], true);
    });
}
