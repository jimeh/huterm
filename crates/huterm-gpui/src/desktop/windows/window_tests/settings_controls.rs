//! Settings switches, segmented controls, modifier chips, and buttons on
//! `huterm-ui`'s components, driven by real keyboard and pointer events:
//! D8, D9, D10, and D16 of `docs/plans/ui-components.md`, and the arrow
//! keys that move between chips.

use gpui::{AccessibleAction, FocusHandle, KeyDownEvent, KeyUpEvent};
use huterm_config::settings::SectionId;
use serde_json::Value;

use super::super::settings_window::SettingsView;
use super::settings::{
    ScratchConfig, open_settings, settings, settled, with_view, with_window,
};
use super::*;
use crate::config::KeybindingEntry;

/// Opens Settings on `section` and draws it.
pub(super) fn show(
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

pub(super) fn visual(harness: &DesktopHarness) -> VisualTestContext {
    let handle = harness
        .cx
        .update(|cx| settings(cx).expect("Settings is open"));
    VisualTestContext::from_window(handle.into(), &harness.cx)
}

/// Sends `keys`, space separated, through the keymap.
pub(super) fn keys(harness: &DesktopHarness, keys: &str) {
    let mut visual = visual(harness);
    visual.simulate_keystrokes(keys);
    visual.run_until_parked();
}

/// Sends a press of `key`, as a held key's repeat when `is_held`.
pub(super) fn key_down(harness: &DesktopHarness, key: &str, is_held: bool) {
    let mut visual = visual(harness);
    visual.simulate_event(KeyDownEvent {
        keystroke: gpui::Keystroke::parse_with_os(
            key,
            crate::keymap::Platform::current().os(),
        )
        .expect("a valid keystroke"),
        is_held,
        prefer_character_input: false,
    });
    visual.run_until_parked();
}

/// Sends `key`'s press and release; GPUI's implicit click on the focused
/// element fires on the release, which `simulate_keystrokes` never sends.
pub(super) fn press_and_release(harness: &DesktopHarness, key: &str) {
    let mut visual = visual(harness);
    visual.simulate_keystrokes(key);
    visual.simulate_event(KeyUpEvent {
        keystroke: gpui::Keystroke::parse_with_os(
            key,
            crate::keymap::Platform::current().os(),
        )
        .expect("a valid keystroke"),
    });
    visual.run_until_parked();
}

pub(super) fn focus(harness: &mut DesktopHarness) -> String {
    with_window(harness, |view, window, cx| view.focus_name(window, cx))
}

/// The accessibility node the element named `id` reported in the last
/// frame; the dump spells element ids in their `Debug` form.
pub(super) fn a11y_node(harness: &mut DesktopHarness, id: &str) -> Value {
    let spelled = format!("{:?}", gpui::ElementId::Name(id.to_owned().into()));
    a11y_find(harness, |node| {
        node["element_id"].as_str() == Some(spelled.as_str())
    })
    .unwrap_or_else(|| panic!("no accessibility node for `{id}`"))
}

pub(super) fn a11y_find(
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
pub(super) fn a11y_click(harness: &mut DesktopHarness, node: &Value) {
    a11y_act(harness, node, AccessibleAction::Click);
}

/// Sends the accessibility `action` to `node`.
pub(super) fn a11y_act(
    harness: &mut DesktopHarness,
    node: &Value,
    action: AccessibleAction,
) {
    let target = node["accesskit_id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .expect("every node reports its AccessKit id");
    // Through the window, not the view: the action updates the view.
    let mut visual = visual(harness);
    visual.update(|window, cx| {
        window.simulate_a11y_action(
            gpui::accesskit::ActionRequest {
                action,
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
pub(super) fn ring(harness: &mut DesktopHarness, handle: &FocusHandle) -> bool {
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
fn a_changed_settings_reset_is_a_named_icon_button_that_resets_it(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new(
        "controls-reset-icon",
        "[terminal.bell]\nvisual = false\n",
    );
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Terminal);
    // The window grows to keep the bell rows on it, for the click.
    visual(&harness).simulate_resize(gpui::size(px(860.0), px(1600.0)));
    visual(&harness).run_until_parked();
    let id = "settings-reset-terminal.bell.visual";
    let node = a11y_node(&mut harness, id);
    let aria = &node["aria"];
    assert_eq!(aria["role"], "Button", "{aria}");
    assert_eq!(aria["label"], "Reset Visual bell", "{aria}");
    assert_eq!(aria["description"], "Reset to default", "{aria}");
    // Icon-only: a square, with no room for the label.
    let bounds = visual(&harness)
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} was drawn"));
    assert_eq!(bounds.size.width, bounds.size.height, "{bounds:?}");
    a11y_click(&mut harness, &node);
    harness.wait_for("the reset", |cx| settled(cx) && file.text().is_empty());
    assert_eq!(file.text(), "");
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
    // Down and Right move focus on, Up and Left back, wrapping at both
    // ends, and write nothing.
    keys(&harness, "right");
    focused(&mut harness, "alt");
    keys(&harness, "left left");
    focused(&mut harness, "shift");
    keys(&harness, "down");
    focused(&mut harness, first);
    keys(&harness, "up");
    focused(&mut harness, "shift");
    keys(&harness, "down");
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

/// Breaks the file with `broken` and waits until Settings reads it and
/// turns read-only, then draws the result.
pub(super) fn break_file(
    harness: &mut DesktopHarness,
    file: &ScratchConfig,
    broken: &str,
) {
    std::fs::write(file.path(), broken).unwrap();
    with_view(harness, SettingsView::follow_reload);
    harness.wait_for("the invalid reread", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.read_only())
                    .unwrap_or(false)
            })
    });
    visual(harness).run_until_parked();
}

#[gpui::test]
fn create_refuses_a_click_and_says_why_while_the_file_is_invalid(
    cx: &mut TestAppContext,
) {
    let logs = "[quake.profiles.logs]\nheight = 0.3\n";
    let file = ScratchConfig::new("controls-create", logs);
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Quake);
    with_window(&mut harness, |view, window, cx| {
        view.type_profile_name("build", window, cx);
    });
    let broken = format!("{logs}[window\n");
    break_file(&mut harness, &file, &broken);
    let create = a11y_node(&mut harness, "settings-qcreate");
    assert_eq!(create["aria"]["disabled"], true, "{create}");
    assert_eq!(
        create["aria"]["description"],
        "Read-only until config.toml is fixed"
    );
    let before = focus(&mut harness);
    a11y_click(&mut harness, &create);
    assert_eq!(focus(&mut harness), before, "the click took focus");
    with_view(&mut harness, |view, cx| {
        // The command's own refusal would have said why in the prompt.
        assert_eq!(view.quake_message(cx), None, "the click ran Create");
        assert_eq!(view.quake_prompts(), (true, false));
    });
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), broken);
}

#[gpui::test]
fn removing_an_unbind_refuses_a_click_and_says_why_while_the_file_is_invalid(
    cx: &mut TestAppContext,
) {
    let unbind = "[[keybinding]]\nkey = \"alt-q\"\ncommand = \"unbind\"\n";
    let file = ScratchConfig::new("controls-unbind", unbind);
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Keybindings);
    // Disabled shortcuts are few, so the unbind card below them is on
    // screen for the click.
    with_window(&mut harness, |view, window, cx| {
        view.action_handle("kfilter").focus(window, cx);
    });
    keys(&harness, "right right right");
    assert_eq!(with_view(&mut harness, |view, _| view.key_filter()), "off");
    // The file still parses, so the shortcuts stay listed, but a global
    // shortcut for a missing profile fails validation.
    let broken = format!(
        "{unbind}\n[[global_keybinding]]\nkey = \"ctrl-alt-shift-y\"\n\
         command = \"toggle_quake\"\nargs = {{ profile = \"missing\" }}\n"
    );
    break_file(&mut harness, &file, &broken);
    let remove = a11y_node(&mut harness, "settings-kunbind-0");
    assert_eq!(remove["aria"]["label"], "Remove alt-q", "{remove}");
    assert_eq!(remove["aria"]["disabled"], true, "{remove}");
    assert_eq!(
        remove["aria"]["description"],
        "Read-only until config.toml is fixed"
    );
    let before = focus(&mut harness);
    a11y_click(&mut harness, &remove);
    assert_eq!(focus(&mut harness), before, "the click took focus");
    assert!(harness.cx.update(settled), "the click queued a removal");
    assert_eq!(file.text(), broken);
}

