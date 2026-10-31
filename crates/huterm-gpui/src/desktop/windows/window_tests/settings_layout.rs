//! The Settings layout on `huterm-ui`'s layout components (migration 6a of
//! `docs/plans/ui-components.md`, #276): the sidebar as vertical tabs, the
//! theme cards as a radio group, the group commands `settings_next` and
//! `settings_previous`, Tab scrolling the focused control into view, the
//! section header and the pane's scrollbar, and the rows, alerts, and empty
//! states.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    Bounds, FocusHandle, Modifiers, Pixels, ScrollDelta, ScrollEvent,
    TouchPhase,
};
use huterm_config::settings::SectionId;
use serde_json::Value;

use super::super::settings_window::SettingsView;
use super::settings::{
    ScratchConfig, conflicted, open_settings, settled, with_view, with_window,
};
use super::settings_controls::{
    a11y_click, a11y_find, a11y_node, break_file, focus, keys, ring, show,
    visual,
};
use super::*;
use crate::config::KeybindingEntry;
use crate::desktop::key_hint::KeyHint;
use crate::keymap::Platform;

/// A key the tracker notes as the last press, which nothing in Settings
/// binds, so a focus move that follows counts as the keyboard's.
fn note_a_key(harness: &DesktopHarness) {
    keys(harness, "f13");
}

/// Gives `handle` focus from the keyboard, as a key that moved it would.
fn keyboard_focus(harness: &mut DesktopHarness, handle: &FocusHandle) {
    note_a_key(harness);
    let handle = handle.clone();
    with_window(harness, move |_, window, cx| handle.focus(window, cx));
    visual(harness).run_until_parked();
}

/// Shows the Quake windows section on the `logs` profile.
fn show_logs(harness: &mut DesktopHarness) {
    with_window(harness, |view, window, cx| {
        view.show_section(SectionId::Quake, cx);
        view.pick_profile("logs");
        window.refresh();
    });
    visual(harness).run_until_parked();
}

fn section(harness: &mut DesktopHarness) -> SectionId {
    with_view(harness, |view, _| view.shown_section())
}

fn sections(harness: &mut DesktopHarness) -> Vec<SectionId> {
    with_view(harness, |view, _| view.sidebar_sections())
}

/// The accessibility node of the control labelled `label` with `role`.
fn labelled(harness: &mut DesktopHarness, role: &str, label: &str) -> Value {
    a11y_find(harness, |node| {
        node["aria"]["role"] == role && node["aria"]["label"] == label
    })
    .unwrap_or_else(|| panic!("no {role} labelled {label}"))
}

#[gpui::test]
fn arrows_move_between_sections_and_show_each(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("layout-sidebar-arrows", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::General);
    let order = sections(&mut harness);
    let last = *order.last().expect("sections");
    let general = with_view(&mut harness, |view, _| {
        view.section_handle(SectionId::General)
    });
    keyboard_focus(&mut harness, &general);
    // Down and Right move on, Up and Left back, wrapping; each move shows
    // the section focus reaches.
    for (key, expected) in [
        ("down", SectionId::Appearance),
        ("right", SectionId::Windows),
        ("up", SectionId::Appearance),
        ("left", SectionId::General),
        ("up", last),
        ("right", SectionId::General),
        ("left", last),
        ("down", SectionId::General),
    ] {
        keys(&harness, key);
        assert_eq!(section(&mut harness), expected, "after {key}");
        assert_eq!(
            focus(&mut harness),
            format!("section:{}", expected.id()),
            "after {key}"
        );
    }
    assert!(harness.cx.update(settled), "a section move queued a write");
    assert_eq!(file.text(), "");
}

#[gpui::test]
fn the_sidebar_is_one_tab_stop_that_tabs_into_the_section(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layout-sidebar-tab", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Tabs);
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::SETTINGS_SEARCH, window, cx).unwrap();
    });
    assert_eq!(focus(&mut harness), "search");
    // One stop: the shown section, not the first.
    keys(&harness, "tab");
    assert_eq!(focus(&mut harness), "section:tabs");
    let handle =
        with_view(&mut harness, |view, _| view.section_handle(SectionId::Tabs));
    assert!(
        with_window(&mut harness, |_, window, cx| {
            huterm_ui::focus::inset_ring_drawn(&handle, window, cx)
        }),
        "the focused tab drew no ring"
    );
    // Tab enters the section's first control, past the footer's links, and
    // Shift-Tab returns to the sidebar.
    keys(&harness, "tab");
    assert_eq!(focus(&mut harness), "tabs.position");
    keys(&harness, "shift-tab");
    assert_eq!(focus(&mut harness), "section:tabs");
    keys(&harness, "shift-tab");
    assert_eq!(focus(&mut harness), "search");
}

#[gpui::test]
fn a_section_tab_reports_its_role_and_selection(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("layout-sidebar-a11y", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::General);
    let list = labelled(&mut harness, "TabList", "Sections");
    assert_eq!(list["aria"]["orientation"], "Vertical", "{list}");
    let general = labelled(&mut harness, "Tab", "General");
    assert_eq!(general["aria"]["selected"], true, "{general}");
    let appearance = labelled(&mut harness, "Tab", "Appearance");
    assert_eq!(appearance["aria"]["selected"], false, "{appearance}");
    let handle = with_view(&mut harness, |view, _| {
        view.section_handle(SectionId::General)
    });
    keyboard_focus(&mut harness, &handle);
    keys(&harness, "down");
    assert_eq!(
        labelled(&mut harness, "Tab", "Appearance")["aria"]["selected"],
        true
    );
    assert_eq!(
        labelled(&mut harness, "Tab", "General")["aria"]["selected"],
        false
    );
}

#[gpui::test]
fn an_arrow_during_a_search_shows_the_section_it_reaches(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layout-sidebar-search", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::SETTINGS_SEARCH, window, cx).unwrap();
    });
    let mut typing = visual(&harness);
    typing.simulate_input("font");
    typing.run_until_parked();
    // The search hides the selection, so reaching the section shown
    // before it still shows that section, and ends the search.
    assert_eq!(
        labelled(&mut harness, "Tab", "Appearance")["aria"]["selected"],
        false
    );
    let general = with_view(&mut harness, |view, _| {
        view.section_handle(SectionId::General)
    });
    keyboard_focus(&mut harness, &general);
    keys(&harness, "down");
    assert_eq!(focus(&mut harness), "section:appearance");
    assert_eq!(
        with_window(&mut harness, |view, _, cx| view.search_text(cx)),
        ""
    );
    assert_eq!(section(&mut harness), SectionId::Appearance);
    assert_eq!(
        labelled(&mut harness, "Tab", "Appearance")["aria"]["selected"],
        true
    );
}

/// The theme names and the index of the one the picker marks.
fn themes(harness: &mut DesktopHarness) -> (Vec<String>, usize) {
    with_view(harness, |view, cx| {
        let names = view.theme_names();
        let selected = view.selected(cx);
        let index = names
            .iter()
            .position(|name| *name == selected)
            .expect("the selected theme has a card");
        (names, index)
    })
}

