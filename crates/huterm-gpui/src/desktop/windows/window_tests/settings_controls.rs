//! Settings switches, segmented controls, modifier chips, and buttons on
//! `huterm-ui`'s components, driven by real keyboard and pointer events:
//! D8, D9, D10, and D16 of `docs/plans/ui-components.md`, and the arrow
//! keys that move between chips.

use gpui::{AccessibleAction, FocusHandle, KeyUpEvent};
use huterm_config::settings::SectionId;
use serde_json::Value;

use super::super::settings_window::SettingsView;
use super::settings::{
    ScratchConfig, open_settings, settings, settled, with_view, with_window,
};
use super::*;
use crate::config::KeybindingEntry;

/// Opens Settings on `section` and draws it.
fn show(
    harness: &mut DesktopHarness,
    file: &ScratchConfig,
    section: SectionId,
) {
    open_settings(harness, &file.path());
    with_window(harness, |view, window, cx| {
        view.show_section(section, cx);
        window.refresh();
    });
    visual(harness).run_until_parked();
}

fn visual(harness: &DesktopHarness) -> VisualTestContext {
    let handle = harness
        .cx
        .update(|cx| settings(cx).expect("Settings is open"));
    VisualTestContext::from_window(handle.into(), &harness.cx)
}

/// Sends `keys`, space separated, through the keymap.
fn keys(harness: &DesktopHarness, keys: &str) {
    let mut visual = visual(harness);
    visual.simulate_keystrokes(keys);
    visual.run_until_parked();
}

/// Sends `key`'s press and release; GPUI's implicit click on the focused
/// element fires on the release, which `simulate_keystrokes` never sends.
fn press_and_release(harness: &DesktopHarness, key: &str) {
    let mut visual = visual(harness);
    visual.simulate_keystrokes(key);
    visual.simulate_event(KeyUpEvent {
        keystroke: gpui::Keystroke::parse(key).expect("a valid keystroke"),
    });
    visual.run_until_parked();
}

fn focus(harness: &mut DesktopHarness) -> String {
    with_window(harness, |view, window, cx| view.focus_name(window, cx))
}

/// The accessibility node the element named `id` reported in the last
/// frame; the dump spells element ids in their `Debug` form.
fn a11y_node(harness: &mut DesktopHarness, id: &str) -> Value {
    let spelled = format!("{:?}", gpui::ElementId::Name(id.to_owned().into()));
    a11y_find(harness, |node| {
        node["element_id"].as_str() == Some(spelled.as_str())
    })
    .unwrap_or_else(|| panic!("no accessibility node for `{id}`"))
}

fn a11y_find(
    harness: &mut DesktopHarness,
    matches: impl Fn(&Value) -> bool,
) -> Option<Value> {
    let json =
        with_window(harness, |_, window, _| window.debug_a11y_tree_json())
            .expect(
                "test windows report an accessibility tree for every frame",
            );
    let tree: Value = serde_json::from_str(&json).unwrap();
    tree["nodes"]
        .as_object()
        .unwrap()
        .values()
        .find(|node| matches(node))
        .cloned()
}

/// An accessibility Click on `node`, which GPUI performs as a pointer
/// press and release at its center.
fn a11y_click(harness: &mut DesktopHarness, node: &Value) {
    let target = node["accesskit_id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .expect("every node reports its AccessKit id");
    // Through the window, not the view: the click updates the view.
    let mut visual = visual(harness);
    visual.update(|window, cx| {
        window.simulate_a11y_action(
            gpui::accesskit::ActionRequest {
                action: AccessibleAction::Click,
                target_tree: gpui::accesskit::TreeId::ROOT,
                target_node: gpui::accesskit::NodeId(target),
                data: None,
            },
            cx,
        );
    });
    visual.run_until_parked();
}

/// Whether the last painted frame drew `handle`'s focus ring.
fn ring(harness: &mut DesktopHarness, handle: &FocusHandle) -> bool {
    with_window(harness, |_, window, cx| {
        huterm_ui::focus::ring_drawn(handle, window, cx)
    })
}

#[gpui::test]
fn arrows_leave_a_switch_alone_and_step_a_segmented_control(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("controls-arrows", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Tabs);
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop("tabs.always_show", window, cx);
    });
    keys(&harness, "up right down left");
    assert_eq!(focus(&mut harness), "tabs.always_show");
    assert!(harness.cx.update(settled), "an arrow queued a write");
    assert_eq!(file.text(), "", "an arrow switched the switch");
    // A segmented control still steps and writes, as a radio group does.
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop("tabs.style", window, cx);
    });
    keys(&harness, "left");
    harness.wait_for("the step", |cx| {
        settled(cx) && file.text().contains("style")
    });
    assert_eq!(file.text(), "[tabs]\nstyle = \"strip\"\n");
}

