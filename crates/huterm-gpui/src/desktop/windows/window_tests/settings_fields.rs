//! Settings fields, number fields, and sliders on `huterm-ui`'s `Input`,
//! `InputGroup`, `NumberField`, and `Slider`, driven by real keyboard and
//! pointer events: D7, D10, and D12 of `docs/plans/ui-components.md`, the
//! search field's clear button, `text_cut`, and the Page Up, Page Down,
//! Home, and End keys on number fields and sliders.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{AccessibleAction, Keystroke, Modifiers, MouseButton, point, px};
use huterm_config::settings::SectionId;

use super::settings::{
    ScratchConfig, settings, settled, with_view, with_window,
};
use super::settings_controls::{
    a11y_act, a11y_click, a11y_find, a11y_node, break_file, focus, keys, ring,
    show, visual,
};
use super::*;

/// The platform's select-all and cut keys, as the default keymap binds
/// them in fields.
const SELECT_ALL: &str = if cfg!(target_os = "macos") {
    "cmd-a"
} else {
    "ctrl-a"
};
const CUT: &str = if cfg!(target_os = "macos") {
    "cmd-x"
} else {
    "ctrl-x"
};

/// Opens Settings on the Quake windows section showing the `logs` profile.
fn show_logs(harness: &mut DesktopHarness, file: &ScratchConfig) {
    show(harness, file, SectionId::Quake);
    with_window(harness, |view, window, _| {
        view.pick_profile("logs");
        window.refresh();
    });
    visual(harness).run_until_parked();
}

/// Clicks the middle of the setting's field.
fn click_field(harness: &mut DesktopHarness, path: &str) {
    let (_, bounds) =
        with_window(harness, |view, _, cx| view.field_caret(path, cx))
            .expect("the field painted");
    let mut visual = visual(harness);
    visual.simulate_click(bounds.center(), Modifiers::none());
    visual.run_until_parked();
}

/// Replaces the focused field's text, as selecting it all and typing does.
fn type_over(harness: &DesktopHarness, text: &str) {
    keys(harness, SELECT_ALL);
    let mut visual = visual(harness);
    visual.simulate_input(text);
    visual.run_until_parked();
}

fn field_text(harness: &mut DesktopHarness, path: &str) -> String {
    with_window(harness, |view, _, cx| view.field_value(path, cx))
}

#[gpui::test]
fn a_slider_steps_by_five_percent_and_stops_at_its_ends(
    cx: &mut TestAppContext,
) {
    // D7: stepping a Quake width or height added 1 and wrote values out of
    // (0, 1]. The floor is one step, so stepping down never writes 0.
    let width = "quake.profiles.logs.width";
    let height = "quake.profiles.logs.height";
    let file = ScratchConfig::new(
        "fields-slider-ends",
        "[quake.profiles.logs]\nwidth = 0.95\nheight = 0.1\n",
    );
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop(width, window, cx);
    });
    keys(&harness, "up");
    harness.wait_for("the step up", |cx| {
        settled(cx) && file.text().contains("width = 1.0")
    });
    keys(&harness, "up");
    assert!(
        harness.cx.update(settled),
        "a step past 100% queued a write"
    );
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop(height, window, cx);
    });
    keys(&harness, "down");
    harness.wait_for("the step down", |cx| {
        settled(cx) && file.text().contains("height = 0.05")
    });
    keys(&harness, "down");
    assert!(harness.cx.update(settled), "a step below 5% queued a write");
    // Nor did it step out of range into an error, as stepping by 1 did.
    for path in [width, height] {
        assert_eq!(with_view(&mut harness, |view, _| view.error(path)), None);
    }
    assert_eq!(
        file.text(),
        "[quake.profiles.logs]\nwidth = 1.0\nheight = 0.05\n"
    );
    assert_eq!(focus(&mut harness), height);
}