fn theme_handle(harness: &mut DesktopHarness, name: &str) -> FocusHandle {
    let key = format!("theme:{name}");
    with_view(harness, move |view, _| view.action_handle(&key))
}

#[gpui::test]
fn arrows_move_between_theme_cards_and_choose_each(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("layout-theme-arrows", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    let (names, start) = themes(&mut harness);
    assert!(names.len() > 2, "{names:?}");
    let count = names.len();
    let handle = theme_handle(&mut harness, &names[start]);
    keyboard_focus(&mut harness, &handle);
    let mut at = start;
    for (key, forward) in [
        ("down", true),
        ("right", true),
        ("up", false),
        ("left", false),
    ] {
        at = if forward {
            (at + 1) % count
        } else {
            (at + count - 1) % count
        };
        keys(&harness, key);
        assert_eq!(themes(&mut harness).1, at, "after {key}");
        assert_eq!(
            focus(&mut harness),
            format!("action:theme:{}", names[at]),
            "after {key}"
        );
    }
    // Both ends wrap.
    let first = theme_handle(&mut harness, &names[0]);
    keyboard_focus(&mut harness, &first);
    keys(&harness, "up");
    assert_eq!(themes(&mut harness).1, count - 1);
    keys(&harness, "down");
    assert_eq!(themes(&mut harness).1, 0);
    harness.wait_for("the theme saves", |cx| {
        settled(cx) && file.text().contains(&names[0])
    });
    assert_eq!(file.text(), format!("[theme]\nname = \"{}\"\n", names[0]));
}

#[gpui::test]
fn arrows_on_the_mode_cards_pick_the_mode_the_theme_cards_set(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new(
        "layout-theme-slots",
        "[theme]\nlight = \"one-light\"\ndark = \"dracula\"\n",
    );
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    // The test platform's system is light, so the light card starts checked,
    // and its accessible name says which mode the system is in.
    assert_eq!(themes(&mut harness).0[themes(&mut harness).1], "one-light");
    let system = if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "the system"
    };
    let light = labelled(
        &mut harness,
        "RadioButton",
        &format!("Light mode theme: one-light, {system} is light"),
    );
    assert_eq!(light["aria"]["toggled"], "True", "{light}");
    labelled(&mut harness, "RadioButton", "Dark mode theme: dracula");
    let light = with_view(&mut harness, |view, _| {
        view.action_handle("theme-slot:light")
    });
    keyboard_focus(&mut harness, &light);
    keys(&harness, "right");
    assert_eq!(focus(&mut harness), "action:theme-slot:dark");
    let (names, at) = themes(&mut harness);
    assert_eq!(names[at], "dracula");
    keys(&harness, "right");
    assert_eq!(focus(&mut harness), "action:theme-slot:light");
    assert!(
        harness.cx.update(settled),
        "a mode card move queued a write"
    );
    assert_eq!(
        file.text(),
        "[theme]\nlight = \"one-light\"\ndark = \"dracula\"\n"
    );
}

#[gpui::test]
fn the_theme_mode_switch_sits_at_its_rows_end_sized_to_its_options(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layout-theme-mode-row", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    let mut visual = visual(&harness);
    let label = visual
        .debug_bounds("settings-label-theme")
        .expect("the Theme mode row drew");
    let switch = visual
        .debug_bounds("settings-theme-mode")
        .expect("the mode switch drew");
    let row = visual
        .debug_bounds("settings-row-theme")
        .expect("the Theme mode row drew");
    // On the label's line, at the row's end inside its 14-point padding,
    // as a segmented row's control is, and far narrower than the row.
    assert!(switch.top() < label.bottom() && label.top() < switch.bottom());
    let inset = row.right() - switch.right();
    assert!((inset - px(14.0)).abs() < px(1.0), "{row:?} {switch:?}");
    assert!(switch.size.width < row.size.width / 2.0, "{switch:?}");
}

#[gpui::test]
fn the_theme_mode_switch_works_from_the_keyboard(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("layout-theme-mode", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    let mode =
        with_view(&mut harness, |view, _| view.action_handle("theme-mode"));
    keyboard_focus(&mut harness, &mode);
    // Right steps to Match system, which starts both modes on the theme.
    keys(&harness, "right");
    harness.wait_for("Match system to save", |cx| {
        settled(cx) && file.text().contains("light")
    });
    assert_eq!(
        file.text(),
        "[theme]\nlight = \"tokyo-night\"\ndark = \"tokyo-night\"\n"
    );
    assert_eq!(focus(&mut harness), "action:theme-mode");
    // Left steps back to one theme.
    keys(&harness, "left");
    harness.wait_for("one theme to save", |cx| {
        settled(cx) && !file.text().contains("light")
    });
    assert_eq!(file.text(), "[theme]\nname = \"tokyo-night\"\n");
    // The arrows stop at the ends, as a segmented row's do; one key at a
    // time, since two steps of a wrapping switch would come back.
    for key in ["left", "down"] {
        keys(&harness, key);
        assert!(harness.cx.update(settled), "{key} past the end wrote");
        assert_eq!(file.text(), "[theme]\nname = \"tokyo-night\"\n");
    }
    // Return opens the options too, and Escape leaves the choice alone.
    keys(&harness, "enter");
    assert!(with_view(&mut harness, |view, _| view.menu_open()));
    keys(&harness, "escape");
    assert!(!with_view(&mut harness, |view, _| view.menu_open()));
    assert_eq!(focus(&mut harness), "action:theme-mode");
    assert_eq!(file.text(), "[theme]\nname = \"tokyo-night\"\n");
    // Space opens the options on the current one, as on every segmented
    // row; Down and Return pick Match system.
    keys(&harness, "space");
    assert_eq!(
        with_view(&mut harness, |view, cx| view.menu_labels(cx)),
        ["One theme", "Match system"]
    );
    keys(&harness, "down enter");
    harness.wait_for("the menu's pick to save", |cx| {
        settled(cx) && file.text().contains("light")
    });
    assert!(!with_view(&mut harness, |view, _| view.menu_open()));
    assert_eq!(focus(&mut harness), "action:theme-mode");
    // At Match system the arrows stop too.
    for key in ["right", "up"] {
        keys(&harness, key);
        assert!(harness.cx.update(settled), "{key} past the end wrote");
        assert!(file.text().contains("light"), "{key}");
    }
}

/// A click on a theme card draws no ring, and keyboard focus on it does.
fn a_theme_card_rings_for_the_keyboard_alone(harness: &mut DesktopHarness) {
    // The click lands at the card's center, so the cards must be on screen
    // below the Font rows.
    visual(harness).simulate_resize(gpui::size(px(860.0), px(900.0)));
    // The first card, at the top of the grid.
    let (names, start) = themes(harness);
    let other = names[usize::from(start == 0)].clone();
    let card = labelled(harness, "RadioButton", &other);
    a11y_click(harness, &card);
    assert_eq!(focus(harness), format!("action:theme:{other}"));
    let handle = theme_handle(harness, &other);
    assert!(!ring(harness, &handle), "a click drew the ring");
    keys(harness, "shift-tab tab");
    assert_eq!(focus(harness), format!("action:theme:{other}"));
    assert!(ring(harness, &handle), "Tab drew no ring");
}

#[gpui::test]
fn a_theme_card_rings_after_tab_but_not_a_click_in_a_dark_theme(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layout-theme-ring-dark", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    a_theme_card_rings_for_the_keyboard_alone(&mut harness);
}

#[gpui::test]
fn a_theme_card_rings_after_tab_but_not_a_click_in_a_light_theme(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layout-theme-ring-light", "");
    let light =
        crate::themes::preview("tokyo-night-day", std::path::Path::new(""))
            .expect("a bundled theme");
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.theme = light;
    });
    show(&mut harness, &file, SectionId::Appearance);
    a_theme_card_rings_for_the_keyboard_alone(&mut harness);
}

