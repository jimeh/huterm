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

use super::settings::{
    ScratchConfig, conflicted, settled, with_view, with_window,
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
        ("right", SectionId::Tabs),
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
    with_view(harness, |view, _| {
        let names = view.theme_names();
        let selected = view.selected();
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

/// A click on a theme card draws no ring, and keyboard focus on it does.
fn a_theme_card_rings_for_the_keyboard_alone(harness: &mut DesktopHarness) {
    let (names, start) = themes(harness);
    let other = names[(start + 1) % names.len()].clone();
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
fn the_theme_cards_are_one_tab_stop_of_radio_buttons(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("layout-theme-stop", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    let (names, start) = themes(&mut harness);
    let group = labelled(&mut harness, "RadioGroup", "Theme");
    assert_eq!(group["aria"]["size_of_set"], names.len(), "{group}");
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
fn a_command_row_toggles_from_its_trigger_and_not_from_its_shortcuts(
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
    // A press on the shortcuts beside the trigger leaves the row open.
    let summary = visual(&harness)
        .debug_bounds("settings-ksummary-new_tab")
        .expect("the summary drew");
    let mut pointer = visual(&harness);
    pointer.simulate_click(summary.center(), Modifiers::none());
    pointer.run_until_parked();
    assert_eq!(expanded(&mut harness)["expanded"], true);
    let trigger = a11y_node(&mut harness, id);
    a11y_click(&mut harness, &trigger);
    assert_eq!(expanded(&mut harness)["expanded"], false);
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