#[gpui::test]
fn a_slider_shows_an_off_grid_value_as_is_and_steps_to_the_grid(
    cx: &mut TestAppContext,
) {
    let width = "quake.profiles.logs.width";
    let file = ScratchConfig::new(
        "fields-slider-grid",
        "[quake.profiles.logs]\nwidth = 0.33\n",
    );
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    let id = format!("settings-slider-{width}");
    let aria = a11y_node(&mut harness, &id)["aria"].clone();
    assert_eq!(aria["role"], "Slider", "{aria}");
    assert_eq!(aria["value"], "33%", "{aria}");
    assert_eq!(aria["numeric_value"], 0.33, "{aria}");
    assert_eq!(aria["min_numeric_value"], 0.05, "{aria}");
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop(width, window, cx);
    });
    // The first step lands on the next grid value, not 0.33 + 0.05.
    keys(&harness, "up");
    harness.wait_for("the step up", |cx| {
        settled(cx) && file.text().contains("0.35")
    });
    keys(&harness, "down down");
    harness.wait_for("the steps down", |cx| {
        settled(cx) && file.text().contains("0.25")
    });
    assert_eq!(file.text(), "[quake.profiles.logs]\nwidth = 0.25\n");
    let aria = a11y_node(&mut harness, &id)["aria"].clone();
    assert_eq!(aria["value"], "25%", "{aria}");
}

#[gpui::test]
fn a_slider_row_keeps_its_description_beside_the_slider(
    cx: &mut TestAppContext,
) {
    // The slider took the row's width, so the description wrapped one
    // character per line and Reset drew over the track.
    let file = ScratchConfig::new(
        "fields-slider-layout",
        "[quake.profiles.logs]\nwidth = 0.3\nheight = 0.4\n",
    );
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    for (label, slider, reset) in [
        (
            "settings-label-quake.profiles.logs.width",
            "settings-slider-quake.profiles.logs.width",
            "settings-reset-quake.profiles.logs.width",
        ),
        (
            "settings-label-quake.profiles.logs.height",
            "settings-slider-quake.profiles.logs.height",
            "settings-reset-quake.profiles.logs.height",
        ),
    ] {
        let mut visual = visual(&harness);
        let mut bounds = |selector| {
            visual
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} was drawn"))
        };
        let (label, slider, reset) =
            (bounds(label), bounds(slider), bounds(reset));
        assert!(
            label.right() <= slider.left(),
            "{label:?} overlaps {slider:?}"
        );
        assert!(label.size.width > px(100.0), "{label:?} is squeezed");
        assert!(
            reset.right() <= slider.left() && !reset.intersects(&slider),
            "{reset:?} overlaps {slider:?}"
        );
    }
}

#[gpui::test]
fn a_slider_shows_the_default_while_its_reset_saves(cx: &mut TestAppContext) {
    // A pending reset holds no value, and the slider showed its minimum.
    let width = "quake.profiles.logs.width";
    let file = ScratchConfig::new(
        "fields-slider-reset",
        "[quake.profiles.logs]\nwidth = 0.3\n",
    );
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    let id = format!("settings-slider-{width}");
    with_window(&mut harness, |view, window, cx| {
        view.press(&format!("reset:{width}"), window, cx);
    });
    assert!(!harness.cx.update(settled), "the reset already saved");
    let aria = a11y_node(&mut harness, &id)["aria"].clone();
    assert_eq!(aria["value"], "100%", "{aria}");
    assert_eq!(aria["numeric_value"], 1.0, "{aria}");
    harness.wait_for("the reset", |cx| {
        settled(cx) && !file.text().contains("width")
    });
    let aria = a11y_node(&mut harness, &id)["aria"].clone();
    assert_eq!(aria["value"], "100%", "{aria}");
}