#[gpui::test]
fn arrows_on_a_theme_card_that_turned_read_only_move_and_write_nothing(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layout-theme-read-only", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    let (names, start) = themes(&mut harness);
    let handle = theme_handle(&mut harness, &names[start]);
    keyboard_focus(&mut harness, &handle);
    break_file(&mut harness, &file, "[window\n");
    let card = format!("action:theme:{}", names[start]);
    assert_eq!(focus(&mut harness), card, "the card lost focus");
    for key in ["down", "right", "up", "left"] {
        keys(&harness, key);
        assert_eq!(focus(&mut harness), card, "after {key}");
        assert_eq!(themes(&mut harness).1, start, "after {key}");
    }
    assert!(harness.cx.update(settled), "an arrow queued a write");
    assert_eq!(file.text(), "[window\n");
}

#[gpui::test]
fn the_theme_cards_are_tab_stops_of_radio_buttons(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("layout-theme-stop", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    let (names, start) = themes(&mut harness);
    // Light themes, then dark ones, the default's group.
    let light = labelled(&mut harness, "RadioGroup", "Light themes");
    let dark = labelled(&mut harness, "RadioGroup", "Dark themes");
    assert_eq!(light["aria"]["size_of_set"], 10, "{light}");
    assert_eq!(dark["aria"]["size_of_set"], names.len() - 10, "{dark}");
    let checked = labelled(&mut harness, "RadioButton", &names[start]);
    assert_eq!(checked["aria"]["toggled"], "True", "{checked}");
    let next = &names[(start + 1) % names.len()];
    assert_eq!(
        labelled(&mut harness, "RadioButton", next)["aria"]["toggled"],
        "False"
    );
    let handle = theme_handle(&mut harness, &names[start]);
    keyboard_focus(&mut harness, &handle);
    // Tab leaves the cards; Shift-Tab comes back to the checked one.
    keys(&harness, "tab");
    assert!(
        !focus(&mut harness).starts_with("action:theme:"),
        "Tab reached another card"
    );
    keys(&harness, "shift-tab");
    assert_eq!(
        focus(&mut harness),
        format!("action:theme:{}", names[start])
    );
    keys(&harness, "down");
    assert_eq!(
        labelled(&mut harness, "RadioButton", next)["aria"]["toggled"],
        "True"
    );
}

#[gpui::test]
fn group_bindings_leave_a_select_a_switch_and_a_slider_their_arrows(
    cx: &mut TestAppContext,
) {
    // The select starts on a value with one before it on every platform:
    // its default is the last choice on macOS and the first on Linux.
    let file = ScratchConfig::new(
        "layout-group-scope",
        "[tabs]\nposition = \"bottom\"\n\n[quake.profiles.logs]\nwidth = 0.5\n",
    );
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Tabs);
    // A select steps between its choices, as before.
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop("tabs.position", window, cx);
    });
    keys(&harness, "down");
    harness.wait_for("the select's step", |cx| {
        settled(cx) && file.text().contains("position = \"top\"")
    });
    assert_eq!(focus(&mut harness), "tabs.position");
    // A switch ignores the arrows.
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop("tabs.always_show", window, cx);
    });
    let before = file.text();
    keys(&harness, "down right");
    assert!(harness.cx.update(settled), "an arrow queued a write");
    assert_eq!(file.text(), before);
    assert_eq!(focus(&mut harness), "tabs.always_show");
    // A slider steps.
    show_logs(&mut harness);
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop("quake.profiles.logs.width", window, cx);
    });
    keys(&harness, "down");
    harness.wait_for("the slider's step", |cx| {
        settled(cx) && file.text().contains("width = 0.45")
    });
}

/// A Settings window whose config, and its scratch file `name` after
/// `text`, add `entries`, each `(key, command, disabled)` with no `when`.
/// The file carries them too, so a write's reload keeps them.
fn with_bindings(
    cx: &mut TestAppContext,
    name: &str,
    text: &str,
    entries: &[(&str, &str, bool)],
) -> (ScratchConfig, DesktopHarness) {
    let mut text = text.to_owned();
    for (key, command, disabled) in entries {
        let disabled = if *disabled { "disabled = true\n" } else { "" };
        let _ = write!(
            text,
            "\n[[keybinding]]\nkey = \"{key}\"\ncommand = \"{command}\"\n\
             {disabled}"
        );
    }
    let file = ScratchConfig::new(name, &text);
    let entries: Vec<_> = entries
        .iter()
        .map(|(key, command, disabled)| KeybindingEntry {
            key: (*key).into(),
            command: (*command).into(),
            when: None,
            args: None,
            description: None,
            disabled: disabled.then_some(true),
        })
        .collect();
    let harness = DesktopHarness::with_config(cx, move |config| {
        config.keybindings.extend(entries);
    });
    (file, harness)
}

/// Presses `key` on the sections, the theme cards, and the modifier
/// chips in turn, and checks that each moves forward.
fn key_moves_every_group(
    harness: &mut DesktopHarness,
    file: &ScratchConfig,
    key: &str,
) {
    show(harness, file, SectionId::General);
    let general =
        with_view(harness, |view, _| view.section_handle(SectionId::General));
    keyboard_focus(harness, &general);
    keys(harness, key);
    assert_eq!(section(harness), SectionId::Appearance, "{key} on sections");
    assert_eq!(focus(harness), "section:appearance", "{key} on sections");
    let (names, start) = themes(harness);
    let handle = theme_handle(harness, &names[start]);
    keyboard_focus(harness, &handle);
    keys(harness, key);
    let next = (start + 1) % names.len();
    assert_eq!(themes(harness).1, next, "{key} on theme cards");
    harness.wait_for("the theme saves", |cx| {
        settled(cx) && file.text().contains(&names[next])
    });
    show(harness, file, SectionId::Terminal);
    let chips = "chip:terminal.link_modifiers";
    let first = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    let chip = with_view(harness, |view, _| {
        view.action_handle(&format!("{chips}:{first}"))
    });
    keyboard_focus(harness, &chip);
    keys(harness, key);
    assert_eq!(
        focus(harness),
        format!("action:{chips}:alt"),
        "{key} on chips"
    );
}