#[gpui::test]
fn the_last_held_modifier_says_why_it_stays_on(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("controls-last-chip", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Terminal);
    let (held, key, other) = if cfg!(target_os = "macos") {
        ("⌘ Cmd", "cmd", "⌥ Option")
    } else {
        ("Ctrl", "ctrl", "Alt")
    };
    let chip = |harness: &mut DesktopHarness, label: &str| {
        a11y_find(harness, |node| node["aria"]["label"] == label)
            .unwrap_or_else(|| panic!("no chip labelled {label}"))
    };
    // The window grows to keep the chips on it, for the click.
    visual(&harness).simulate_resize(gpui::size(px(860.0), px(1600.0)));
    visual(&harness).run_until_parked();
    let last = chip(&mut harness, held);
    assert_eq!(last["aria"]["toggled"], "True", "{last}");
    assert_eq!(last["aria"]["description"], "Keep at least one modifier");
    assert!(chip(&mut harness, other)["aria"]["description"].is_null());
    // The chip stays focusable, so the click lands and is refused.
    a11y_click(&mut harness, &last);
    assert_eq!(
        focus(&mut harness),
        format!("action:chip:terminal.link_modifiers:{key}")
    );
    assert!(harness.cx.update(settled), "the click queued a write");
    assert_eq!(file.text(), "");
    assert_eq!(chip(&mut harness, held)["aria"]["toggled"], "True");
}

