//! The terminal window's dialogs in its `huterm-ui` dialog slot, migration
//! 7b of `docs/plans/ui-components.md`: the close confirmation, About, and
//! Licenses stay open through outside presses and window deactivation,
//! keep Tab inside, and return focus to the terminal; a long license
//! scrolls to its end.

use std::sync::Arc;

use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TouchPhase, point};

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