#[gpui::test]
fn a_stepping_arrow_without_a_condition_leaves_the_groups_their_arrows(
    cx: &mut TestAppContext,
) {
    let (file, mut harness) = with_bindings(
        cx,
        "layout-group-stepping-arrow",
        "[quake.profiles.logs]\nwidth = 0.5\n",
        &[("down", "settings_increase", false)],
    );
    // The group moves match deeper than any stepping binding, so Down
    // still moves every group.
    key_moves_every_group(&mut harness, &file, "down");
    // On a slider Down runs the user's binding.
    show_logs(&mut harness);
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop("quake.profiles.logs.width", window, cx);
    });
    keys(&harness, "down");
    harness.wait_for("the slider's step", |cx| {
        settled(cx) && file.text().contains("width = 0.55")
    });
}

#[gpui::test]
fn a_rebound_settings_next_moves_groups_and_leaves_steppers_alone(
    cx: &mut TestAppContext,
) {
    let (file, mut harness) = with_bindings(
        cx,
        "layout-group-rebound",
        "[font]\nsize = 14.0\n\n[quake.profiles.logs]\nwidth = 0.5\n",
        &[
            ("down", "settings_next", true),
            ("j", "settings_next", false),
        ],
    );
    key_moves_every_group(&mut harness, &file, "j");
    // Switching Down off, with no condition, leaves the theme cards still.
    show(&mut harness, &file, SectionId::Appearance);
    let (names, start) = themes(&mut harness);
    let handle = theme_handle(&mut harness, &names[start]);
    keyboard_focus(&mut harness, &handle);
    keys(&harness, "down");
    assert_eq!(themes(&mut harness).1, start);
    // What each keystroke resolved to: `j` reaches no binding on a slider
    // or a number field.
    let resolved = Rc::new(RefCell::new(Vec::new()));
    let record = resolved.clone();
    let _observer = harness.cx.update(|cx| {
        cx.observe_keystrokes(move |event, _, _| {
            record.borrow_mut().push((
                event.keystroke.key.clone(),
                event.action.as_ref().map(|action| action.name()),
            ));
        })
    });
    with_window(&mut harness, |view, window, cx| {
        view.focus_field("font.size", window, cx);
    });
    let before = file.text();
    keys(&harness, "j");
    assert_eq!(focus(&mut harness), "font.size");
    show_logs(&mut harness);
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop("quake.profiles.logs.width", window, cx);
    });
    keys(&harness, "j");
    assert_eq!(focus(&mut harness), "quake.profiles.logs.width");
    assert!(harness.cx.update(settled), "`j` queued a write");
    assert_eq!(file.text(), before);
    let pressed: Vec<_> = resolved
        .borrow()
        .iter()
        .filter(|(key, _)| key == "j")
        .cloned()
        .collect();
    assert_eq!(
        pressed,
        [("j".to_owned(), None), ("j".to_owned(), None)],
        "`j` resolved to a binding"
    );
    // Down still steps the slider.
    keys(&harness, "down");
    harness.wait_for("the slider's step", |cx| {
        settled(cx) && file.text().contains("width = 0.45")
    });
}

/// Whether the focused control, with its ring, shows fully in the pane;
/// `None` when it sits outside the pane, such as in the sidebar.
fn focused_in_view(harness: &mut DesktopHarness) -> Option<bool> {
    with_window(harness, |view, window, cx| {
        let focused = window.focused(cx)?;
        let painted = huterm_ui::focus::ring_bounds(&focused, window, cx)?;
        let viewport = view.pane_viewport();
        if painted.left() < viewport.left() {
            return None;
        }
        let reach = px(3.0);
        Some(
            painted.top() - reach >= viewport.top()
                && painted.bottom() + reach <= viewport.bottom(),
        )
    })
}

fn scrolled(harness: &mut DesktopHarness) -> Pixels {
    with_view(harness, |view, _| view.pane_scrolled())
}

/// Opens the Terminal section in a short window, so most of it sits below
/// the fold, with keyboard focus on its first control.
fn short_terminal(harness: &mut DesktopHarness, file: &ScratchConfig) {
    show(harness, file, SectionId::Terminal);
    visual(harness).simulate_resize(gpui::size(px(860.0), px(420.0)));
    visual(harness).run_until_parked();
    let tab =
        with_view(harness, |view, _| view.section_handle(SectionId::Terminal));
    keyboard_focus(harness, &tab);
    keys(harness, "tab");
    assert_eq!(focused_in_view(harness), Some(true));
}

#[gpui::test]
fn tab_and_shift_tab_scroll_the_focused_control_fully_into_view(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layout-reveal-tab", "");
    let mut harness = DesktopHarness::new(cx);
    short_terminal(&mut harness, &file);
    let mut moved = false;
    for _ in 0..40 {
        let before = scrolled(&mut harness);
        keys(&harness, "tab");
        let Some(shown) = focused_in_view(&mut harness) else {
            break;
        };
        assert!(shown, "Tab left {} out of view", focus(&mut harness));
        moved |= scrolled(&mut harness) > before;
    }
    assert!(moved, "no Tab reached a control below the fold");
    // Back up from the bottom: each control above the fold scrolls down
    // into view.
    while focused_in_view(&mut harness).is_none() {
        keys(&harness, "shift-tab");
    }
    let mut moved = false;
    for _ in 0..40 {
        let before = scrolled(&mut harness);
        keys(&harness, "shift-tab");
        let Some(shown) = focused_in_view(&mut harness) else {
            break;
        };
        assert!(shown, "Shift-Tab left {} out of view", focus(&mut harness));
        moved |= scrolled(&mut harness) < before;
    }
    assert!(moved, "no Shift-Tab reached a control above the fold");
    assert_eq!(focus(&mut harness), "section:terminal");
}

#[gpui::test]
fn tab_scrolls_a_control_that_draws_its_own_ring_into_view(
    cx: &mut TestAppContext,
) {
    // Add global shortcut is a recorder, which keeps its own focus style
    // until the recorder's migration, at the foot of the Quake section.
    let file = ScratchConfig::new("layout-reveal-recorder", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Quake);
    visual(&harness).simulate_resize(gpui::size(px(860.0), px(420.0)));
    visual(&harness).run_until_parked();
    let tab = with_view(&mut harness, |view, _| {
        view.section_handle(SectionId::Quake)
    });
    keyboard_focus(&mut harness, &tab);
    for _ in 0..60 {
        keys(&harness, "tab");
        if focus(&mut harness) == "action:gadd" {
            break;
        }
    }
    assert_eq!(focus(&mut harness), "action:gadd");
    assert!(scrolled(&mut harness) > px(0.0), "the pane never scrolled");
    assert_eq!(focused_in_view(&mut harness), Some(true));
}