#[gpui::test]
fn a_click_draws_no_focus_ring_and_the_keyboard_does(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("controls-ring", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Tabs);
    let switch = a11y_node(&mut harness, "settings-switch-tabs.always_show");
    a11y_click(&mut harness, &switch);
    harness.wait_for("the click's save", |cx| {
        settled(cx) && file.text().contains("always_show")
    });
    visual(&harness).run_until_parked();
    assert_eq!(focus(&mut harness), "tabs.always_show");
    let handle = with_window(&mut harness, |view, _, _| {
        view.stop_handle("tabs.always_show")
    });
    assert!(!ring(&mut harness, &handle), "a click drew the ring");
    // Back and forth with the keyboard lands on it again, with the ring.
    keys(&harness, "shift-tab tab");
    assert_eq!(focus(&mut harness), "tabs.always_show");
    assert!(ring(&mut harness, &handle), "Tab drew no ring");
}

#[gpui::test]
fn a_read_only_control_refuses_a_click_and_keys_and_says_why(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("controls-read-only", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Tabs);
    // The segmented control has keyboard focus when the file breaks.
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop("tabs.style", window, cx);
    });
    std::fs::write(file.path(), "[window\n").unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the invalid reread", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.read_only())
                    .unwrap_or(false)
            })
    });
    visual(&harness).run_until_parked();
    // Focus stays, so a key reaching the window root cannot pass for a
    // refusal; the control's own commands refuse.
    assert_eq!(focus(&mut harness), "tabs.style");
    keys(&harness, "left space");
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), "[window\n");
    // A click on the switch neither takes focus nor writes. The banner
    // pushes the rows down, so the window grows to keep the switch on it.
    visual(&harness).simulate_resize(gpui::size(px(860.0), px(1600.0)));
    visual(&harness).run_until_parked();
    let switch = a11y_node(&mut harness, "settings-switch-tabs.always_show");
    a11y_click(&mut harness, &switch);
    assert_eq!(focus(&mut harness), "tabs.style");
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), "[window\n");
    // Each control says why it is read-only.
    let reason = "Read-only until config.toml is fixed";
    for id in [
        "settings-switch-tabs.always_show",
        "settings-segments-tabs.style",
    ] {
        let aria = &a11y_node(&mut harness, id)["aria"];
        assert_eq!(aria["disabled"], true, "{id}: {aria}");
        assert_eq!(aria["description"], reason, "{id}: {aria}");
    }
}

#[gpui::test]
fn a_removed_enter_binding_activates_no_switch_chip_or_button(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new(
        "controls-enter",
        "[terminal.bell]\nvisual = false\n",
    );
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
    show(&mut harness, &file, SectionId::Terminal);
    // Alt is off, so a click on its chip would turn it on.
    for key in [
        "chip:terminal.link_modifiers:alt",
        "reset:terminal.bell.visual",
    ] {
        with_window(&mut harness, |view, window, cx| {
            view.action_handle(key).focus(window, cx);
        });
        press_and_release(&harness, "enter");
        assert_eq!(focus(&mut harness), format!("action:{key}"));
    }
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop("terminal.bell.visual", window, cx);
    });
    press_and_release(&harness, "enter");
    assert!(harness.cx.update(settled), "Enter queued a write");
    assert_eq!(file.text(), "[terminal.bell]\nvisual = false\n");
    // Space, still bound, toggles the switch.
    keys(&harness, "space");
    harness.wait_for("the toggle", |cx| {
        settled(cx) && file.text().contains("true")
    });
    assert_eq!(file.text(), "[terminal.bell]\nvisual = true\n");
}

#[gpui::test]
fn arrows_move_between_modifier_chips_and_space_toggles_one(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("controls-chips", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Terminal);
    let (first, path) = if cfg!(target_os = "macos") {
        ("cmd", "chip:terminal.link_modifiers")
    } else {
        ("ctrl", "chip:terminal.link_modifiers")
    };
    let focused = |harness: &mut DesktopHarness, chip: &str| {
        assert_eq!(focus(harness), format!("action:{path}:{chip}"));
    };
    with_window(&mut harness, |view, window, cx| {
        view.action_handle(&format!("{path}:{first}"))
            .focus(window, cx);
    });
    // Right and Left move focus, wrapping at both ends, and write nothing.
    keys(&harness, "right");
    focused(&mut harness, "alt");
    keys(&harness, "left left");
    focused(&mut harness, "shift");
    keys(&harness, "down");
    focused(&mut harness, "alt");
    keys(&harness, "up up");
    focused(&mut harness, first);
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), "");
    // The only held modifier refuses to turn off; another turns on.
    keys(&harness, "space");
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), "");
    keys(&harness, "right space");
    harness.wait_for("the toggle", |cx| {
        settled(cx) && file.text().contains("link_modifiers")
    });
    assert_eq!(
        file.text(),
        format!("[terminal]\nlink_modifiers = \"{first}-alt\"\n")
    );
    focused(&mut harness, "alt");
}
