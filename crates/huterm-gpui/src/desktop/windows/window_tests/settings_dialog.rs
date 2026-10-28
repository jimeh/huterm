//! The Settings window's dialog through the desktop harness: the Quake
//! delete-profile alert of migration 7a and D17 in
//! `docs/plans/ui-components.md`, driven by real keys and presses.

use huterm_config::settings::SectionId;

use super::super::settings_window::SettingsView;
use super::settings::{
    ScratchConfig, settings, settled, with_view, with_window,
};
use super::settings_controls::{
    a11y_find, a11y_node, break_file, focus, keys, show, visual,
};
use super::*;
use crate::config::KeybindingEntry;

const LOGS: &str = "# Mine.\n\n[quake.profiles.logs]\nheight = 0.3 # short\n";

/// Opens Settings on the Quake windows section showing `logs`.
fn show_logs(harness: &mut DesktopHarness, file: &ScratchConfig) {
    show(harness, file, SectionId::Quake);
    with_window(harness, |view, window, _| {
        view.pick_profile("logs");
        window.refresh();
    });
    visual(harness).run_until_parked();
}

/// Presses Enter on Delete profile.
fn open_alert(harness: &mut DesktopHarness) {
    with_window(harness, |view, window, cx| {
        view.action_handle("qdel").focus(window, cx);
    });
    keys(harness, "enter");
}

/// Clicks at `position` without drawing first, so the click meets the last
/// drawn frame.
fn click_drawn_frame(
    window: &mut gpui::Window,
    cx: &mut App,
    position: gpui::Point<gpui::Pixels>,
) {
    let modifiers = gpui::Modifiers::none();
    window.dispatch_event(
        gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
            position,
            modifiers,
            button: gpui::MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    );
    window.dispatch_event(
        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
            position,
            modifiers,
            button: gpui::MouseButton::Left,
            click_count: 1,
        }),
        cx,
    );
}

fn dialog(harness: &mut DesktopHarness) -> &'static str {
    with_view(harness, |view, _| view.dialog_name())
}

#[gpui::test]
fn delete_profile_opens_an_alert_with_focus_on_cancel(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("dialog-open", LOGS);
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    open_alert(&mut harness);
    assert_eq!(dialog(&mut harness), "delete");
    assert_eq!(focus(&mut harness), "dialog:qdelno");
    let alert = a11y_node(&mut harness, "settings-delete-profile");
    let aria = &alert["aria"];
    assert_eq!(aria["role"], "AlertDialog", "{alert}");
    assert_eq!(aria["label"], "Delete the \u{201c}logs\u{201d} profile?");
    assert_eq!(
        aria["description"],
        "Its settings and comments are removed from config.toml."
    );
    assert_eq!(aria["modal"], true);
    assert_eq!(file.text(), LOGS);
}

#[gpui::test]
fn tab_shift_tab_and_the_arrows_stay_inside_the_alert(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("dialog-cycle", LOGS);
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    open_alert(&mut harness);
    for (key, expected) in [
        ("tab", "dialog:qdelyes"),
        ("tab", "dialog:qdelno"),
        ("shift-tab", "dialog:qdelyes"),
        ("right", "dialog:qdelno"),
        ("left", "dialog:qdelyes"),
    ] {
        keys(&harness, key);
        assert_eq!(focus(&mut harness), expected, "after {key}");
    }
    assert_eq!(dialog(&mut harness), "delete");
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), LOGS);
}

#[gpui::test]
fn escape_and_enter_on_cancel_close_the_alert_on_delete_profile(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("dialog-cancel", LOGS);
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    for key in ["escape", "enter"] {
        open_alert(&mut harness);
        keys(&harness, key);
        assert_eq!(dialog(&mut harness), "none", "after {key}");
        assert_eq!(focus(&mut harness), "action:qdel", "after {key}");
    }
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), LOGS);
}

#[gpui::test]
fn a_rebound_dialog_cancel_cancels_and_escape_is_not_special(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("dialog-rebound", LOGS);
    let mut harness = DesktopHarness::with_config(cx, |config| {
        for (key, disabled) in [("escape", Some(true)), ("ctrl-g", None)] {
            config.keybindings.push(KeybindingEntry {
                key: key.into(),
                command: "dialog_cancel".into(),
                when: None,
                args: None,
                description: None,
                disabled,
            });
        }
    });
    show_logs(&mut harness, &file);
    open_alert(&mut harness);
    keys(&harness, "escape");
    assert_eq!(dialog(&mut harness), "delete");
    assert_eq!(focus(&mut harness), "dialog:qdelno");
    keys(&harness, "ctrl-g");
    assert_eq!(dialog(&mut harness), "none");
    assert_eq!(focus(&mut harness), "action:qdel");
    assert_eq!(file.text(), LOGS);
}