#[gpui::test]
fn focus_moving_to_a_control_in_view_scrolls_nothing(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("layout-reveal-in-view", "");
    let mut harness = DesktopHarness::new(cx);
    short_terminal(&mut harness, &file);
    // Tab until the pane scrolls, so the control before the focused one
    // sits in view above it.
    for _ in 0..40 {
        keys(&harness, "tab");
        if scrolled(&mut harness) > px(0.0) {
            break;
        }
    }
    let at = scrolled(&mut harness);
    assert!(at > px(0.0), "no Tab scrolled the pane");
    keys(&harness, "shift-tab");
    assert_eq!(focused_in_view(&mut harness), Some(true));
    assert_eq!(scrolled(&mut harness), at, "a control in view scrolled");
}

#[gpui::test]
fn a_click_on_a_control_partly_out_of_view_scrolls_nothing(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layout-reveal-pointer", "");
    let mut harness = DesktopHarness::new(cx);
    short_terminal(&mut harness, &file);
    let path = "terminal.close_on_exit";
    // Find where the switch draws: from the keyboard, it scrolls into view.
    let handle = with_view(&mut harness, |view, _| view.stop_handle(path));
    keyboard_focus(&mut harness, &handle);
    let painted: Bounds<Pixels> = with_window(&mut harness, |_, window, cx| {
        huterm_ui::focus::ring_bounds(&handle, window, cx)
    })
    .expect("the switch drew its ring");
    // Scroll so its top sits two points above the pane, its ring hidden;
    // its ring paints there once more, so its bounds stay current.
    let viewport = with_view(&mut harness, |view, _| view.pane_viewport());
    let target =
        scrolled(&mut harness) + (painted.top() - viewport.top()) + px(2.0);
    with_view(&mut harness, |view, cx| {
        view.scroll_pane_to(target);
        cx.notify();
    });
    visual(&harness).run_until_parked();
    assert_eq!(scrolled(&mut harness), target);
    let shifted = with_window(&mut harness, |_, window, cx| {
        huterm_ui::focus::ring_bounds(&handle, window, cx)
    })
    .expect("the switch drew its ring");
    assert_eq!(shifted.top(), viewport.top() - px(2.0));
    // Clicking away and back moves focus to it by the pointer.
    let tab = labelled(&mut harness, "Tab", "Terminal");
    a11y_click(&mut harness, &tab);
    assert_eq!(focus(&mut harness), "section:terminal");
    let switch = a11y_node(&mut harness, &format!("settings-switch-{path}"));
    a11y_click(&mut harness, &switch);
    assert_eq!(focus(&mut harness), path);
    assert_eq!(scrolled(&mut harness), target, "the click scrolled");
}

/// Shows the `logs` Quake profile in a short window, where Add global
/// shortcut sits below the fold, and checks that the keyboard reaching it
/// with nothing open scrolls the pane; then scrolls back to the top.
fn short_quake(
    harness: &mut DesktopHarness,
    file: &ScratchConfig,
) -> FocusHandle {
    show(harness, file, SectionId::Quake);
    visual(harness).simulate_resize(gpui::size(px(860.0), px(420.0)));
    show_logs(harness);
    let add = with_view(harness, |view, _| view.action_handle("gadd"));
    keyboard_focus(harness, &add);
    assert!(
        scrolled(harness) > px(0.0),
        "Add global shortcut was in view"
    );
    let first = with_view(harness, |view, _| view.action_handle("qprof"));
    keyboard_focus(harness, &first);
    with_view(harness, |view, cx| {
        view.scroll_pane_to(px(0.0));
        cx.notify();
    });
    visual(harness).run_until_parked();
    assert_eq!(scrolled(harness), px(0.0));
    add
}

#[gpui::test]
fn focus_moving_under_an_open_menu_scrolls_nothing(cx: &mut TestAppContext) {
    let file = ScratchConfig::new(
        "layout-reveal-menu",
        "[quake.profiles.logs]\nheight = 0.3\n",
    );
    let mut harness = DesktopHarness::new(cx);
    let add = short_quake(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.press("qprof", window, cx);
    });
    visual(&harness).run_until_parked();
    assert!(with_view(&mut harness, |view, _| view.menu_open()));
    keyboard_focus(&mut harness, &add);
    assert_eq!(focus(&mut harness), "action:gadd");
    assert!(with_view(&mut harness, |view, _| view.menu_open()));
    assert_eq!(scrolled(&mut harness), px(0.0), "the pane scrolled");
}

#[gpui::test]
fn focus_moving_under_an_open_dialog_scrolls_nothing(cx: &mut TestAppContext) {
    let file = ScratchConfig::new(
        "layout-reveal-dialog",
        "[quake.profiles.logs]\nheight = 0.3\n",
    );
    let mut harness = DesktopHarness::new(cx);
    let add = short_quake(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.action_handle("qdel").focus(window, cx);
    });
    keys(&harness, "enter");
    assert_eq!(
        with_view(&mut harness, |view, _| view.dialog_name()),
        "delete"
    );
    // Delete profile sits below the fold too, so focus reaching it scrolled.
    with_view(&mut harness, |view, cx| {
        view.scroll_pane_to(px(0.0));
        cx.notify();
    });
    visual(&harness).run_until_parked();
    assert_eq!(scrolled(&mut harness), px(0.0));
    keyboard_focus(&mut harness, &add);
    assert_eq!(focus(&mut harness), "action:gadd");
    assert_eq!(
        with_view(&mut harness, |view, _| view.dialog_name()),
        "delete"
    );
    assert_eq!(scrolled(&mut harness), px(0.0), "the pane scrolled");
}

/// The heading the pane's header shows.
fn heading(harness: &mut DesktopHarness) -> String {
    a11y_find(harness, |node| node["aria"]["role"] == "Heading")
        .expect("the pane draws a heading")["aria"]["label"]
        .as_str()
        .expect("the heading is named")
        .to_owned()
}

fn header(harness: &DesktopHarness) -> Bounds<Pixels> {
    visual(harness)
        .debug_bounds("settings-header")
        .expect("the header painted")
}

#[gpui::test]
fn the_header_names_the_section_above_the_pane_it_scrolls(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layout-header", "");
    let mut harness = DesktopHarness::new(cx);
    short_terminal(&mut harness, &file);
    assert_eq!(heading(&mut harness), "Terminal");
    let fixed = header(&harness);
    // The header spans the top of the window over the pane.
    assert_eq!(fixed.top(), px(0.0));
    let viewport = with_view(&mut harness, |view, _| view.pane_viewport());
    assert_eq!(viewport.top(), fixed.bottom());
    assert_eq!(viewport.left(), fixed.left());
    // The content scrolls under it; the header stays.
    with_view(&mut harness, |view, cx| {
        view.scroll_pane_to(px(120.0));
        cx.notify();
    });
    visual(&harness).run_until_parked();
    assert_eq!(scrolled(&mut harness), px(120.0));
    assert_eq!(header(&harness), fixed);
    assert_eq!(heading(&mut harness), "Terminal");
    // It follows the section, and names a search's results.
    keys(&harness, "shift-tab");
    keys(&harness, "down");
    assert_eq!(section(&mut harness), SectionId::Scrollback);
    assert_eq!(heading(&mut harness), "Scrollback");
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::SETTINGS_SEARCH, window, cx).unwrap();
    });
    let mut typing = visual(&harness);
    typing.simulate_input("font");
    typing.run_until_parked();
    assert_eq!(heading(&mut harness), "Search results");
}