#[gpui::test]
fn a_slider_drag_past_its_start_writes_its_floor(cx: &mut TestAppContext) {
    let width = "quake.profiles.logs.width";
    let file = ScratchConfig::new(
        "fields-slider-drag",
        "[quake.profiles.logs]\nwidth = 0.5\n",
    );
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    let mut visual = visual(&harness);
    let slider = visual
        .debug_bounds("settings-slider-quake.profiles.logs.width")
        .expect("the slider was drawn");
    let y = slider.center().y;
    visual.simulate_mouse_down(
        point(slider.left() + px(40.0), y),
        MouseButton::Left,
        Modifiers::none(),
    );
    visual.run_until_parked();
    let past = point(slider.left() - px(60.0), y);
    visual.simulate_mouse_move(
        past,
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    visual.run_until_parked();
    visual.simulate_mouse_up(past, MouseButton::Left, Modifiers::none());
    visual.run_until_parked();
    harness.wait_for("the drag to save", settled);
    // Never 0, which the schema's exclusive minimum refuses.
    assert_eq!(with_view(&mut harness, |view, _| view.error(width)), None);
    assert_eq!(file.text(), "[quake.profiles.logs]\nwidth = 0.05\n");
    assert_eq!(focus(&mut harness), width);
}

#[gpui::test]
fn an_accessibility_increment_steps_a_slider_by_five_percent(
    cx: &mut TestAppContext,
) {
    let width = "quake.profiles.logs.width";
    let file = ScratchConfig::new(
        "fields-slider-a11y",
        "[quake.profiles.logs]\nwidth = 0.5\n",
    );
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    let node = a11y_node(&mut harness, &format!("settings-slider-{width}"));
    a11y_act(&mut harness, &node, AccessibleAction::Increment);
    harness.wait_for("the step to save", settled);
    assert_eq!(file.text(), "[quake.profiles.logs]\nwidth = 0.55\n");
}

/// The update interval is macOS's alone; `values` tests the step itself
/// on every platform.
#[cfg(target_os = "macos")]
#[gpui::test]
fn an_empty_update_interval_steps_down_to_its_minimum(cx: &mut TestAppContext) {
    // It has a minimum of 1 and no maximum or default, so a step down from
    // empty went to a stand-in maximum of 2^40 hours.
    let path = "updates.check_interval_hours";
    let file = ScratchConfig::new("fields-empty-interval", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Updates);
    for step in ["down", "pagedown", "Decrease"] {
        assert_eq!(field_text(&mut harness, path), "", "before {step}");
        with_window(&mut harness, |view, window, cx| {
            view.focus_field(path, window, cx);
        });
        if step == "Decrease" {
            let button =
                a11y_find(&mut harness, |node| node["aria"]["label"] == step)
                    .expect("a Decrease stepper");
            a11y_click(&mut harness, &button);
        } else {
            keys(&harness, step);
        }
        harness.wait_for(&format!("{step} to save"), |cx| {
            settled(cx) && file.text().contains("check_interval_hours")
        });
        assert_eq!(
            file.text(),
            "[updates]\ncheck_interval_hours = 1\n",
            "{step}"
        );
        assert_eq!(field_text(&mut harness, path), "1", "{step}");
        with_window(&mut harness, |view, window, cx| {
            view.press(&format!("reset:{path}"), window, cx);
        });
        harness.wait_for("the reset", |cx| {
            settled(cx) && !file.text().contains("check_interval_hours")
        });
    }
}

#[gpui::test]
fn a_number_field_steps_and_typing_clamp_to_its_range(cx: &mut TestAppContext) {
    // D7 for a number setting: font sizes run from 6 to 96.
    let path = "font.size";
    let file = ScratchConfig::new("fields-number", "[font]\nsize = 95.0\n");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(path, window, cx);
    });
    keys(&harness, "up");
    harness.wait_for("the step up", |cx| {
        settled(cx) && file.text().contains("96.0")
    });
    keys(&harness, "up");
    assert!(harness.cx.update(settled), "a step past 96 queued a write");
    assert_eq!(with_view(&mut harness, |view, _| view.error(path)), None);
    assert_eq!(field_text(&mut harness, path), "96");
    // A typed number out of range saves, and shows, the nearest bound.
    type_over(&harness, "1");
    keys(&harness, "enter");
    harness.wait_for("the commit to settle", settled);
    assert_eq!(with_view(&mut harness, |view, _| view.error(path)), None);
    assert_eq!(file.text(), "[font]\nsize = 6.0\n");
    assert_eq!(field_text(&mut harness, path), "6");
}

