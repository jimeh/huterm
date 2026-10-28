use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AccessibleAction, Axis, FocusHandle, KeyUpEvent, Keystroke, SharedString,
    TestAppContext, VisualTestContext, Window, div, prelude::*, px,
};

use super::{Tabs, panel};
use crate::focus::ring_drawn;
use crate::testing::{
    KEYS, a11y_labelled, a11y_node, a11y_send, bind_keys, host, route_moves,
    tokens,
};

const SECTIONS: [&str; 3] = ["General", "Appearance", "Keybindings"];

struct Probe {
    tabs: Vec<FocusHandle>,
    before: FocusHandle,
    picks: Rc<RefCell<Vec<SharedString>>>,
}

/// A focusable stop, then a tab list over [`SECTIONS`] along `axis` with
/// General selected, which routes the test keys as an owner does, then
/// the selected section's panel.
fn sections(
    cx: &mut TestAppContext,
    axis: Axis,
    disabled: bool,
) -> (Probe, &mut VisualTestContext) {
    let tabs: Vec<FocusHandle> =
        cx.update(|cx| SECTIONS.iter().map(|_| cx.focus_handle()).collect());
    let before = cx.update(|cx| cx.focus_handle().tab_stop(true));
    let picks = Rc::new(RefCell::new(Vec::new()));
    let selected = Rc::new(RefCell::new(SharedString::from(SECTIONS[0])));
    let (drawn, drawn_before, seen) =
        (tabs.clone(), before.clone(), picks.clone());
    let (_, cx) = host(cx, move |_, _| {
        let pick = {
            let (seen, selected) = (seen.clone(), selected.clone());
            move |value: SharedString, window: &mut Window| {
                seen.borrow_mut().push(value.clone());
                *selected.borrow_mut() = value;
                window.refresh();
            }
        };
        let current = selected.borrow().clone();
        let list = SECTIONS.iter().zip(&drawn).fold(
            Tabs::new("sections", "Settings sections", tokens())
                .key_context(KEYS),
            |list, (section, focus)| {
                list.tab(*section, *section, current == *section, focus)
            },
        );
        let list = if axis == Axis::Vertical {
            list.vertical()
        } else {
            list
        };
        let list = if disabled {
            list.disabled(Some("The file is invalid".into()))
        } else {
            let pick = pick.clone();
            list.on_select(move |value, window, _| pick(value, window))
        };
        let routed = route_moves(
            div().w(px(200.0)).child(list),
            &drawn,
            Some(axis),
            move |index, window, _| pick(SECTIONS[index].into(), window),
        );
        div()
            .child(div().id("before").track_focus(&drawn_before).size(px(10.0)))
            .child(routed)
            .child(panel("panel", current.clone()).child(current))
            .into_any_element()
    });
    bind_keys(cx);
    (
        Probe {
            tabs,
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

fn focused(cx: &mut VisualTestContext, handle: &FocusHandle) -> bool {
    cx.update(|window, _| handle.is_focused(window))
}

fn picks(probe: &Probe) -> Vec<SharedString> {
    probe.picks.borrow().clone()
}

#[gpui::test]
fn a_tab_list_reports_its_tabs_and_panel(cx: &mut TestAppContext) {
    let (_, cx) = sections(cx, Axis::Vertical, false);
    cx.update(|window, _| {
        let list = &a11y_node(window, "sections")["aria"];
        assert_eq!(list["role"], "TabList");
        assert_eq!(list["label"], "Settings sections");
        assert_eq!(list["orientation"], "Vertical");
        assert_eq!(list["size_of_set"], 3);
        let general = &a11y_labelled(window, "General")["aria"];
        assert_eq!(general["role"], "Tab");
        assert_eq!(general["selected"], true);
        assert_eq!(general["position_in_set"], 0);
        let keys = &a11y_labelled(window, "Keybindings")["aria"];
        assert_eq!(keys["selected"], false);
        assert_eq!(keys["position_in_set"], 2);
        let panel = &a11y_node(window, "panel")["aria"];
        assert_eq!(panel["role"], "TabPanel");
        assert_eq!(panel["label"], "General");
    });
}

#[gpui::test]
fn a_pointer_click_selects_and_focuses_a_tab(cx: &mut TestAppContext) {
    let (probe, cx) = sections(cx, Axis::Vertical, false);
    click(cx, "Appearance");
    assert_eq!(picks(&probe), ["Appearance"]);
    assert!(focused(cx, &probe.tabs[1]));
    cx.update(|window, _| {
        assert_eq!(
            a11y_labelled(window, "Appearance")["aria"]["selected"],
            true
        );
        assert_eq!(a11y_node(window, "panel")["aria"]["label"], "Appearance");
    });
    // The selected tab reports no change.
    click(cx, "Appearance");
    assert_eq!(picks(&probe), ["Appearance"]);
    // GPUI clicks a focused element on Enter's key-up; nothing routes
    // Enter here, so only that implicit click could select.
    cx.update(|window, cx| probe.tabs[2].focus(window, cx));
    cx.simulate_keystrokes("enter");
    cx.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").expect("a valid keystroke"),
    });
    cx.run_until_parked();
    assert_eq!(picks(&probe), ["Appearance"]);
}

#[gpui::test]
fn vertical_arrows_home_and_end_move_and_select(cx: &mut TestAppContext) {
    let (probe, cx) = sections(cx, Axis::Vertical, false);
    cx.update(|window, cx| probe.tabs[0].focus(window, cx));
    cx.simulate_keystrokes("down");
    assert!(focused(cx, &probe.tabs[1]));
    assert_eq!(picks(&probe), ["Appearance"]);
    // The cross axis moves nothing.
    cx.simulate_keystrokes("right left");
    assert!(focused(cx, &probe.tabs[1]));
    // Moves wrap at the ends, and Home and End jump to them.
    cx.simulate_keystrokes("down down up end home");
    assert!(focused(cx, &probe.tabs[0]));
    assert_eq!(
        picks(&probe),
        [
            "Appearance",
            "Keybindings",
            "General",
            "Keybindings",
            "Keybindings",
            "General"
        ]
    );
}

#[gpui::test]
fn horizontal_tabs_move_on_left_and_right(cx: &mut TestAppContext) {
    let (probe, cx) = sections(cx, Axis::Horizontal, false);
    cx.update(|window, cx| probe.tabs[0].focus(window, cx));
    cx.simulate_keystrokes("down up");
    assert!(focused(cx, &probe.tabs[0]));
    cx.simulate_keystrokes("left");
    assert!(focused(cx, &probe.tabs[2]));
    cx.simulate_keystrokes("right");
    assert!(focused(cx, &probe.tabs[0]));
    assert_eq!(picks(&probe), ["Keybindings", "General"]);
    cx.update(|window, _| {
        assert_eq!(
            a11y_node(window, "sections")["aria"]["orientation"],
            "Horizontal"
        );
    });
}

#[gpui::test]
fn the_list_is_one_tab_stop_at_its_selection(cx: &mut TestAppContext) {
    let (probe, cx) = sections(cx, Axis::Vertical, false);
    click(cx, "Keybindings");
    cx.update(|window, cx| probe.before.focus(window, cx));
    cx.update(Window::focus_next);
    cx.run_until_parked();
    assert!(focused(cx, &probe.tabs[2]));
    // Past the selected tab, Tab leaves the list.
    cx.update(Window::focus_next);
    cx.run_until_parked();
    assert!(focused(cx, &probe.before));
}

#[gpui::test]
fn keyboard_focus_rings_the_focused_tab(cx: &mut TestAppContext) {
    let (probe, cx) = sections(cx, Axis::Vertical, false);
    cx.update(|window, cx| probe.tabs[0].focus(window, cx));
    cx.simulate_keystrokes("down");
    cx.update(|window, cx| {
        assert!(ring_drawn(&probe.tabs[1], window, cx));
        assert!(!ring_drawn(&probe.tabs[0], window, cx));
    });
}

#[gpui::test]
fn a_disabled_list_neither_selects_nor_takes_focus(cx: &mut TestAppContext) {
    let (probe, cx) = sections(cx, Axis::Vertical, true);
    click(cx, "Appearance");
    assert!(probe.picks.borrow().is_empty());
    assert!(!focused(cx, &probe.tabs[1]));
    // Nor is it a tab stop.
    cx.update(|window, cx| probe.before.focus(window, cx));
    cx.update(Window::focus_next);
    cx.run_until_parked();
    assert!(focused(cx, &probe.before));
    cx.update(|window, _| {
        let list = &a11y_node(window, "sections")["aria"];
        assert_eq!(list["disabled"], true);
        assert_eq!(list["description"], "The file is invalid");
        assert_eq!(
            a11y_labelled(window, "Appearance")["aria"]["disabled"],
            true
        );
    });
}