#[gpui::test]
fn the_pane_shows_its_scrollbar_when_it_scrolls(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("layout-scrollbar", "");
    let mut harness = DesktopHarness::new(cx);
    short_terminal(&mut harness, &file);
    let shows = |harness: &mut DesktopHarness| {
        with_view(harness, |view, _| view.pane_scrollbar_shows())
    };
    assert!(!shows(&mut harness), "the indicator shows before a scroll");
    // The wheel over the pane scrolls it and shows the indicator.
    let viewport = with_view(&mut harness, |view, _| view.pane_viewport());
    let mut wheel = visual(&harness);
    wheel.simulate_event(ScrollEvent {
        position: viewport.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-60.0))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
        ..Default::default()
    });
    wheel.run_until_parked();
    assert!(
        scrolled(&mut harness) > px(0.0),
        "the wheel scrolled nothing"
    );
    assert!(shows(&mut harness), "the wheel left the indicator hidden");
    // Tab to a control below the fold shows it too, once it has faded.
    with_view(&mut harness, |view, cx| {
        view.scroll_pane_to(px(0.0));
        view.hide_pane_scrollbar();
        cx.notify();
    });
    visual(&harness).run_until_parked();
    assert!(!shows(&mut harness));
    for _ in 0..40 {
        keys(&harness, "tab");
        if scrolled(&mut harness) > px(0.0) {
            break;
        }
    }
    assert!(scrolled(&mut harness) > px(0.0), "no Tab scrolled the pane");
    assert!(shows(&mut harness), "the reveal left the indicator hidden");
}

#[gpui::test]
fn a_short_window_scrolls_the_sections_between_the_search_and_the_footer(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layout-sidebar-scroll", "");
    // The largest interface text makes the sections overflow the window's
    // minimum height on both platforms; Linux has no title strip and one
    // section fewer.
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.font.ui_size = 16.0;
    });
    show(&mut harness, &file, SectionId::General);
    let height = px(420.0);
    visual(&harness).simulate_resize(gpui::size(px(860.0), height));
    visual(&harness).run_until_parked();
    assert!(
        with_view(&mut harness, |view, _| view.sections_overflow()),
        "the sections fit, so nothing can scroll"
    );
    let search = with_view(&mut harness, |view, _| view.search_bounds())
        .expect("the search field painted");
    let footer = |harness: &mut DesktopHarness| {
        visual(harness)
            .debug_bounds("settings-footer")
            .expect("the footer painted")
    };
    assert!(
        footer(&mut harness).bottom() <= height,
        "the footer overflows"
    );
    let scrolled = |harness: &mut DesktopHarness| {
        with_view(harness, |view, _| view.sections_scrolled())
    };
    assert_eq!(scrolled(&mut harness), px(0.0));
    // Up from the first section wraps to the last, which the list scrolls
    // into view with its ring.
    let general = with_view(&mut harness, |view, _| {
        view.section_handle(SectionId::General)
    });
    keyboard_focus(&mut harness, &general);
    keys(&harness, "up");
    let last = *sections(&mut harness).last().expect("sections");
    assert_eq!(section(&mut harness), last);
    assert!(scrolled(&mut harness) > px(0.0), "the list did not scroll");
    let (ring, viewport) = with_window(&mut harness, |view, window, cx| {
        let focused = window.focused(cx).expect("a focused tab");
        (
            huterm_ui::focus::ring_bounds(&focused, window, cx),
            view.sections_viewport(),
        )
    });
    let ring = ring.expect("the tab drew its ring");
    assert!(
        ring.top() >= viewport.top() && ring.bottom() <= viewport.bottom(),
        "{ring:?} is outside {viewport:?}"
    );
    // The search field and the footer stay where they were.
    assert_eq!(
        with_view(&mut harness, |view, _| view.search_bounds()),
        Some(search)
    );
    assert!(footer(&mut harness).bottom() <= height);
    // Tab from the search returns to the selected tab, scrolled away from
    // by the wheel, and shows it again.
    with_view(&mut harness, |view, cx| {
        view.scroll_sections_to_top();
        cx.notify();
    });
    visual(&harness).run_until_parked();
    assert_eq!(scrolled(&mut harness), px(0.0));
    // The sidebar's search: in Keybindings, the last section on Linux,
    // the search command focuses the section's own search instead.
    let search = with_view(&mut harness, |view, cx| view.search_handle(cx));
    keyboard_focus(&mut harness, &search);
    keys(&harness, "tab");
    assert_eq!(focus(&mut harness), format!("section:{}", last.id()));
    assert!(scrolled(&mut harness) > px(0.0), "Tab left the tab hidden");
    // Next Tab, a command that moves no focus, scrolls back to the first.
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::NEXT_TAB, window, cx).unwrap();
    });
    visual(&harness).run_until_parked();
    assert_eq!(section(&mut harness), SectionId::General);
    assert_eq!(scrolled(&mut harness), px(0.0));
}

#[test]
fn the_traffic_lights_centre_in_the_header_at_the_native_inset() {
    use super::super::settings_window::traffic_lights;
    let native = |x: f64, size: f64| Bounds {
        origin: point(x, 5.0),
        size: gpui::size(size, size),
    };
    assert_eq!(traffic_lights(native(9.0, 14.0)), point(px(9.0), px(19.0)));
    assert_eq!(traffic_lights(native(7.0, 12.0)), point(px(7.0), px(20.0)));
    // Buttons taller than the strip start at its top edge.
    assert_eq!(traffic_lights(native(9.0, 60.0)), point(px(9.0), px(0.0)));
}

#[gpui::test]
fn the_title_strip_leaves_the_search_its_clicks(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("layout-title-strip", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::General);
    let tab = with_view(&mut harness, |view, _| {
        view.section_handle(SectionId::General)
    });
    keyboard_focus(&mut harness, &tab);
    let search = with_view(&mut harness, |view, _| view.search_bounds())
        .expect("the search painted");
    let fixed = header(&harness);
    let mut pointer = visual(&harness);
    // A press on the header, or above the search on macOS, where the
    // traffic lights sit, is the window's.
    pointer.simulate_click(fixed.center(), Modifiers::none());
    if cfg!(target_os = "macos") {
        assert!(search.top() >= fixed.bottom(), "the search is in the strip");
        pointer.simulate_click(
            point(search.center().x, fixed.center().y),
            Modifiers::none(),
        );
    }
    pointer.simulate_click(search.center(), Modifiers::none());
    pointer.run_until_parked();
    assert_eq!(focus(&mut harness), "search");
}

