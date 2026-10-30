//! Settings selects on `huterm-ui`'s popover, driven by real pointer and
//! keyboard events: the dismissal and focus contract of
//! `docs/plans/ui-components.md` (D1 to D5, D15, D16).

use gpui::{Bounds, KeyUpEvent, Modifiers, Pixels, Point};
use huterm_config::settings::SectionId;

use super::super::settings_window::SettingsView;
use super::settings::{
    ScratchConfig, open_settings, settings, settled, with_view, with_window,
};
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

/// Opens Settings on the Quake windows section and draws it.
fn quake_section(harness: &mut DesktopHarness, file: &ScratchConfig) {
    open_settings(harness, &file.path());
    with_window(harness, |view, window, cx| {
        view.show_section(SectionId::Quake, cx);
        window.refresh();
    });
    draw(harness);
}

#[gpui::test]
fn a_tall_menu_scrolls_within_the_window(cx: &mut TestAppContext) {
    // Forty profiles make the profile menu taller than any window.
    let mut profiles = String::new();
    for index in 0..40 {
        writeln!(profiles, "[quake.profiles.p{index:02}]\nheight = 0.5")
            .unwrap();
    }
    let file = ScratchConfig::new("select-tall", &profiles);
    let mut harness = DesktopHarness::new(cx);
    quake_section(&mut harness, &file);
    let profile = trigger(&mut harness, "qprof");
    click(&harness, profile.center());
    let viewport = visual(&harness).update(|window, _| window.viewport_size());
    let (menu, scrolls) = with_window(&mut harness, |view, _, cx| {
        view.menu_layout(cx).expect("the profile menu drew")
    });
    assert!(scrolls, "forty profiles do not fit");
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

#[gpui::test]
fn focus_leaving_the_menu_closes_it_where_focus_went(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("select-search", "");
    let mut harness = DesktopHarness::new(cx);
    tabs_section(&mut harness, &file);
    // GPUI reports focus changes only in an active window.
    with_window(&mut harness, |_, window, _| window.activate_window());
    draw(&harness);
    let at = trigger(&mut harness, "tabs.position").center();
    click(&harness, at);
    assert!(menu_open(&mut harness));
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::SETTINGS_SEARCH, window, cx).unwrap();
    });
    draw(&harness);
    assert!(!menu_open(&mut harness));
    assert_eq!(focus(&mut harness), "search");
    // The closed menu is gone from the screen too: a stale frame would
    // still draw its window-wide layer over the next select.
    let label = trigger(&mut harness, "tabs.label").center();
    click(&harness, label);
    assert!(menu_open(&mut harness));
}

#[gpui::test]
fn changing_section_closes_the_menu(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("select-section", "");
    let mut harness = DesktopHarness::new(cx);
    tabs_section(&mut harness, &file);
    let at = trigger(&mut harness, "tabs.position").center();
    click(&harness, at);
    assert!(menu_open(&mut harness));
    with_window(&mut harness, |view, window, cx| {
        view.show_section(SectionId::General, cx);
        window.refresh();
    });
    draw(&harness);
    assert!(!menu_open(&mut harness));
    // The menu's control is in the old section, so focus recovers to the
    // window's root rather than staying on the dropped menu.
    assert!(with_window(&mut harness, |view, window, _| {
        view.root_focused(window)
    }));
}

#[gpui::test]
fn resizing_the_window_closes_the_menu(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("select-resize", "");
    let mut harness = DesktopHarness::new(cx);
    tabs_section(&mut harness, &file);
    let at = trigger(&mut harness, "tabs.position").center();
    click(&harness, at);
    assert!(menu_open(&mut harness));
    // The menu was placed for the larger window.
    let resized = visual(&harness).update(|window, _| {
        let viewport = window.viewport_size();
        gpui::size(viewport.width, viewport.height * 0.6)
    });
    visual(&harness).simulate_resize(resized);
    draw(&harness);
    assert!(!menu_open(&mut harness));
    assert_eq!(focus(&mut harness), "tabs.position");
}

