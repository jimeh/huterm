//! The terminal window's dialogs in its `huterm-ui` dialog slot, migration
//! 7b of `docs/plans/ui-components.md`: the close confirmation, About, and
//! Licenses stay open through outside presses and window deactivation,
//! keep Tab inside, and return focus to the terminal; Space presses the
//! close confirmation's and About's focused button; text typed or
//! composed on a Licenses button reaches its search; a long license
//! scrolls to its end.

use std::sync::Arc;

use gpui::{
    EntityInputHandler, InputHandler, Modifiers, ScrollDelta, ScrollWheelEvent,
    TouchPhase, point,
};

use super::buttons::CLOSE;
use super::*;
use crate::licenses::{Catalog, Component, LicenseText};

/// A point on the terminal at the window's lower left, outside any
/// dialog's panel.
fn beside_the_panel(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> gpui::Point<Pixels> {
    harness.update_view(window, |view, window, _| {
        let panel = view.dialog.panel_bounds().expect("the panel painted");
        let at = point(px(12.0), window.viewport_size().height - px(12.0));
        assert!(!panel.contains(&at), "{at:?} is on the panel {panel:?}");
        at
    })
}

/// A left click at `at` in `window`.
fn click(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    at: gpui::Point<Pixels>,
) {
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_click(at, Modifiers::none());
    harness.cx.run_until_parked();
}

/// Makes `window` active again without moving its focus.
fn reactivate(harness: &mut DesktopHarness, window: AnyWindowHandle) {
    harness.update_view(window, |_, window, _| window.activate_window());
    harness.cx.run_until_parked();
}

#[gpui::test]
fn the_close_confirmation_stays_open_through_an_outside_press_and_deactivation(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, runs) = harness.busy_window();
    harness.press(window, CLOSE);
    harness.wait_for("the confirmation on its primary button", |cx| {
        window
            .update(cx, |root, window, cx| {
                let view = root.downcast::<WorkspaceView>().unwrap();
                view.read(cx).close_buttons.focused(window) == "primary"
            })
            .unwrap()
    });
    let beside = beside_the_panel(&mut harness, window);
    click(&mut harness, window, beside);
    assert_eq!(harness.dialog_focus(window), Some("primary"));
    harness.cx.update(|cx| {
        assert!(!focused(cx, window, runs), "the press focused the terminal");
    });
    harness.deactivate(window);
    assert_eq!(harness.dialog_focus(window), Some("primary"));
    reactivate(&mut harness, window);
    assert_eq!(harness.dialog_focus(window), Some("primary"));
    // Return on the primary button closes the tab.
    harness.press(window, "enter");
    harness.wait_for("the tab to close", |cx| {
        !confirming(cx, window) && terminal(cx, window, runs).is_none()
    });
}

#[gpui::test]
fn escape_cancels_the_close_confirmation_and_tab_stays_inside_it(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, runs) = harness.busy_window();
    harness.press(window, CLOSE);
    harness.wait_for("the confirmation", |cx| confirming(cx, window));
    let mut seen = Vec::new();
    for key in ["tab", "tab", "tab", "shift-tab", "shift-tab"] {
        harness.press(window, key);
        seen.push(harness.dialog_focus(window).unwrap());
    }
    assert_eq!(seen, ["cancel", "primary", "cancel", "primary", "cancel"]);
    harness.press(window, "escape");
    assert_eq!(harness.dialog_focus(window), None);
    harness.wait_for("the terminal to take focus back", |cx| {
        focused(cx, window, runs)
    });
    harness
        .cx
        .update(|cx| assert!(terminal(cx, window, runs).is_some()));
}

#[gpui::test]
fn space_on_the_close_confirmations_cancel_cancels(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let (window, runs) = harness.busy_window();
    harness.press(window, CLOSE);
    harness.wait_for("the confirmation", |cx| confirming(cx, window));
    harness.press(window, "tab");
    assert_eq!(harness.dialog_focus(window), Some("cancel"));
    harness.press(window, "space");
    assert_eq!(harness.dialog_focus(window), None);
    harness.wait_for("the terminal to take focus back", |cx| {
        !confirming(cx, window) && focused(cx, window, runs)
    });
    harness
        .cx
        .update(|cx| assert!(terminal(cx, window, runs).is_some()));
}

#[gpui::test]
fn space_on_abouts_close_closes_it(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    harness.invoke(window, ids::ABOUT).unwrap();
    assert_eq!(harness.about_focus(window), Some(AboutButton::Close));
    harness.press(window, "space");
    assert_eq!(harness.about_focus(window), None);
    harness.wait_for("the terminal to take focus back", |cx| {
        focused(cx, window, tab)
    });
}

