//! Settings selects on `huterm-ui`'s popover, driven by real pointer and
//! keyboard events: the dismissal and focus contract of
//! `docs/plans/ui-components.md` (D1 to D5, D15, D16).

use gpui::{Bounds, KeyUpEvent, Modifiers, Pixels, Point};
use huterm_config::settings::SectionId;

use super::settings::{ScratchConfig, open_settings, settings, with_window};
use super::*;
use crate::config::KeybindingEntry;

/// Opens Settings on the Tabs section, whose position, label, and close
/// button settings are selects, and draws it.
fn tabs_section(harness: &mut DesktopHarness, file: &ScratchConfig) {
    open_settings(harness, &file.path());
    with_window(harness, |view, window, cx| {
        view.show_section(SectionId::Tabs, cx);
        window.refresh();
    });
    draw(harness);
}

fn visual(harness: &DesktopHarness) -> VisualTestContext {
    let handle = harness
        .cx
        .update(|cx| settings(cx).expect("Settings is open"));
    VisualTestContext::from_window(handle.into(), &harness.cx)
}

fn draw(harness: &DesktopHarness) {
    visual(harness).run_until_parked();
}

fn trigger(harness: &mut DesktopHarness, key: &str) -> Bounds<Pixels> {
    with_window(harness, |view, _, _| view.select_triggers())
        .into_iter()
        .find_map(|(name, bounds)| (name == key).then_some(bounds))
        .unwrap_or_else(|| panic!("no `{key}` select drawn"))
}

fn click(harness: &DesktopHarness, at: Point<Pixels>) {
    let mut visual = visual(harness);
    visual.simulate_click(at, Modifiers::none());
    visual.run_until_parked();
}

fn focus(harness: &mut DesktopHarness) -> String {
    with_window(harness, |view, window, cx| view.focus_name(window, cx))
}

fn menu_open(harness: &mut DesktopHarness) -> bool {
    with_window(harness, |view, _, _| view.menu_open())
}

#[gpui::test]
fn a_press_outside_the_menu_closes_it_and_acts_on_nothing(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("select-outside", "");
    let mut harness = DesktopHarness::new(cx);
    tabs_section(&mut harness, &file);
    let position = trigger(&mut harness, "tabs.position");
    let label = trigger(&mut harness, "tabs.label");
    click(&harness, position.center());
    assert!(menu_open(&mut harness));
    assert_eq!(focus(&mut harness), "menu");
    // A press on another select closes this menu; it neither opens that
    // select nor moves focus to it.
    click(&harness, label.center());
    assert!(!menu_open(&mut harness));
    assert_eq!(focus(&mut harness), "tabs.position");
    assert_eq!(file.text(), "");
}

#[gpui::test]
fn a_second_press_on_the_trigger_closes_its_menu(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("select-toggle", "");
    let mut harness = DesktopHarness::new(cx);
    tabs_section(&mut harness, &file);
    let position = trigger(&mut harness, "tabs.position");
    click(&harness, position.center());
    assert!(menu_open(&mut harness));
    click(&harness, position.center());
    assert!(!menu_open(&mut harness));
    assert_eq!(focus(&mut harness), "tabs.position");
}

#[gpui::test]
fn a_tall_menu_scrolls_within_the_window(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("select-tall", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.show_section(SectionId::Appearance, cx);
        window.refresh();
    });
    draw(&harness);
    with_window(&mut harness, |view, window, cx| {
        view.press("fonts:font.family", window, cx);
    });
    draw(&harness);
    let viewport = visual(&harness).update(|window, _| window.viewport_size());
    let (menu, scrolls) = with_window(&mut harness, |view, _, cx| {
        view.menu_layout(cx).expect("the font menu drew")
    });
    assert!(scrolls, "every installed family does not fit");
    assert!(menu.top() >= px(0.0) && menu.bottom() <= viewport.height);
}

#[gpui::test]
fn deactivating_the_window_closes_the_menu(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("select-deactivate", "");
    let mut harness = DesktopHarness::new(cx);
    tabs_section(&mut harness, &file);
    with_window(&mut harness, |_, window, _| window.activate_window());
    draw(&harness);
    let position = trigger(&mut harness, "tabs.position");
    click(&harness, position.center());
    assert!(menu_open(&mut harness));
    let mut visual = visual(&harness);
    visual.deactivate_window();
    visual.run_until_parked();
    assert!(!menu_open(&mut harness));
    assert_eq!(focus(&mut harness), "tabs.position");
}

#[gpui::test]
fn shift_tab_closes_the_menu_onto_its_trigger(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("select-shift-tab", "");
    let mut harness = DesktopHarness::new(cx);
    tabs_section(&mut harness, &file);
    let position = trigger(&mut harness, "tabs.position");
    click(&harness, position.center());
    assert_eq!(focus(&mut harness), "menu");
    let mut visual = visual(&harness);
    visual.simulate_keystrokes("shift-tab");
    visual.run_until_parked();
    assert!(!menu_open(&mut harness));
    assert_eq!(focus(&mut harness), "tabs.position");
}

#[gpui::test]
fn a_removed_enter_binding_does_not_open_a_select(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("select-enter", "");
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.keybindings.push(KeybindingEntry {
            key: "enter".into(),
            command: "settings_toggle".into(),
            when: None,
            args: None,
            description: None,
            disabled: Some(true),
        });
    });
    tabs_section(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop("tabs.position", window, cx);
    });
    draw(&harness);
    // GPUI clicks a focused element on Enter's key-up; send both.
    let mut visual = visual(&harness);
    visual.simulate_keystrokes("enter");
    visual.simulate_event(KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").expect("a valid keystroke"),
    });
    visual.run_until_parked();
    assert!(!menu_open(&mut harness), "Enter is unbound");
    // Space, still bound, opens it.
    visual.simulate_keystrokes("space");
    visual.run_until_parked();
    assert!(menu_open(&mut harness));
}