#[gpui::test]
fn enter_commits_a_number_field_and_escape_reverts_it(cx: &mut TestAppContext) {
    let path = "font.size";
    let file = ScratchConfig::new("fields-commit", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(path, window, cx);
    });
    type_over(&harness, "20");
    assert!(harness.cx.update(settled), "typing alone queued a write");
    keys(&harness, "enter");
    harness.wait_for("the commit", |cx| {
        settled(cx) && file.text().contains("20.0")
    });
    type_over(&harness, "30");
    keys(&harness, "escape");
    assert_eq!(field_text(&mut harness, path), "20");
    assert!(harness.cx.update(settled), "Escape queued a write");
    assert_eq!(file.text(), "[font]\nsize = 20.0\n");
}

#[gpui::test]
fn a_read_only_field_takes_no_focus_refuses_typing_and_says_why(
    cx: &mut TestAppContext,
) {
    // D10: a read-only field took focus from a click and accepted typing,
    // which the save then dropped, and nothing said why.
    let path = "font.family";
    let file =
        ScratchConfig::new("fields-read-only", "[font]\nfamily = \"Menlo\"\n");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    // The field has focus when the file breaks; its own editing refuses.
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(path, window, cx);
    });
    let broken = "[font]\nfamily = \"Menlo\"\n[window\n";
    break_file(&mut harness, &file, broken);
    assert_eq!(focus(&mut harness), path);
    let mut typing = visual(&harness);
    typing.simulate_input("Courier");
    typing.run_until_parked();
    keys(&harness, "backspace enter");
    assert_eq!(field_text(&mut harness, path), "Menlo");
    // A click on the field takes no focus.
    with_window(&mut harness, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx);
    });
    click_field(&mut harness, path);
    assert_ne!(focus(&mut harness), path, "the click focused the field");
    let aria =
        a11y_node(&mut harness, &format!("settings-field-{path}"))["aria"]
            .clone();
    assert_eq!(aria["disabled"], true, "{aria}");
    assert_eq!(
        aria["description"], "Read-only until config.toml is fixed",
        "{aria}"
    );
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), broken);
}

#[gpui::test]
fn a_long_value_keeps_the_caret_in_view(cx: &mut TestAppContext) {
    // D12: the field never scrolled, so the caret ran past its edge.
    let path = "font.family";
    let file = ScratchConfig::new("fields-long", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(path, window, cx);
    });
    type_over(&harness, &"Long Family Name ".repeat(6));
    let (caret, visible) =
        with_window(&mut harness, |view, _, cx| view.field_caret(path, cx))
            .expect("the field painted");
    assert!(
        caret >= visible.left() && caret <= visible.right(),
        "the caret at {caret:?} left {visible:?}"
    );
    keys(&harness, "home");
    let (caret, visible) =
        with_window(&mut harness, |view, _, cx| view.field_caret(path, cx))
            .expect("the field painted");
    assert_eq!(caret, visible.left(), "Home scrolled back to the start");
}