#[gpui::test]
fn a_command_row_toggles_from_its_title_and_from_its_shortcuts(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layout-command-row", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Keybindings);
    visual(&harness).simulate_resize(gpui::size(px(860.0), px(1600.0)));
    visual(&harness).run_until_parked();
    let id = "settings-kcmd-new_tab";
    let expanded =
        |harness: &mut DesktopHarness| a11y_node(harness, id)["aria"].clone();
    let aria = expanded(&mut harness);
    assert_eq!(aria["role"], "Button", "{aria}");
    assert_eq!(aria["label"], "New Tab", "{aria}");
    assert_eq!(aria["expanded"], false, "{aria}");
    let trigger = a11y_node(&mut harness, id);
    a11y_click(&mut harness, &trigger);
    assert_eq!(expanded(&mut harness)["expanded"], true);
    // The shortcut summary is content inside the trigger, which spans the
    // row, so a press on it toggles too.
    let summary = visual(&harness)
        .debug_bounds("settings-ksummary-new_tab")
        .expect("the summary drew");
    let mut pointer = visual(&harness);
    pointer.simulate_click(summary.center(), Modifiers::none());
    pointer.run_until_parked();
    assert_eq!(expanded(&mut harness)["expanded"], false);
}

#[gpui::test]
fn a_command_row_describes_its_shortcut_summary(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("layout-command-summary", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Keybindings);
    let key = if cfg!(target_os = "macos") {
        "cmd-t"
    } else {
        "ctrl-shift-t"
    };
    let summary = |harness: &mut DesktopHarness, id: &str| {
        let description = a11y_node(harness, id)["aria"]["description"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        description
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .to_owned()
    };
    // The trigger draws the summary as key caps, which report no node.
    let new_tab = summary(&mut harness, "settings-kcmd-new_tab");
    let hint = KeyHint::parse(key, Platform::current()).text();
    assert!(new_tab.starts_with(&hint), "{new_tab:?} names {hint:?}");
    // A command without shortcuts says so.
    let unbound = a11y_find(&mut harness, |node| {
        node["element_id"]
            .as_str()
            .is_some_and(|id| id.contains("settings-kcmd-"))
            && node["aria"]["description"]
                .as_str()
                .is_some_and(|text| text.ends_with("\nNo shortcut"))
    });
    assert!(unbound.is_some(), "no command reads No shortcut");
}

#[gpui::test]
fn validation_messages_are_polite_status_nodes_named_by_their_message(
    cx: &mut TestAppContext,
) {
    let original = "[[keybinding]]\nkey = \"ctrl-alt-l\"\n\
                    command = \"resize_pane_right\"\nargs = { amount = 5 }\n";
    let file = ScratchConfig::new("layout-messages", original);
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Keybindings);
    let message = |harness: &mut DesktopHarness, text: &str| {
        a11y_find(harness, |node| {
            node["aria"]["role"] == "Status" && node["aria"]["label"] == text
        })
        .unwrap_or_else(|| panic!("no Status node reads {text:?}"))
    };
    // A shortcut's condition that does not parse.
    with_window(&mut harness, |view, window, cx| {
        view.press("kcmd:resize_pane_right", window, cx);
        view.type_key("when:u0", "Terminal &&", window, cx);
        view.enter_key("when:u0", cx);
    });
    visual(&harness).run_until_parked();
    let error = with_view(&mut harness, |view, _| view.key_error("u0"))
        .expect("the condition's error");
    let node = message(&mut harness, &error);
    assert_eq!(node["aria"]["live"], "Polite", "{node}");
    // A global shortcut without a modifier, in Quake windows.
    with_window(&mut harness, |view, window, cx| {
        view.show_section(SectionId::Quake, cx);
        window.refresh();
    });
    visual(&harness).run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.press("gadd", window, cx);
    });
    visual(&harness).run_until_parked();
    keys(&harness, "a");
    let error = with_view(&mut harness, |view, _| view.global_error("gadd"))
        .expect("the recording's error");
    let node = a11y_node(&mut harness, "settings-message-gadd");
    assert_eq!(node["aria"]["role"], "Status", "{node}");
    assert_eq!(node["aria"]["label"], error.as_str(), "{node}");
    assert_eq!(node["aria"]["live"], "Polite", "{node}");
}

#[gpui::test]
fn the_banner_conflicts_and_an_empty_list_report_their_roles(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layout-alerts", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Quake);
    let empty = a11y_node(&mut harness, "settings-gempty");
    assert_eq!(empty["aria"]["role"], "Group", "{empty}");
    assert_eq!(empty["aria"]["label"], "No global shortcuts", "{empty}");
    // A value the file changed under an edit is a warning alert.
    with_window(&mut harness, |view, window, cx| {
        view.show_section(SectionId::Appearance, cx);
        window.refresh();
    });
    std::fs::write(file.path(), "[font]\nsize = 20.0\n").unwrap();
    with_window(&mut harness, |view, window, cx| {
        view.type_into("font.size", "15", window, cx);
        view.enter("font.size", cx);
    });
    harness.wait_for("the conflict", |cx| {
        settled(cx) && conflicted(cx, "font.size")
    });
    visual(&harness).run_until_parked();
    let conflict = a11y_node(&mut harness, "settings-conflict-font.size");
    assert_eq!(conflict["aria"]["role"], "Status", "{conflict}");
    assert!(
        conflict["aria"]["label"]
            .as_str()
            .is_some_and(|label| !label.is_empty()),
        "{conflict}"
    );
    // An invalid file is a danger alert.
    break_file(&mut harness, &file, "[window\n");
    let banner = a11y_node(&mut harness, "settings-banner");
    assert_eq!(banner["aria"]["role"], "Status", "{banner}");
    assert_eq!(banner["aria"]["label"], "config.toml has an error");
}

