//! Settings fields, number fields, and sliders on `huterm-ui`'s `Input`,
//! `InputGroup`, `NumberField`, and `Slider`, driven by real keyboard and
//! pointer events: D7, D10, and D12 of `docs/plans/ui-components.md`, the
//! search field's clear button, and `text_cut`.

use gpui::Modifiers;
use huterm_config::settings::SectionId;

use super::settings::{ScratchConfig, settled, with_view, with_window};
use super::settings_controls::{
    a11y_click, a11y_find, a11y_node, break_file, focus, keys, ring, show,
    visual,
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