#[gpui::test]
fn about_stays_open_through_an_outside_press_and_deactivation(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    harness.invoke(window, ids::ABOUT).unwrap();
    assert_eq!(harness.about_focus(window), Some(AboutButton::Close));
    let mut seen = Vec::new();
    for key in ["tab", "tab", "tab", "shift-tab", "shift-tab"] {
        harness.press(window, key);
        seen.push(harness.about_focus(window).unwrap());
    }
    assert_eq!(
        seen,
        [
            AboutButton::Licenses,
            AboutButton::Copy,
            AboutButton::Close,
            AboutButton::Copy,
            AboutButton::Licenses,
        ]
    );
    let beside = beside_the_panel(&mut harness, window);
    click(&mut harness, window, beside);
    assert_eq!(harness.about_focus(window), Some(AboutButton::Licenses));
    harness.deactivate(window);
    assert_eq!(harness.about_focus(window), Some(AboutButton::Licenses));
    reactivate(&mut harness, window);
    assert_eq!(harness.about_focus(window), Some(AboutButton::Licenses));
    harness.press(window, "escape");
    assert_eq!(harness.about_focus(window), None);
    harness.wait_for("the terminal to take focus back", |cx| {
        focused(cx, window, tab)
    });
}

/// A catalog of one component whose license runs to `lines` lines, the
/// last of them `the end`.
fn long_license(lines: usize) -> Catalog {
    let mut body: Vec<String> =
        (1..lines).map(|line| format!("line {line}")).collect();
    body.push("the end".to_owned());
    Catalog {
        components: vec![Component {
            name: "Long".to_owned(),
            version: Some("1.0".to_owned()),
            kind: "Rust crate".to_owned(),
            license: "MIT".to_owned(),
            used: None,
            url: None,
            texts: vec![LicenseText {
                title: "LICENSE".to_owned(),
                body: Arc::from(body.join("\n")),
            }],
        }],
    }
}

/// A catalog of Alpha and Beta, each with a website.
fn two_components() -> Catalog {
    let component = |name: &str| Component {
        name: name.to_owned(),
        version: None,
        kind: "Rust crate".to_owned(),
        license: "MIT".to_owned(),
        used: None,
        url: Some(format!("https://example.com/{name}")),
        texts: vec![LicenseText {
            title: "LICENSE".to_owned(),
            body: Arc::from("MIT License"),
        }],
    };
    Catalog {
        components: vec![component("Alpha"), component("Beta")],
    }
}

/// Opens the Licenses view on [`two_components`] over About in a new
/// window, its search focused.
fn open_two_components(harness: &mut DesktopHarness) -> AnyWindowHandle {
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    harness.invoke(window, ids::ABOUT).unwrap();
    harness.update_view(window, |view, window, cx| {
        view.open_licenses(|| Ok(two_components()), window, cx);
    });
    harness.wait_for("the notices to load", |cx| {
        matches!(
            licenses_state(cx, window),
            Some(LicensesState::Ready { .. })
        )
    });
    harness.cx.run_until_parked();
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Search));
    window
}

/// How many components match the Licenses search in `window`.
fn license_matches(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> usize {
    match harness.cx.update(|cx| licenses_state(cx, window)) {
        Some(LicensesState::Ready { matches, .. }) => matches,
        state => panic!("the view is not ready: {state:?}"),
    }
}

/// The Licenses search's text in `window`.
fn search_text(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> String {
    harness.update_view(window, |view, _, cx| {
        let search = view.licenses.as_ref().unwrap().read(cx).search();
        search.read(cx).text().to_owned()
    })
}

#[gpui::test]
fn typing_on_a_licenses_button_inserts_once_at_the_search_caret(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let window = open_two_components(&mut harness);
    harness.press(window, "a l p h a tab");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Website));
    harness.press(window, "z");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Search));
    assert_eq!(search_text(&mut harness, window), "alphaz");
    // Done shows with no match, so only the typing moves focus from it.
    harness.press(window, "tab");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Done));
    harness.press(window, "z");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Search));
    assert_eq!(search_text(&mut harness, window), "alphazz");
}

#[gpui::test]
fn a_composition_started_on_a_licenses_button_finishes_in_the_search(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let window = open_two_components(&mut harness);
    // Done shows whatever the search matches, so only the composition moves
    // focus from it.
    harness.press(window, "shift-tab");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Done));
    // A dead key marks text through the handler the view registers for the
    // focused button; GPUI's tests can only send committed text through the
    // window.
    harness.update_view(window, |view, window, cx| {
        let licenses = view.licenses.as_ref().unwrap().read(cx);
        let mut handler = licenses.button_input_handler(cx);
        handler.replace_and_mark_text_in_range(None, "´", None, window, cx);
    });
    harness.cx.run_until_parked();
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Search));
    let marked = |harness: &mut DesktopHarness| {
        harness.update_view(window, |view, window, cx| {
            let search = view.licenses.as_ref().unwrap().read(cx).search();
            search.update(cx, |input, cx| input.marked_text_range(window, cx))
        })
    };
    assert_eq!(search_text(&mut harness, window), "´");
    assert_eq!(marked(&mut harness), Some(0..1));
    // The commit replaces the marked text through the window's handler.
    harness.press(window, "é");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Search));
    assert_eq!(search_text(&mut harness, window), "é");
    assert_eq!(marked(&mut harness), None);
}