#[gpui::test]
fn a_field_shows_its_ring_after_tab_and_none_after_a_click(
    cx: &mut TestAppContext,
) {
    let path = "font.family";
    let file = ScratchConfig::new("fields-ring", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    click_field(&mut harness, path);
    assert_eq!(focus(&mut harness), path);
    let handle =
        with_window(&mut harness, |view, _, cx| view.field_handle(path, cx));
    assert!(!ring(&mut harness, &handle), "a click drew the ring");
    // Back and forth with the keyboard lands on it again, with the ring.
    keys(&harness, "shift-tab tab");
    assert_eq!(focus(&mut harness), path);
    assert!(ring(&mut harness, &handle), "Tab drew no ring");
}

#[gpui::test]
fn the_search_clear_button_empties_the_search(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("fields-clear", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::General);
    // An empty search offers nothing to clear.
    let clear = |harness: &mut DesktopHarness| {
        a11y_find(harness, |node| node["aria"]["label"] == "Clear search")
    };
    assert!(clear(&mut harness).is_none());
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::SETTINGS_SEARCH, window, cx).unwrap();
    });
    let mut typing = visual(&harness);
    typing.simulate_input("font");
    typing.run_until_parked();
    let button = clear(&mut harness).expect("a clear button");
    a11y_click(&mut harness, &button);
    assert_eq!(
        with_window(&mut harness, |view, _, cx| view.search_text(cx)),
        ""
    );
    // The press leaves focus in the search, ready for a new query.
    assert_eq!(focus(&mut harness), "search");
    assert!(clear(&mut harness).is_none());
}

#[gpui::test]
fn text_cut_moves_a_fields_selection_to_the_clipboard(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("fields-cut", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::General);
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::SETTINGS_SEARCH, window, cx).unwrap();
    });
    let mut typing = visual(&harness);
    typing.simulate_input("scrollback");
    typing.run_until_parked();
    keys(&harness, &format!("{SELECT_ALL} {CUT}"));
    assert_eq!(
        with_window(&mut harness, |view, _, cx| view.search_text(cx)),
        ""
    );
    let clipboard = harness
        .cx
        .update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(clipboard.as_deref(), Some("scrollback"));
}

/// The Edit menu's Cut, Copy, Paste, and Select All are enabled while a
/// field has focus, and act on it: Cut is the field's `text_cut`, and the
/// terminal's Copy, Paste, and Select All run as the field's commands. On
/// any other control they are disabled, and the terminal's other items,
/// such as Clear Scrollback, stay disabled in a field too.
#[gpui::test]
fn the_edit_menu_acts_on_a_focused_field_and_only_there(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("fields-edit-menu", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Tabs);
    let edit = [
        ids::TEXT_CUT,
        ids::COPY,
        ids::PASTE,
        ids::SELECT_ALL,
        ids::CLEAR_SCROLLBACK,
    ];
    let available = |harness: &mut DesktopHarness| {
        with_window(harness, |_, window, cx| {
            edit.map(|id| {
                window.is_action_available(
                    crate::commands::invoke(id).into_boxed().as_ref(),
                    cx,
                )
            })
        })
    };
    let run = |harness: &mut DesktopHarness, id| {
        with_window(harness, |_, window, cx| {
            window
                .dispatch_action(crate::commands::invoke(id).into_boxed(), cx);
        });
        visual(harness).run_until_parked();
    };
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::SETTINGS_SEARCH, window, cx).unwrap();
    });
    let mut typing = visual(&harness);
    typing.simulate_input("scrollback");
    typing.run_until_parked();
    assert_eq!(available(&mut harness), [true, true, true, true, false]);
    run(&mut harness, ids::SELECT_ALL);
    run(&mut harness, ids::COPY);
    assert_eq!(clipboard(&mut harness).as_deref(), Some("scrollback"));
    run(&mut harness, ids::TEXT_CUT);
    assert_eq!(
        with_window(&mut harness, |view, _, cx| view.search_text(cx)),
        ""
    );
    run(&mut harness, ids::PASTE);
    assert_eq!(
        with_window(&mut harness, |view, _, cx| view.search_text(cx)),
        "scrollback"
    );
    // A switch is no field.
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop("tabs.always_show", window, cx);
    });
    visual(&harness).run_until_parked();
    assert_eq!(available(&mut harness), [false; 5]);
}