#[gpui::test]
fn the_profile_select_still_switches_profiles_on_an_invalid_file(
    cx: &mut TestAppContext,
) {
    let original = "[quake.profiles.logs]\nheight = 0.3\n";
    let file = ScratchConfig::new("select-locked-profile", original);
    let mut harness = DesktopHarness::new(cx);
    quake_section(&mut harness, &file);
    std::fs::write(file.path(), format!("{original}[window\n")).unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the invalid reread", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.read_only())
                    .unwrap_or(false)
            })
    });
    draw(&harness);
    // Choosing the profile to show writes nothing, so a press opens it.
    let at = trigger(&mut harness, "qprof").center();
    click(&harness, at);
    assert!(menu_open(&mut harness));
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::MENU_SELECT_LAST, window, cx).unwrap();
        view.command(ids::MENU_CONFIRM, window, cx).unwrap();
    });
    draw(&harness);
    with_window(&mut harness, |view, _, _| {
        assert_eq!(view.shown_profile_name(), "logs");
    });
}

/// The link chord's default and its Settings spelling of Cmd on this
/// platform.
fn platform_keys() -> (&'static str, &'static str) {
    if cfg!(target_os = "macos") {
        ("cmd", "cmd")
    } else {
        ("ctrl", "super")
    }
}

#[gpui::test]
fn the_drag_modifier_select_offers_what_the_link_chord_leaves_and_writes_it(
    cx: &mut TestAppContext,
) {
    const PATH: &str = "panes.drag_modifier";
    let file = ScratchConfig::new("select-drag-modifier", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.show_section(SectionId::Panes, cx);
        window.refresh();
    });
    draw(&harness);
    // The row draws a select in the Panes section.
    trigger(&mut harness, PATH);
    let (link, cmd) = platform_keys();
    with_window(&mut harness, |view, window, cx| {
        view.open_select_for(PATH, window, cx);
        let ids = view.menu_ids(cx);
        let labels = view.menu_labels(cx);
        assert_eq!(ids.first().map(String::as_str), Some("off"));
        assert_eq!(labels.first().map(String::as_str), Some("Off"));
        // The link chord plus Shift opens links over a program that takes
        // the mouse, so the menu leaves it out.
        assert!(!ids.contains(&format!("{link}-shift")), "{ids:?}");
        let both = format!("{cmd}-ctrl");
        let index = ids.iter().position(|id| *id == both).expect("Cmd-Ctrl");
        let label = if cfg!(target_os = "macos") {
            "⌃⌘ Control-Command"
        } else {
            "Ctrl+Super"
        };
        assert_eq!(labels[index], label);
        // Two or more modifiers, less the excluded ones.
        let platform = if cfg!(target_os = "macos") { 9 } else { 11 };
        assert_eq!(ids.len(), 1 + platform - 1, "{ids:?}");
        view.pick_menu_item(&both, cx);
    });
    harness.wait_for("the save", |cx| {
        settled(cx) && file.text().contains("drag_modifier")
    });
    assert_eq!(
        file.text(),
        format!("[panes]\ndrag_modifier = \"{cmd}-ctrl\"\n")
    );
    // The menu opens on the value, and Off removes the key.
    with_window(&mut harness, |view, window, cx| {
        view.open_select_for(PATH, window, cx);
        view.pick_menu_item("off", cx);
    });
    harness.wait_for("the removal", |cx| {
        settled(cx) && !file.text().contains("drag_modifier")
    });
}

#[gpui::test]
fn a_link_chip_that_would_take_the_drag_modifier_is_refused(
    cx: &mut TestAppContext,
) {
    let (link, _) = platform_keys();
    let original = format!("[panes]\ndrag_modifier = \"{link}-alt-shift\"\n");
    let file = ScratchConfig::new("chip-drag-collision", &original);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    // The link chord plus Alt would make the drag combination the chord
    // plus Shift.
    with_window(&mut harness, |view, window, cx| {
        view.press("chip:terminal.link_modifiers:alt", window, cx);
    });
    harness.wait_for("the refusal", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| {
                        view.setting_error("terminal.link_modifiers").is_some()
                    })
                    .unwrap_or(false)
            })
    });
    with_view(&mut harness, |view, _| {
        let message = view.setting_error("terminal.link_modifiers").unwrap();
        assert!(
            message.contains("panes.drag_modifier")
                && message.contains("terminal.link_modifiers plus shift"),
            "{message}"
        );
    });
    assert_eq!(file.text(), original);
}