#[gpui::test]
fn arrows_step_the_shortcut_filter_and_stop_at_its_ends(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("controls-filter", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Keybindings);
    let filter = |harness: &mut DesktopHarness| {
        with_view(harness, |view, _| view.key_filter())
    };
    with_window(&mut harness, |view, window, cx| {
        view.action_handle("kfilter").focus(window, cx);
    });
    keys(&harness, "left");
    assert_eq!(filter(&mut harness), "all");
    keys(&harness, "right");
    assert_eq!(filter(&mut harness), "custom");
    keys(&harness, "right right");
    assert_eq!(filter(&mut harness), "off");
    keys(&harness, "right");
    assert_eq!(filter(&mut harness), "off");
    keys(&harness, "left");
    assert_eq!(filter(&mut harness), "when");
    assert_eq!(focus(&mut harness), "action:kfilter");
    assert!(harness.cx.update(settled), "an arrow queued a write");
    assert_eq!(file.text(), "");
}

/// The only tri-state, automatic update checks, applies on macOS alone;
/// elsewhere it is read-only.
#[cfg(target_os = "macos")]
#[gpui::test]
fn arrows_step_a_tri_state_in_its_drawn_order_and_stop_at_its_ends(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("controls-tri-state", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Updates);
    let path = "updates.automatic_checks";
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop(path, window, cx);
    });
    // Default is first, so Left from it writes nothing.
    keys(&harness, "left");
    assert!(
        harness.cx.update(settled),
        "Left from Default queued a write"
    );
    assert_eq!(file.text(), "");
    let step = |harness: &mut DesktopHarness, key: &str, expected: &str| {
        keys(harness, key);
        harness
            .wait_for("the step", |cx| settled(cx) && file.text() == expected);
    };
    let on = "[updates]\nautomatic_checks = true\n";
    let off = "[updates]\nautomatic_checks = false\n";
    step(&mut harness, "right", on);
    step(&mut harness, "right", off);
    // Off is last.
    keys(&harness, "right");
    assert!(harness.cx.update(settled), "Right from Off queued a write");
    assert_eq!(file.text(), off);
    step(&mut harness, "left", on);
    // Back to Default removes the key, as Reset does.
    keys(&harness, "left");
    harness.wait_for("the reset", |cx| {
        settled(cx) && !file.text().contains("automatic_checks")
    });
    assert_eq!(focus(&mut harness), path);
}

#[gpui::test]
fn link_look_footer_actions_report_as_buttons(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("controls-link-roles", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Tabs);
    let open = a11y_node(&mut harness, "settings-footer-open");
    assert_eq!(open["aria"]["role"], "Button", "{open}");
    // An invalid file gives the footer a status action too.
    break_file(&mut harness, &file, "[window\n");
    let ids = ["settings-footer-reload", "settings-footer-open-error"]
        .map(|id| format!("{:?}", gpui::ElementId::Name(id.into())));
    let action = a11y_find(&mut harness, |node| {
        ids.iter()
            .any(|id| node["element_id"].as_str() == Some(id.as_str()))
    })
    .expect("the invalid file's status offers an action");
    assert_eq!(action["aria"]["role"], "Button", "{action}");
}

#[gpui::test]
fn show_advanced_is_one_state_that_the_file_keeps(cx: &mut TestAppContext) {
    // Terminal and Scrollback each have an advanced setting.
    const TERM: &str = "settings-row-terminal.term";
    const SIZE: &str = "settings-row-terminal.scrollback_bytes";
    let file = ScratchConfig::new(
        "controls-show-advanced",
        "[settings]\nshow_advanced = true\n",
    );
    let mut harness = DesktopHarness::new(cx);
    let drawn = |harness: &mut DesktopHarness, section, row: &'static str| {
        with_window(harness, |view, window, cx| {
            view.show_section(section, cx);
            window.refresh();
        });
        visual(harness).debug_bounds(row).is_some()
    };
    // A file that shows them shows them from the window's first frame.
    show(&mut harness, &file, SectionId::Terminal);
    assert!(drawn(&mut harness, SectionId::Terminal, TERM));
    // General has a row for the state.
    assert!(drawn(
        &mut harness,
        SectionId::General,
        "settings-row-settings.show_advanced"
    ));
    // Hiding them on one page hides them on every page, and returns the
    // key to its default.
    with_window(&mut harness, |view, window, cx| {
        view.show_section(SectionId::Scrollback, cx);
        view.press("advanced:pane", window, cx);
    });
    harness.wait_for("the removal", |cx| {
        settled(cx) && !file.text().contains("show_advanced")
    });
    // The emptied table goes with its key.
    assert_eq!(file.text(), "");
    assert!(!drawn(&mut harness, SectionId::Scrollback, SIZE));
    assert!(!drawn(&mut harness, SectionId::Terminal, TERM));
    // Showing them writes the key, and every page follows.
    with_window(&mut harness, |view, window, cx| {
        view.press("advanced:pane", window, cx);
    });
    harness.wait_for("the save", |cx| {
        settled(cx) && file.text().contains("show_advanced = true")
    });
    assert!(drawn(&mut harness, SectionId::Scrollback, SIZE));
}