/// Right-presses the first character the field shows in `bounds`, as a
/// context click does.
fn right_press(harness: &DesktopHarness, bounds: gpui::Bounds<gpui::Pixels>) {
    let mut visual = visual(harness);
    let at = point(bounds.left() + px(2.0), bounds.center().y);
    visual.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    visual.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    visual.run_until_parked();
}

fn clipboard(harness: &mut DesktopHarness) -> Option<String> {
    harness
        .cx
        .update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
}

/// A field's context menu edits the field and returns focus to it, and
/// its draft stays a draft while the menu is open: the field commits only
/// when focus goes somewhere else. A number field's own `Input` asks for
/// the menu as any field does.
#[gpui::test]
fn a_fields_context_menu_edits_it_and_keeps_its_draft_until_focus_leaves(
    cx: &mut TestAppContext,
) {
    let path = "terminal.scrollback_rows";
    let file = ScratchConfig::new("fields-menu", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Scrollback);
    // GPUI reports blurs only in an active window.
    with_window(&mut harness, |_, window, _| window.activate_window());
    click_field(&mut harness, path);
    type_over(&harness, "5000");
    let (_, bounds) =
        with_window(&mut harness, |view, _, cx| view.field_caret(path, cx))
            .expect("the field painted");
    let menu = |harness: &mut DesktopHarness| {
        with_view(harness, |view, cx| {
            (
                view.field_menu_shown(),
                view.menu_ids(cx),
                view.menu_disabled_ids(cx),
            )
        })
    };
    // With nothing selected, Cut and Copy say so, and the draft waits.
    right_press(&harness, bounds);
    assert_eq!(
        menu(&mut harness),
        (
            true,
            ["text_cut", "text_copy", "text_paste", "text_select_all"]
                .map(str::to_owned)
                .to_vec(),
            ["text_cut", "text_copy"].map(str::to_owned).to_vec(),
        )
    );
    assert!(
        harness.cx.update(settled) && file.text().is_empty(),
        "opening the menu saved"
    );
    keys(&harness, "escape");
    assert!(!menu(&mut harness).0);
    assert_eq!(focus(&mut harness), path);
    // A press on the selection keeps it, so Cut is the first item.
    keys(&harness, SELECT_ALL);
    right_press(&harness, bounds);
    keys(&harness, "down enter");
    assert_eq!(field_text(&mut harness, path), "");
    assert_eq!(clipboard(&mut harness).as_deref(), Some("5000"));
    assert_eq!(focus(&mut harness), path);
    // An empty field offers Paste alone.
    right_press(&harness, bounds);
    keys(&harness, "down enter");
    assert_eq!(field_text(&mut harness, path), "5000");
    assert!(harness.cx.update(settled), "an edit from the menu saved");
    assert_eq!(file.text(), "");
    // Focus leaving the open menu leaves the field too, which saves.
    right_press(&harness, bounds);
    with_window(&mut harness, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx);
    });
    harness.cx.run_until_parked();
    assert!(!menu(&mut harness).0);
    harness.wait_for("the field to save", |cx| {
        settled(cx) && file.text().contains("scrollback_rows = 5000")
    });
}

/// Closing Settings, or moving to another section, while a field's menu
/// is open saves the field's draft, as it would with focus in the field.
#[gpui::test]
fn closing_or_changing_section_with_a_fields_menu_open_saves_its_draft(
    cx: &mut TestAppContext,
) {
    let path = "terminal.scrollback_rows";
    for (name, command) in
        [("close", ids::CLOSE_WINDOW), ("tab", ids::NEXT_TAB)]
    {
        let file = ScratchConfig::new(&format!("fields-menu-{name}"), "");
        let mut harness = DesktopHarness::new(cx);
        show(&mut harness, &file, SectionId::Scrollback);
        with_window(&mut harness, |_, window, _| window.activate_window());
        click_field(&mut harness, path);
        type_over(&harness, "5000");
        let (_, bounds) =
            with_window(&mut harness, |view, _, cx| view.field_caret(path, cx))
                .expect("the field painted");
        right_press(&harness, bounds);
        assert!(with_view(&mut harness, |view, _| view.field_menu_shown()));
        with_window(&mut harness, |view, window, cx| {
            let _ = view.command(command, window, cx);
        });
        // Settings closes once the write lands, so wait on the file.
        harness.wait_for(&format!("{name} to save the draft"), |_| {
            file.text().contains("scrollback_rows = 5000")
        });
        if command == ids::CLOSE_WINDOW {
            harness.wait_for("Settings to close", |cx| settings(cx).is_none());
        }
    }
}