#[gpui::test]
fn a_setting_row_names_its_descriptions_and_error(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("layout-field", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Scrollback);
    let path = "terminal.scrollback_rows";
    let id = format!("settings-row-{path}");
    let row = a11y_node(&mut harness, &id);
    assert_eq!(row["aria"]["role"], "Group", "{row}");
    assert_eq!(row["aria"]["label"], "Scrollback rows", "{row}");
    let described = row["aria"]["description"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(!described.is_empty(), "{row}");
    assert!(!described.contains("Must be a whole number"), "{row}");
    with_window(&mut harness, |view, window, cx| {
        view.type_into(path, "big", window, cx);
        view.enter(path, cx);
    });
    visual(&harness).run_until_parked();
    let row = a11y_node(&mut harness, &id);
    assert_eq!(
        row["aria"]["description"],
        format!("{described}\nMust be a whole number"),
        "{row}"
    );
}

#[gpui::test]
fn a_command_search_result_is_a_named_link_that_opens_its_command(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layout-command-result", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::General);
    let search = |harness: &mut DesktopHarness| {
        with_window(harness, |view, window, cx| {
            view.command(ids::SETTINGS_SEARCH, window, cx).unwrap();
        });
        let mut typing = visual(harness);
        typing.simulate_input("split right");
        typing.run_until_parked();
    };
    search(&mut harness);
    let result = a11y_node(&mut harness, "settings-kgo-split_right");
    assert_eq!(result["aria"]["role"], "Link", "{result}");
    assert_eq!(result["aria"]["label"], "Split Right", "{result}");
    a11y_click(&mut harness, &result);
    assert_eq!(section(&mut harness), SectionId::Keybindings);
    assert_eq!(focus(&mut harness), "action:kcmd:split_right");
    // From the keyboard: Return in the search reaches the first result,
    // and Return there opens it.
    show(&mut harness, &file, SectionId::General);
    search(&mut harness);
    keys(&harness, "enter");
    assert_eq!(focus(&mut harness), "action:kgo:split_right");
    let handle = with_view(&mut harness, |view, _| {
        view.action_handle("kgo:split_right")
    });
    assert!(
        with_window(&mut harness, |_, window, cx| {
            huterm_ui::focus::inset_ring_drawn(&handle, window, cx)
        }),
        "the focused result drew no ring"
    );
    keys(&harness, "enter");
    assert_eq!(section(&mut harness), SectionId::Keybindings);
    assert_eq!(focus(&mut harness), "action:kcmd:split_right");
}

#[gpui::test]
fn an_unbound_key_row_draws_its_key_caps_and_names_the_key(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new(
        "layout-unbound-row",
        "[[keybinding]]\nkey = \"alt-shift-k\"\ncommand = \"unbind\"\n",
    );
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Keybindings);
    let row = a11y_node(&mut harness, "settings-kunbound-0");
    assert_eq!(
        row["aria"]["label"],
        KeyHint::parse("alt-shift-k", Platform::current()).text(),
        "{row}"
    );
    assert!(
        visual(&harness)
            .debug_bounds("settings-kunbound-keys-0")
            .is_some(),
        "the key caps did not draw"
    );
}

#[gpui::test]
fn the_interface_font_size_applies_to_an_open_window_on_reload(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("layout-ui-size", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    let title = |harness: &mut DesktopHarness| {
        visual(harness)
            .debug_bounds("settings-label-font.size")
            .expect("the Font size row drew")
    };
    let before = title(&mut harness);
    // The test platform installs no fonts, so the family falls back.
    std::fs::write(
        file.path(),
        "[font]\nui_size = 16.0\nui_family = \"Missing Sans\"\n",
    )
    .unwrap();
    harness.cx.update(|cx| {
        let read = config::reload(&file.path());
        finish_reload(cx, ReloadKind::Manual, &file.path(), read, 0);
    });
    visual(&harness).run_until_parked();
    let family = harness.cx.update(|cx| {
        huterm_ui::typography::Typography::of(cx).family.to_string()
    });
    assert_eq!(family, huterm_ui::typography::SYSTEM_FAMILY);
    let rem = with_window(&mut harness, |_, window, _| window.rem_size());
    assert!(
        (f32::from(rem) - 16.0 * 16.0 / 13.0).abs() < 1e-3,
        "{rem:?}"
    );
    // The row's label and description grow with the scale, without the
    // window reopening.
    let after = title(&mut harness);
    assert!(
        after.size.height > before.size.height,
        "{before:?} -> {after:?}"
    );
}

#[gpui::test]
fn a_search_matches_a_description_as_it_reads(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("layout-search-code", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::SETTINGS_SEARCH, window, cx).unwrap();
    });
    // Pane title bars' description reads "`visible` bars take space",
    // which shows as "visible bars take space".
    let mut typing = visual(&harness);
    typing.simulate_input("visible bars take");
    typing.run_until_parked();
    assert!(
        visual(&harness)
            .debug_bounds("settings-row-panes.title_bar")
            .is_some(),
        "the search across a code span found no row"
    );
}

/// A row of each control type: a switch, a select, a segmented control, a
/// slider, a stepper, modifier chips, and a text field, and Theme mode,
/// whose theme cards sit under its line.
const CONTROL_ROWS: [(SectionId, &str); 9] = [
    (SectionId::Terminal, "terminal.bell.visual"),
    (SectionId::Tabs, "tabs.label"),
    (SectionId::Tabs, "tabs.style"),
    (SectionId::Panes, "panes.inactive_dim"),
    (SectionId::Panes, "panes.inactive_dim_background"),
    (SectionId::Appearance, "font.size"),
    (SectionId::Terminal, "terminal.link_modifiers"),
    (SectionId::Scrollback, "terminal.scrollback_bytes"),
    (SectionId::Appearance, "theme"),
];

/// The height of each of [`CONTROL_ROWS`], in order, and whether it drew
/// its Reset.
fn control_row_heights(harness: &mut DesktopHarness) -> Vec<(Pixels, bool)> {
    CONTROL_ROWS
        .iter()
        .map(|(section, path)| {
            with_window(harness, |view, window, cx| {
                view.show_section(*section, cx);
                window.refresh();
            });
            let mut drawn = visual(harness);
            drawn.run_until_parked();
            let height = drawn
                .debug_bounds(format!("settings-row-{path}").leak())
                .unwrap_or_else(|| panic!("the {path} row drew"))
                .size
                .height;
            let reset = drawn
                .debug_bounds(format!("settings-reset-{path}").leak())
                .is_some();
            (height, reset)
        })
        .collect()
}

#[gpui::test]
fn a_rows_height_does_not_depend_on_whether_the_file_sets_it(
    cx: &mut TestAppContext,
) {
    // Advanced settings show, for the size limit.
    let advanced = "[settings]\nshow_advanced = true\n\n";
    let file = ScratchConfig::new("layout-row-heights", advanced);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    // The narrowest window leaves labels and descriptions the least room.
    visual(&harness).simulate_resize(gpui::size(px(640.0), px(1600.0)));
    let unset = control_row_heights(&mut harness);
    assert!(unset.iter().all(|(_, reset)| !reset), "{unset:?}");
    // Each row set to its default, or the size limit, which has none, to a
    // value, so only the dot, Reset, and the field's text change.
    let link = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    std::fs::write(
        file.path(),
        format!(
            "{advanced}[terminal]\nlink_modifiers = \"{link}\"\n\
             scrollback_bytes = \"64MiB\"\n\n\
             [terminal.bell]\nvisual = false\n\n\
             [tabs]\nlabel = \"smart\"\nstyle = \"pill\"\n\n\
             [panes]\ninactive_dim = 0.2\ninactive_dim_background = 0.0\n\n\
             [font]\nsize = 14.0\n\n\
             [theme]\nname = \"tokyo-night\"\n"
        ),
    )
    .unwrap();
    with_view(&mut harness, SettingsView::reread);
    harness.wait_for("Settings to read the file", settled);
    let set = control_row_heights(&mut harness);
    assert!(set.iter().all(|(_, reset)| *reset), "{set:?}");
    let heights = |rows: &[(Pixels, bool)]| -> Vec<(&str, Pixels)> {
        CONTROL_ROWS
            .iter()
            .zip(rows)
            .map(|((_, path), (height, _))| (*path, *height))
            .collect()
    };
    assert_eq!(heights(&set), heights(&unset), "set (left), unset (right)");
}