#[gpui::test]
fn enter_on_delete_removes_the_profile_and_focuses_the_profile_select(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("dialog-delete", LOGS);
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    open_alert(&mut harness);
    keys(&harness, "tab enter");
    harness.wait_for("the delete", |cx| {
        settled(cx) && !file.text().contains("logs")
    });
    assert_eq!(file.text(), "# Mine.\n");
    assert_eq!(dialog(&mut harness), "none");
    assert_eq!(focus(&mut harness), "action:qprof");
    with_view(&mut harness, |view, _| {
        assert_eq!(view.shown_profile_name(), "default");
    });
}

#[gpui::test]
fn an_outside_press_leaves_the_alert_open_and_acts_on_nothing(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("dialog-outside", LOGS);
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    let profiles = with_view(&mut harness, |view, _| {
        view.select_triggers()
            .into_iter()
            .find_map(|(key, bounds)| (key == "qprof").then_some(bounds))
            .expect("the profile select painted")
    });
    // A press on the profile select before the alert's first frame lands
    // on a frame without its scrim; the guard stops it.
    with_window(&mut harness, |view, window, cx| {
        view.open_delete_alert_unseen(window, cx);
    });
    let mut visual = visual(&harness);
    visual.update(|window, cx| {
        click_drawn_frame(window, cx, profiles.center());
    });
    assert!(!with_view(&mut harness, |view, _| view.menu_open()));
    with_window(&mut harness, |_, window, _| window.refresh());
    visual.run_until_parked();
    assert_eq!(dialog(&mut harness), "delete");
    assert_eq!(focus(&mut harness), "dialog:qdelno");
    // Once drawn, a press beside the panel neither dismisses nor acts.
    let panel = with_view(&mut harness, |view, _| view.dialog_panel())
        .expect("the alert painted");
    assert!(!panel.contains(&profiles.center()));
    visual.simulate_click(profiles.center(), gpui::Modifiers::none());
    visual.run_until_parked();
    assert!(!with_view(&mut harness, |view, _| view.menu_open()));
    assert_eq!(dialog(&mut harness), "delete");
    assert_eq!(focus(&mut harness), "dialog:qdelno");
    assert_eq!(file.text(), LOGS);
}

#[gpui::test]
fn a_read_only_file_opens_no_delete_alert(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("dialog-read-only", LOGS);
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    let broken = format!("{LOGS}[window\n");
    break_file(&mut harness, &file, &broken);
    let delete = a11y_node(&mut harness, "settings-qdel");
    assert_eq!(delete["aria"]["disabled"], true, "{delete}");
    open_alert(&mut harness);
    assert_eq!(dialog(&mut harness), "none");
    assert_eq!(focus(&mut harness), "action:qdel");
    let cancel = a11y_find(&mut harness, |node| {
        node["element_id"]
            .as_str()
            .is_some_and(|id| id.contains("qdelno"))
    });
    assert!(cancel.is_none(), "{cancel:?}");
}

#[gpui::test]
fn a_profile_removed_elsewhere_closes_its_alert(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("dialog-removed", LOGS);
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    open_alert(&mut harness);
    assert_eq!(dialog(&mut harness), "delete");
    std::fs::write(file.path(), "# Mine.\n").unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the reread", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.profile_names().len() == 1)
                    .unwrap_or(false)
            })
    });
    visual(&harness).run_until_parked();
    assert_eq!(dialog(&mut harness), "none");
    assert_eq!(focus(&mut harness), "action:qprof");
    assert_eq!(file.text(), "# Mine.\n");
}

#[gpui::test]
fn the_alert_blocks_other_settings_commands_but_not_closing_the_window(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("dialog-blocks", LOGS);
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    open_alert(&mut harness);
    assert_eq!(dialog(&mut harness), "delete");
    // Search and section moves would take focus behind the scrim, where
    // Escape no longer reaches the alert.
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::SETTINGS_SEARCH, window, cx).unwrap();
        view.command(ids::NEXT_TAB, window, cx).unwrap();
    });
    harness.cx.run_until_parked();
    assert_eq!(dialog(&mut harness), "delete");
    assert_eq!(focus(&mut harness), "dialog:qdelno");
    assert_eq!(
        with_view(&mut harness, |view, _| view.shown_section()),
        SectionId::Quake
    );
    // Closing the window still works.
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::CLOSE_WINDOW, window, cx).unwrap();
    });
    harness.wait_for("the window to close", |cx| settings(cx).is_none());
}