/// A field's menu opens over a dialog and takes its keys, which the open
/// dialog would otherwise take for itself.
#[gpui::test]
fn a_dialog_fields_context_menu_takes_its_keys(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("fields-dialog-menu", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Quake);
    with_window(&mut harness, |view, window, cx| {
        view.type_profile_name("scratch", window, cx);
    });
    visual(&harness).run_until_parked();
    let (_, bounds) =
        with_window(&mut harness, |view, _, cx| view.dialog_field_caret(cx))
            .expect("the dialog's field painted");
    keys(&harness, SELECT_ALL);
    right_press(&harness, bounds);
    assert!(with_view(&mut harness, |view, _| view.field_menu_shown()));
    keys(&harness, "down enter");
    assert_eq!(clipboard(&mut harness).as_deref(), Some("scratch"));
    with_view(&mut harness, |view, cx| {
        assert!(!view.field_menu_shown());
        assert_eq!(view.quake_prompts(), (true, false));
        assert_eq!(view.profile_name_text(cx), "");
    });
}

/// Sends `key` and waits for the file to read exactly `text`.
fn key_writes(
    harness: &mut DesktopHarness,
    file: &ScratchConfig,
    key: &str,
    text: &str,
) {
    keys(harness, key);
    harness.wait_for(&format!("{key} to write {text:?}"), |cx| {
        settled(cx) && file.text() == text
    });
}

/// Sends each of `keys` and asserts, before any background task runs, that
/// it queued no write. A write against an invalid file fails on disk, so
/// only the queue shows whether the control refused.
fn assert_keys_queue_no_write(harness: &mut DesktopHarness, keys: &str) {
    let window = harness
        .cx
        .update(|cx| settings(cx).expect("Settings is open"));
    for key in keys.split(' ') {
        let keystroke = Keystroke::parse_with_os(
            key,
            crate::keymap::Platform::current().os(),
        )
        .expect("a valid keystroke");
        harness.cx.dispatch_keystroke(window.into(), keystroke);
        assert!(harness.cx.update(settled), "{key} queued a write");
    }
    visual(harness).run_until_parked();
}

#[gpui::test]
fn page_keys_page_a_slider_and_home_and_end_reach_its_ends(
    cx: &mut TestAppContext,
) {
    let width = "quake.profiles.logs.width";
    let wrote =
        |value: &str| format!("[quake.profiles.logs]\nwidth = {value}\n");
    let file = ScratchConfig::new("fields-slider-pages", &wrote("0.95"));
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop(width, window, cx);
    });
    // A page is a tenth of the range, two 5% steps, clamped at 100%.
    key_writes(&mut harness, &file, "pageup", &wrote("1.0"));
    key_writes(&mut harness, &file, "pagedown", &wrote("0.9"));
    key_writes(&mut harness, &file, "home", &wrote("0.05"));
    key_writes(&mut harness, &file, "pageup", &wrote("0.15"));
    key_writes(&mut harness, &file, "end", &wrote("1.0"));
    assert_eq!(focus(&mut harness), width);
}