#[gpui::test]
fn copy_on_a_licenses_button_keeps_focus_there(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let window = open_two_components(&mut harness);
    harness.press(window, "a l p h a tab");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Website));
    harness.invoke(window, ids::TEXT_COPY).unwrap();
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Website));
    assert_eq!(search_text(&mut harness, window), "alpha");
}

#[gpui::test]
fn editing_keys_on_a_licenses_button_move_to_the_search(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let window = open_two_components(&mut harness);
    harness.press(window, "a l p h a z");
    assert_eq!(license_matches(&mut harness, window), 0);
    // Only Done follows a search with no match.
    harness.press(window, "tab");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Done));
    harness.press(window, "backspace");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Search));
    assert_eq!(license_matches(&mut harness, window), 1);
    harness.press(window, "tab");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Website));
    // No binding claims Space on a button, so it types into the search, as
    // it would with the search focused.
    harness.press(window, "space");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Search));
    assert_eq!(search_text(&mut harness, window), "alpha ");
    assert_eq!(license_matches(&mut harness, window), 1);
    harness.press(window, "tab");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Website));
    harness.press(window, "left");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Search));
    harness.press(window, "tab tab");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Copy));
    harness.press(window, "right");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Search));
}

#[gpui::test]
fn a_search_with_no_match_from_a_licenses_button_keeps_keys_working(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let window = open_two_components(&mut harness);
    harness.press(window, "a l p h a tab tab");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Copy));
    harness.cx.update(|cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
            "zzz".to_owned(),
        ));
    });
    harness.invoke(window, ids::TEXT_PASTE).unwrap();
    assert_eq!(license_matches(&mut harness, window), 0);
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Search));
    harness.press(window, "escape");
    assert_eq!(license_matches(&mut harness, window), 2);
    harness.press(window, "tab");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Website));
    // An edit that reaches the search without moving focus, and hides the
    // focused button, also returns focus to the search.
    harness.press(window, "tab");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Copy));
    harness.update_view(window, |view, _, cx| {
        view.licenses
            .clone()
            .unwrap()
            .update(cx, |licenses, cx| licenses.set_search("zzz", cx));
    });
    harness.cx.run_until_parked();
    assert_eq!(license_matches(&mut harness, window), 0);
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Search));
    harness.press(window, "escape");
    assert_eq!(license_matches(&mut harness, window), 2);
    harness.press(window, "escape");
    assert_eq!(harness.cx.update(|cx| licenses_state(cx, window)), None);
}

/// How many pages a test scrolls, more than its long license needs.
const PAGES: u8 = 40;

/// The Licenses text's scroll offset, furthest offset, and painted bounds.
fn text_scroll(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> (Pixels, Pixels, gpui::Bounds<Pixels>) {
    harness.update_view(window, |view, _, cx| {
        view.licenses.as_ref().unwrap().read(cx).text_scroll()
    })
}

#[gpui::test]
fn a_long_license_scrolls_to_its_end_by_page_down_and_the_wheel(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    harness.invoke(window, ids::ABOUT).unwrap();
    harness.update_view(window, |view, window, cx| {
        view.open_licenses(|| Ok(long_license(400)), window, cx);
    });
    harness.wait_for("the license to lay out", |cx| {
        matches!(
            licenses_state(cx, window),
            Some(LicensesState::Ready { .. })
        )
    });
    harness.cx.run_until_parked();
    let (offset, max, text) = text_scroll(&mut harness, window);
    assert_eq!(offset, px(0.0));
    assert!(max > px(0.0), "the text does not scroll: {max:?}");
    // The text's viewport lies within the panel, so its end can show.
    let panel = harness
        .update_view(window, |view, _, _| view.dialog.panel_bounds())
        .expect("the panel painted");
    assert!(
        text.bottom() <= panel.bottom() && text.top() >= panel.top(),
        "the text {text:?} overflows the panel {panel:?}"
    );
    // More than the text needs: a press or tick past the end stays there.
    let pages = (max / text.size.height).ceil();
    assert!(pages < f32::from(PAGES), "{pages} pages");
    for _ in 0..PAGES {
        harness.press(window, "pagedown");
    }
    assert_eq!(text_scroll(&mut harness, window).0, -max, "Page Down");
    for _ in 0..PAGES {
        harness.press(window, "pageup");
    }
    assert_eq!(text_scroll(&mut harness, window).0, px(0.0), "Page Up");
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    for _ in 0..PAGES {
        visual.simulate_event(ScrollWheelEvent {
            position: text.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), -text.size.height)),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
    }
    harness.cx.run_until_parked();
    assert_eq!(text_scroll(&mut harness, window).0, -max, "the wheel");
}