#[gpui::test]
fn page_keys_page_a_number_field_and_home_and_end_move_its_caret(
    cx: &mut TestAppContext,
) {
    // Font sizes run from 6 to 96; a page is a tenth of that, nine steps.
    let path = "font.size";
    let file =
        ScratchConfig::new("fields-number-pages", "[font]\nsize = 90.0\n");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(path, window, cx);
    });
    key_writes(&mut harness, &file, "pageup", "[font]\nsize = 96.0\n");
    key_writes(&mut harness, &file, "pagedown", "[font]\nsize = 87.0\n");
    // Home and End move the caret in the text, so typing lands at each
    // end, and write nothing.
    keys(&harness, "home");
    let mut typing = visual(&harness);
    typing.simulate_input("1");
    typing.run_until_parked();
    assert_eq!(field_text(&mut harness, path), "187");
    keys(&harness, "end");
    let mut typing = visual(&harness);
    typing.simulate_input("0");
    typing.run_until_parked();
    assert_eq!(field_text(&mut harness, path), "1870");
    assert!(harness.cx.update(settled), "Home or End queued a write");
    keys(&harness, "escape");
    assert_eq!(field_text(&mut harness, path), "87");
    assert_eq!(file.text(), "[font]\nsize = 87.0\n");
}

#[gpui::test]
fn page_home_and_end_elsewhere_in_settings_stay_unbound_and_write_nothing(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("fields-pages-elsewhere", "");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Tabs);
    // What each keystroke resolved to: none on a control that is neither
    // a slider nor a number field, so the key is not consumed.
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
    for path in ["tabs.always_show", "tabs.style"] {
        with_window(&mut harness, |view, window, cx| {
            view.focus_stop(path, window, cx);
        });
        keys(&harness, "pageup pagedown home end");
        assert_eq!(focus(&mut harness), path);
    }
    let resolved = resolved.borrow().clone();
    assert_eq!(resolved.len(), 8, "{resolved:?}");
    assert!(
        resolved.iter().all(|(_, action)| action.is_none()),
        "a key resolved to a binding: {resolved:?}"
    );
    // A text field keeps its own palette commands for the keys.
    let path = "font.family";
    with_window(&mut harness, |view, window, cx| {
        view.show_section(SectionId::Appearance, cx);
        window.refresh();
    });
    visual(&harness).run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(path, window, cx);
    });
    type_over(&harness, "Menlo");
    keys(&harness, "pageup pagedown");
    assert_eq!(field_text(&mut harness, path), "Menlo");
    assert!(harness.cx.update(settled), "a page key queued a write");
    assert_eq!(file.text(), "");
}

#[gpui::test]
fn page_home_and_end_refuse_on_a_read_only_slider(cx: &mut TestAppContext) {
    let width = "quake.profiles.logs.width";
    let file = ScratchConfig::new(
        "fields-slider-read-only",
        "[quake.profiles.logs]\nwidth = 0.3\n",
    );
    let mut harness = DesktopHarness::new(cx);
    show_logs(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.focus_stop(width, window, cx);
    });
    let broken = "[quake.profiles.logs]\nwidth = 0.3\n[window\n";
    break_file(&mut harness, &file, broken);
    // Focus stays, so a key reaching the window root cannot pass for a
    // refusal.
    assert_eq!(focus(&mut harness), width);
    assert_keys_queue_no_write(&mut harness, "pageup pagedown home end");
    assert_eq!(file.text(), broken);
}

#[gpui::test]
fn page_keys_refuse_on_a_read_only_number_field(cx: &mut TestAppContext) {
    let path = "font.size";
    let file =
        ScratchConfig::new("fields-number-read-only", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    show(&mut harness, &file, SectionId::Appearance);
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(path, window, cx);
    });
    let broken = "[font]\nsize = 13.0\n[window\n";
    break_file(&mut harness, &file, broken);
    assert_eq!(focus(&mut harness), path);
    assert_keys_queue_no_write(&mut harness, "pageup pagedown");
    assert_eq!(field_text(&mut harness, path), "13");
    assert_eq!(file.text(), broken);
}
