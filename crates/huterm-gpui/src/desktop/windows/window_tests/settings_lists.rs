//! The Settings condition and font family fields on `huterm-ui`'s
//! `Combobox`, driven by real keyboard and pointer events: migration 5a and
//! D6 of `docs/plans/ui-components.md`.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Bounds, Entity, Modifiers, MouseButton, Pixels, Point, point, px};
use huterm_config::settings::SectionId;
use huterm_ui::components::combobox::{Combobox, ComboboxEvent};

use super::settings::{
    ScratchConfig, open_settings, settings, settled, with_view, with_window,
};
use super::settings_controls::{
    a11y_click, a11y_find, a11y_node, break_file, focus, keys, visual,
};
use super::*;
use crate::desktop::windows::UnavailableFonts;

/// The families the font field suggests in place of the installed ones,
/// sorted as the platform's list is.
const FAMILIES: &[&str] = &["Fira Code", "Fira Mono", "Menlo", "Monaco"];

const FAMILY: &str = "font.family";

/// The font family field's chevron.
const CHEVRON: &str = "settings-fonts-font.family";

/// The platform's select-all key, as the default keymap binds it in fields.
const SELECT_ALL: &str = if cfg!(target_os = "macos") {
    "cmd-a"
} else {
    "ctrl-a"
};

/// Select Tab's first two defaults, rows one above the other.
const SELECT_TABS: [&str; 2] = if cfg!(target_os = "macos") {
    ["cmd-1", "cmd-2"]
} else {
    ["alt-1", "alt-2"]
};

/// A suggestion with a description: a context's row height.
const CONTEXT_ROW: f32 = 40.0;
/// A suggestion of one line: a family's row height.
const FAMILY_ROW: f32 = 28.0;

/// Types `text` into the focused field, as the keyboard does.
fn typed(harness: &DesktopHarness, text: &str) {
    let mut visual = visual(harness);
    visual.simulate_input(text);
    visual.run_until_parked();
}

/// What `list` shows: whether it is open, its labels, and its highlight.
fn shown(
    harness: &mut DesktopHarness,
    list: &Entity<Combobox>,
) -> (bool, Vec<String>, Option<String>) {
    harness.cx.update(|cx| {
        let list = list.read(cx);
        (
            list.is_open(),
            list.options()
                .iter()
                .map(|option| option.label.to_string())
                .collect(),
            list.highlighted().map(|option| option.label.to_string()),
        )
    })
}

fn list_bounds(
    harness: &mut DesktopHarness,
    list: &Entity<Combobox>,
) -> Bounds<Pixels> {
    harness
        .cx
        .update(|cx| list.read(cx).list_bounds())
        .expect("the list painted")
}

/// The middle of `list`'s row `index`, under its border and padding, for
/// rows `height` tall.
fn row_center(
    harness: &mut DesktopHarness,
    list: &Entity<Combobox>,
    index: usize,
    height: f32,
) -> Point<Pixels> {
    let bounds = list_bounds(harness, list);
    #[expect(clippy::cast_precision_loss, reason = "a small row index")]
    let top = 1.0 + 4.0 + height * index as f32 + height / 2.0;
    point(bounds.left() + px(20.0), bounds.top() + px(top))
}

/// Counts the blurs of the field focus is on from now on. GPUI reports
/// blurs only in an active window, so this activates it first.
fn count_blurs(harness: &mut DesktopHarness) -> Rc<Cell<usize>> {
    with_window(harness, |_, window, _| window.activate_window());
    let blurs = Rc::new(Cell::new(0));
    let counted = blurs.clone();
    with_window(harness, |_, window, cx| {
        let focused = window.focused(cx).expect("a focused field");
        cx.on_blur(&focused, window, move |_, _, _| {
            counted.set(counted.get() + 1);
        })
        .detach();
    });
    blurs
}

/// The accessibility state of the element named `id`.
fn aria(harness: &mut DesktopHarness, id: &str) -> serde_json::Value {
    a11y_node(harness, id)["aria"].clone()
}

/// Opens Keybindings on `command`'s row for `key` and focuses its `when`
/// field; returns the field's key.
fn condition(
    harness: &mut DesktopHarness,
    file: &ScratchConfig,
    command: huterm_protocol::CommandId,
    key: &str,
) -> String {
    open_settings(harness, &file.path());
    let row = with_view(harness, |view, _| view.key_row(command, key))
        .unwrap_or_else(|| panic!("a row for {key}"));
    let when = format!("when:{row}");
    with_window(harness, |view, window, cx| {
        view.press(&format!("kgo:{}", command.as_str()), window, cx);
    });
    harness.cx.run_until_parked();
    with_window(harness, |view, window, cx| {
        view.focus_key(&when, window, cx);
    });
    visual(harness).run_until_parked();
    when
}

fn key_text(harness: &mut DesktopHarness, key: &str) -> String {
    with_view(harness, |view, cx| view.key_text(key, cx))
}

#[gpui::test]
fn typing_keeps_condition_suggestions_open_and_return_completes_the_first(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("lists-condition-type", "");
    let mut harness = DesktopHarness::new(cx);
    let when = condition(&mut harness, &file, ids::NEXT_TAB, "ctrl-tab");
    let list = with_view(&mut harness, |view, _| view.key_list(&when));
    typed(&harness, "S");
    let (open, labels, highlight) = shown(&mut harness, &list);
    assert!(open, "typing a name opened no suggestions");
    assert_eq!(
        labels,
        [
            "selection",
            "split",
            "Settings",
            "SettingsControl",
            "SettingsGroup"
        ]
    );
    assert_eq!(highlight.as_deref(), Some("selection"));
    // Typing continues in the field while the suggestions show, and the
    // first match stays highlighted.
    typed(&harness, "ettingsG");
    assert_eq!(focus(&mut harness), format!("key:{when}"));
    assert_eq!(key_text(&mut harness, &when), "SettingsG");
    let (open, labels, highlight) = shown(&mut harness, &list);
    assert!(open);
    assert_eq!(labels, ["SettingsGroup"]);
    assert_eq!(highlight.as_deref(), Some("SettingsGroup"));
    let field = aria(&mut harness, &format!("keybinding-field-{when}"));
    assert_eq!(field["role"], "EditableComboBox", "{field}");
    assert_eq!(field["expanded"], true, "{field}");
    assert_eq!(field["description"], "SettingsGroup", "{field}");
    // Return completes the name before the caret and closes the list; it
    // saves nothing yet.
    keys(&harness, "enter");
    assert_eq!(key_text(&mut harness, &when), "SettingsGroup");
    assert!(!shown(&mut harness, &list).0);
    let field = aria(&mut harness, &format!("keybinding-field-{when}"));
    assert_eq!(field["expanded"], false, "{field}");
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), "");
}

#[gpui::test]
fn the_arrows_and_the_pointer_move_one_condition_highlight(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("lists-condition-arrows", "");
    let mut harness = DesktopHarness::new(cx);
    let when = condition(&mut harness, &file, ids::NEXT_TAB, "ctrl-tab");
    let list = with_view(&mut harness, |view, _| view.key_list(&when));
    typed(&harness, "S");
    keys(&harness, "down");
    assert_eq!(shown(&mut harness, &list).2.as_deref(), Some("split"));
    // Previous wraps from the first match to the last.
    keys(&harness, "up up");
    assert_eq!(
        shown(&mut harness, &list).2.as_deref(),
        Some("SettingsGroup")
    );
    keys(&harness, "pageup");
    assert_eq!(shown(&mut harness, &list).2.as_deref(), Some("selection"));
    let field = aria(&mut harness, &format!("keybinding-field-{when}"));
    assert_eq!(field["description"], "selection", "{field}");
    // A moving pointer moves the same highlight.
    let row = row_center(&mut harness, &list, 2, CONTEXT_ROW);
    let mut pointer = visual(&harness);
    pointer.simulate_mouse_move(row, None, Modifiers::none());
    pointer.run_until_parked();
    assert_eq!(shown(&mut harness, &list).2.as_deref(), Some("Settings"));
    let field = aria(&mut harness, &format!("keybinding-field-{when}"));
    assert_eq!(field["description"], "Settings", "{field}");
    // The text never changed, and nothing was written.
    assert_eq!(key_text(&mut harness, &when), "S");
    assert_eq!(focus(&mut harness), format!("key:{when}"));
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), "");
}

#[gpui::test]
fn escape_and_tab_close_condition_suggestions_before_the_field_acts(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("lists-condition-escape", "");
    let mut harness = DesktopHarness::new(cx);
    let when = condition(&mut harness, &file, ids::NEXT_TAB, "ctrl-tab");
    let list = with_view(&mut harness, |view, _| view.key_list(&when));
    typed(&harness, "Te");
    assert!(shown(&mut harness, &list).0);
    // The first Escape closes the list and keeps the text and focus.
    keys(&harness, "escape");
    assert!(!shown(&mut harness, &list).0);
    assert_eq!(key_text(&mut harness, &when), "Te");
    assert_eq!(focus(&mut harness), format!("key:{when}"));
    // Down opens it again, and Tab closes it the same way.
    keys(&harness, "down");
    assert_eq!(shown(&mut harness, &list).2.as_deref(), Some("Terminal"));
    keys(&harness, "tab");
    assert!(!shown(&mut harness, &list).0);
    assert_eq!(key_text(&mut harness, &when), "Te");
    assert_eq!(focus(&mut harness), format!("key:{when}"));
    // With the list closed, Escape puts back the saved text and stays.
    keys(&harness, "escape");
    assert_eq!(key_text(&mut harness, &when), "");
    assert_eq!(focus(&mut harness), format!("key:{when}"));
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), "");
}

#[gpui::test]
fn pressing_a_condition_suggestion_completes_it_without_a_blur(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("lists-condition-press", "");
    let mut harness = DesktopHarness::new(cx);
    let when = condition(&mut harness, &file, ids::NEXT_TAB, "ctrl-tab");
    let list = with_view(&mut harness, |view, _| view.key_list(&when));
    let blurs = count_blurs(&mut harness);
    typed(&harness, "S");
    let row = row_center(&mut harness, &list, 1, CONTEXT_ROW);
    visual(&harness).simulate_click(row, Modifiers::none());
    harness.cx.run_until_parked();
    // A blur would have saved the typed "S" first.
    assert_eq!(blurs.get(), 0, "the press blurred the field");
    assert_eq!(key_text(&mut harness, &when), "split");
    assert!(!shown(&mut harness, &list).0);
    assert_eq!(focus(&mut harness), format!("key:{when}"));
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), "");
}

#[gpui::test]
fn a_condition_suggestion_never_lands_where_the_caret_moved(
    cx: &mut TestAppContext,
) {
    // The highlight outlived a caret move, and Return inserted the name at
    // the new caret: "TerminalTe".
    let file = ScratchConfig::new("lists-condition-caret", "");
    let mut harness = DesktopHarness::new(cx);
    let when = condition(&mut harness, &file, ids::NEXT_TAB, "ctrl-tab");
    let list = with_view(&mut harness, |view, _| view.key_list(&when));
    typed(&harness, "Te");
    assert_eq!(shown(&mut harness, &list).2.as_deref(), Some("Terminal"));
    // The suggestions follow the name before the caret.
    keys(&harness, "left");
    let (open, _, highlight) = shown(&mut harness, &list);
    assert!(open, "\"T\" before the caret closed the list");
    assert_eq!(highlight.as_deref(), Some("Terminal"));
    keys(&harness, "left");
    assert!(!shown(&mut harness, &list).0, "no name before the caret");
    // A pick that no longer completes the name before the caret, as from
    // a list that has not caught up, changes nothing.
    harness.cx.update(|cx| {
        list.update(cx, |_, cx| {
            cx.emit(ComboboxEvent::Picked("Terminal".into()));
        });
    });
    harness.cx.run_until_parked();
    assert_eq!(key_text(&mut harness, &when), "Te");
    // With the list closed, Return saves the typed text as it stands.
    keys(&harness, "enter");
    assert!(harness.cx.update(settled));
    let text = file.text();
    assert!(text.ends_with("when = \"Te\"\n"), "{text}");
}

#[gpui::test]
fn a_completion_in_the_middle_of_a_name_replaces_all_of_it(
    cx: &mut TestAppContext,
) {
    // "Termin|a" completed to "Terminala": the name's tail stayed.
    let file = ScratchConfig::new("lists-condition-mid-name", "");
    let mut harness = DesktopHarness::new(cx);
    let when = condition(&mut harness, &file, ids::NEXT_TAB, "ctrl-tab");
    let list = with_view(&mut harness, |view, _| view.key_list(&when));
    typed(&harness, "Termina");
    keys(&harness, "left");
    assert_eq!(shown(&mut harness, &list).2.as_deref(), Some("Terminal"));
    keys(&harness, "enter");
    assert_eq!(key_text(&mut harness, &when), "Terminal");
}

#[gpui::test]
fn a_completion_typed_in_front_of_a_name_keeps_that_name(
    cx: &mut TestAppContext,
) {
    // Replacing the whole word deleted the name typed in front of:
    // "sel|split" became "selection".
    let file = ScratchConfig::new("lists-condition-in-front", "");
    let mut harness = DesktopHarness::new(cx);
    let when = condition(&mut harness, &file, ids::NEXT_TAB, "ctrl-tab");
    let list = with_view(&mut harness, |view, _| view.key_list(&when));
    typed(&harness, "split");
    keys(&harness, "home");
    typed(&harness, "sel");
    assert_eq!(shown(&mut harness, &list).2.as_deref(), Some("selection"));
    keys(&harness, "enter");
    assert_eq!(key_text(&mut harness, &when), "selectionsplit");
}

#[gpui::test]
fn a_sibling_picked_inside_a_known_name_replaces_it(cx: &mut TestAppContext) {
    // Keeping the tail glued the two names: "SettingsGroupControl".
    let file = ScratchConfig::new("lists-condition-sibling", "");
    let mut harness = DesktopHarness::new(cx);
    let when = condition(&mut harness, &file, ids::NEXT_TAB, "ctrl-tab");
    let list = with_view(&mut harness, |view, _| view.key_list(&when));
    typed(&harness, "SettingsControl");
    for _ in 0.."Control".len() {
        keys(&harness, "left");
    }
    keys(&harness, "down");
    let (open, labels, _) = shown(&mut harness, &list);
    assert!(open, "Down opened no suggestions for \"Settings\"");
    assert!(
        labels.iter().any(|label| label == "SettingsGroup"),
        "{labels:?}"
    );
    while shown(&mut harness, &list).2.as_deref() != Some("SettingsGroup") {
        keys(&harness, "down");
    }
    keys(&harness, "enter");
    assert_eq!(key_text(&mut harness, &when), "SettingsGroup");
}

#[gpui::test]
fn condition_suggestions_float_over_the_rows_below(cx: &mut TestAppContext) {
    // D6: the list laid out in the row's flow and pushed the rows below
    // it down.
    let file = ScratchConfig::new("lists-condition-float", "");
    let mut harness = DesktopHarness::new(cx);
    let when = condition(&mut harness, &file, ids::SELECT_TAB, SELECT_TABS[0]);
    let next = with_view(&mut harness, |view, _| {
        view.key_row(ids::SELECT_TAB, SELECT_TABS[1])
    })
    .expect("Select Tab's second row");
    let next = format!("when:{next}");
    let bounds = |harness: &mut DesktopHarness, key: &str| {
        with_view(harness, |view, cx| view.key_field_bounds(key, cx))
            .unwrap_or_else(|| panic!("{key} painted"))
    };
    let field = bounds(&mut harness, &when);
    let below = bounds(&mut harness, &next);
    let list = with_view(&mut harness, |view, _| view.key_list(&when));
    typed(&harness, "S");
    visual(&harness).run_until_parked();
    let suggestions = list_bounds(&mut harness, &list);
    assert!(
        suggestions.top() >= field.bottom()
            || suggestions.bottom() <= field.top(),
        "the list {suggestions:?} covers its field {field:?}"
    );
    assert_eq!(bounds(&mut harness, &when), field);
    assert_eq!(
        bounds(&mut harness, &next),
        below,
        "the list pushed the next row"
    );
}

/// Opens Settings on Appearance, with [`FAMILIES`] as the installed
/// families, and focuses the font family field.
fn font_field(harness: &mut DesktopHarness, file: &ScratchConfig) {
    open_settings(harness, &file.path());
    with_view(harness, |view, cx| view.set_families(FAMILIES, cx));
    with_window(harness, |view, window, cx| {
        view.show_section(SectionId::Appearance, cx);
        window.refresh();
    });
    visual(harness).run_until_parked();
    with_window(harness, |view, window, cx| {
        view.focus_field(FAMILY, window, cx);
    });
    visual(harness).run_until_parked();
}

fn font_list(harness: &mut DesktopHarness) -> Entity<Combobox> {
    with_view(harness, |view, _| view.font_list_at(FAMILY))
}

fn field_text(harness: &mut DesktopHarness) -> String {
    with_window(harness, |view, _, cx| view.field_value(FAMILY, cx))
}

/// Replaces the focused field's text, as selecting it all and typing does.
fn type_over(harness: &DesktopHarness, text: &str) {
    keys(harness, SELECT_ALL);
    typed(harness, text);
}

/// Presses the font family field's chevron, as a pointer does.
fn press_chevron(harness: &mut DesktopHarness) {
    let chevron = a11y_node(harness, CHEVRON);
    a11y_click(harness, &chevron);
}

fn saved(harness: &mut DesktopHarness, file: &ScratchConfig, family: &str) {
    let expected = format!("[font]\nfamily = \"{family}\"\n");
    harness.wait_for("the family save", |cx| {
        settled(cx) && file.text() == expected
    });
}

#[gpui::test]
fn a_font_name_that_is_not_installed_saves_on_enter_and_on_blur(
    cx: &mut TestAppContext,
) {
    // A set family, as the default one differs by platform.
    let file = ScratchConfig::new(
        "lists-font-missing",
        "[font]\nfamily = \"Menlo\"\n",
    );
    let mut harness = DesktopHarness::new(cx);
    font_field(&mut harness, &file);
    let list = font_list(&mut harness);
    assert_eq!(field_text(&mut harness), "Menlo");
    type_over(&harness, "Huterm Missing");
    assert!(!shown(&mut harness, &list).0, "a missing name matched");
    keys(&harness, "enter");
    saved(&mut harness, &file, "Huterm Missing");
    // Leaving the field saves what it holds too.
    with_window(&mut harness, |_, window, _| window.activate_window());
    type_over(&harness, "Other Missing");
    with_window(&mut harness, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx);
    });
    harness.cx.run_until_parked();
    assert_ne!(focus(&mut harness), FAMILY);
    saved(&mut harness, &file, "Other Missing");
}

#[gpui::test]
fn escape_closes_font_suggestions_then_puts_back_the_value(
    cx: &mut TestAppContext,
) {
    let original = "[font]\nfamily = \"Menlo\"\n";
    let file = ScratchConfig::new("lists-font-escape", original);
    let mut harness = DesktopHarness::new(cx);
    font_field(&mut harness, &file);
    let list = font_list(&mut harness);
    type_over(&harness, "mo");
    let (open, labels, highlight) = shown(&mut harness, &list);
    assert!(open);
    assert_eq!(labels, ["Fira Mono", "Monaco"]);
    assert_eq!(highlight, None, "nothing is highlighted until a move");
    keys(&harness, "escape");
    assert!(!shown(&mut harness, &list).0);
    assert_eq!(field_text(&mut harness), "mo");
    assert_eq!(focus(&mut harness), FAMILY);
    // The second Escape puts back the value and leaves the field, as
    // Escape in any Settings field does.
    keys(&harness, "escape");
    assert_eq!(field_text(&mut harness), "Menlo");
    assert_ne!(focus(&mut harness), FAMILY);
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), original);
}

#[gpui::test]
fn enter_saves_the_typed_font_unless_a_suggestion_is_highlighted(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("lists-font-enter", "");
    let mut harness = DesktopHarness::new(cx);
    font_field(&mut harness, &file);
    let list = font_list(&mut harness);
    type_over(&harness, "Mona");
    assert_eq!(
        shown(&mut harness, &list),
        (true, vec!["Monaco".into()], None)
    );
    keys(&harness, "enter");
    saved(&mut harness, &file, "Mona");
    assert!(!shown(&mut harness, &list).0);
    // Moving to a suggestion makes Enter pick it.
    type_over(&harness, "Mona");
    keys(&harness, "down");
    assert_eq!(shown(&mut harness, &list).2.as_deref(), Some("Monaco"));
    keys(&harness, "enter");
    saved(&mut harness, &file, "Monaco");
    assert_eq!(field_text(&mut harness), "Monaco");
    assert_eq!(focus(&mut harness), FAMILY);
}

#[gpui::test]
fn picking_a_font_suggestion_writes_it_with_no_blur_first(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("lists-font-pick", "");
    let mut harness = DesktopHarness::new(cx);
    font_field(&mut harness, &file);
    let list = font_list(&mut harness);
    let blurs = count_blurs(&mut harness);
    type_over(&harness, "Fira");
    assert_eq!(shown(&mut harness, &list).1, ["Fira Code", "Fira Mono"]);
    let row = row_center(&mut harness, &list, 1, FAMILY_ROW);
    visual(&harness).simulate_click(row, Modifiers::none());
    harness.cx.run_until_parked();
    // A blur would have saved the typed "Fira" before the pick.
    assert_eq!(blurs.get(), 0, "the press blurred the field");
    saved(&mut harness, &file, "Fira Mono");
    assert_eq!(field_text(&mut harness), "Fira Mono");
    assert_eq!(focus(&mut harness), FAMILY);
    assert!(!shown(&mut harness, &list).0);
}

#[gpui::test]
fn the_font_field_is_one_combobox_without_an_installed_select(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("lists-font-one", "");
    let mut harness = DesktopHarness::new(cx);
    font_field(&mut harness, &file);
    let labelled = |harness: &mut DesktopHarness, role: &str| {
        a11y_find(harness, |node| {
            node["aria"]["label"] == "Installed fonts"
                && node["aria"]["role"] == role
        })
    };
    assert!(
        labelled(&mut harness, "ComboBox").is_none(),
        "the Installed select still shows"
    );
    let id = format!("settings-field-{FAMILY}");
    let field = aria(&mut harness, &id);
    assert_eq!(field["role"], "EditableComboBox", "{field}");
    assert_eq!(field["expanded"], false, "{field}");
    // An emptied field lists every family, and Down moves to the first.
    keys(&harness, &format!("{SELECT_ALL} backspace down"));
    let field = aria(&mut harness, &id);
    assert_eq!(field["expanded"], true, "{field}");
    assert_eq!(field["description"], "Fira Code", "{field}");
    assert!(
        labelled(&mut harness, "ListBox").is_some(),
        "the suggestions' list"
    );
    let list = font_list(&mut harness);
    assert_eq!(shown(&mut harness, &list).1, FAMILIES);
}

#[gpui::test]
fn a_read_only_font_field_shows_no_suggestions(cx: &mut TestAppContext) {
    let original = "[font]\nfamily = \"Menlo\"\n";
    let file = ScratchConfig::new("lists-font-read-only", original);
    let mut harness = DesktopHarness::new(cx);
    font_field(&mut harness, &file);
    let list = font_list(&mut harness);
    type_over(&harness, "mo");
    assert!(shown(&mut harness, &list).0);
    // The file breaks while the list shows; the field turns read-only.
    let broken = "[font]\nfamily = \"Menlo\"\n[window\n";
    break_file(&mut harness, &file, broken);
    assert!(!shown(&mut harness, &list).0, "the list stayed open");
    // Focus stays, so a key reaching the window root cannot pass for a
    // refusal.
    assert_eq!(focus(&mut harness), FAMILY);
    keys(&harness, "down");
    typed(&harness, "n");
    assert!(!shown(&mut harness, &list).0, "a read-only field opened it");
    let field = aria(&mut harness, &format!("settings-field-{FAMILY}"));
    assert_eq!(field["expanded"], false, "{field}");
    // The chevron is disabled with the field's reason, and opens nothing.
    let chevron = aria(&mut harness, CHEVRON);
    assert_eq!(chevron["disabled"], true, "{chevron}");
    assert_eq!(
        chevron["description"], "Read-only until config.toml is fixed",
        "{chevron}"
    );
    press_chevron(&mut harness);
    assert!(!shown(&mut harness, &list).0, "the chevron opened it");
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), broken);
}

#[gpui::test]
fn down_lists_every_font_on_the_current_one_before_an_edit(
    cx: &mut TestAppContext,
) {
    let file =
        ScratchConfig::new("lists-font-browse", "[font]\nfamily = \"Menlo\"\n");
    let mut harness = DesktopHarness::new(cx);
    font_field(&mut harness, &file);
    let list = font_list(&mut harness);
    let every = || FAMILIES.iter().map(|family| (*family).to_owned()).collect();
    keys(&harness, "down");
    assert_eq!(
        shown(&mut harness, &list),
        (true, every(), Some("Menlo".into()))
    );
    keys(&harness, "escape up");
    assert_eq!(
        shown(&mut harness, &list),
        (true, every(), Some("Menlo".into()))
    );
    assert_eq!(field_text(&mut harness), "Menlo");
}

#[gpui::test]
fn typing_filters_the_fonts_until_a_pick_shows_a_value_again(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("lists-font-filter", "");
    let mut harness = DesktopHarness::new(cx);
    font_field(&mut harness, &file);
    let list = font_list(&mut harness);
    let fira = vec!["Fira Code".to_owned(), "Fira Mono".to_owned()];
    type_over(&harness, "Fira");
    assert_eq!(shown(&mut harness, &list), (true, fira.clone(), None));
    // Reopening keeps the typed filter.
    keys(&harness, "escape down");
    assert_eq!(
        shown(&mut harness, &list),
        (true, fira, Some("Fira Code".into()))
    );
    keys(&harness, "down enter");
    saved(&mut harness, &file, "Fira Mono");
    // The picked family is a value, so Down lists every family again.
    keys(&harness, "down");
    let (open, labels, highlight) = shown(&mut harness, &list);
    assert!(open);
    assert_eq!(labels, FAMILIES);
    assert_eq!(highlight.as_deref(), Some("Fira Mono"));
}

#[gpui::test]
fn the_font_chevron_toggles_every_font_and_keeps_focus_in_the_field(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new(
        "lists-font-chevron",
        "[font]\nfamily = \"Menlo\"\n",
    );
    let mut harness = DesktopHarness::new(cx);
    font_field(&mut harness, &file);
    let list = font_list(&mut harness);
    let blurs = count_blurs(&mut harness);
    let every: Vec<String> =
        FAMILIES.iter().map(|family| (*family).to_owned()).collect();
    press_chevron(&mut harness);
    assert_eq!(
        shown(&mut harness, &list),
        (true, every.clone(), Some("Menlo".into()))
    );
    assert_eq!(aria(&mut harness, CHEVRON)["expanded"], true);
    assert_eq!(blurs.get(), 0, "the chevron blurred the field");
    assert_eq!(focus(&mut harness), FAMILY);
    press_chevron(&mut harness);
    assert!(!shown(&mut harness, &list).0, "a second press left it open");
    // The chevron lists every family whatever the user typed.
    type_over(&harness, "Fira");
    keys(&harness, "escape");
    press_chevron(&mut harness);
    assert_eq!(shown(&mut harness, &list), (true, every.clone(), None));
    press_chevron(&mut harness);
    assert_eq!(blurs.get(), 0, "the chevron blurred the field");
    assert_eq!(focus(&mut harness), FAMILY);
    // Escape puts back the value and leaves the field; from elsewhere, the
    // chevron focuses the field again.
    keys(&harness, "escape");
    assert_ne!(focus(&mut harness), FAMILY);
    press_chevron(&mut harness);
    assert_eq!(focus(&mut harness), FAMILY);
    assert_eq!(
        shown(&mut harness, &list),
        (true, every, Some("Menlo".into()))
    );
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), "[font]\nfamily = \"Menlo\"\n");
}

#[gpui::test]
fn the_font_chevron_is_not_a_tab_stop(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("lists-font-tab", "");
    let mut harness = DesktopHarness::new(cx);
    font_field(&mut harness, &file);
    // Tab passes the chevron for the next setting, and back.
    keys(&harness, "tab");
    assert_eq!(focus(&mut harness), "font.size");
    keys(&harness, "shift-tab");
    assert_eq!(focus(&mut harness), FAMILY);
}

/// A `when` field's draft waits while its context menu is open, and saves
/// once focus leaves the menu for somewhere else, as leaving the field does.
#[gpui::test]
fn a_condition_draft_saves_when_focus_leaves_its_context_menu(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("lists-condition-menu", "");
    let mut harness = DesktopHarness::new(cx);
    let when = condition(&mut harness, &file, ids::NEXT_TAB, "ctrl-tab");
    // GPUI reports blurs only in an active window.
    with_window(&mut harness, |_, window, _| window.activate_window());
    typed(&harness, "Terminal");
    let (_, bounds) =
        with_window(&mut harness, |view, _, cx| view.key_caret(&when, cx))
            .expect("the field painted");
    let mut visual = visual(&harness);
    let at = point(bounds.left() + px(2.0), bounds.center().y);
    visual.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    visual.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    visual.run_until_parked();
    assert!(with_view(&mut harness, |view, _| view.field_menu_shown()));
    assert!(
        harness.cx.update(settled) && file.text().is_empty(),
        "opening the menu saved"
    );
    with_window(&mut harness, |view, window, cx| {
        view.focus_handle(cx).focus(window, cx);
    });
    harness.cx.run_until_parked();
    harness.wait_for("the condition to save", |cx| {
        settled(cx) && file.text().contains("when = \"Terminal\"")
    });
}

#[gpui::test]
fn a_family_that_is_not_installed_says_what_draws_instead(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new(
        "lists-font-fallback",
        "[font]\nfamily = \"Monaco\"\nui_family = \"Missing Sans\"\n",
    );
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.font.family = "Monaco".into();
        config.font.ui_family = Some("Missing Sans".into());
    });
    open_settings(&mut harness, &file.path());
    // As Huterm last applied them on a system with `FAMILIES`, which a
    // test platform does not list.
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().unavailable_fonts = UnavailableFonts {
            terminal: None,
            interface: Some("Missing Sans".into()),
        };
    });
    with_view(&mut harness, |view, cx| view.set_families(FAMILIES, cx));
    with_window(&mut harness, |view, window, cx| {
        view.show_section(SectionId::Appearance, cx);
        window.refresh();
    });
    visual(&harness).run_until_parked();
    let description = |harness: &mut DesktopHarness, path: &str| {
        a11y_node(harness, &format!("settings-row-{path}"))["aria"]
            ["description"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    // The interface family saved but draws in the system font, and the
    // row says so; an installed terminal family needs no note.
    assert!(
        description(&mut harness, "font.ui_family").ends_with(
            "Missing Sans is not installed, so the interface uses the system font."
        ),
        "{}",
        description(&mut harness, "font.ui_family")
    );
    assert!(
        !description(&mut harness, FAMILY).contains("not installed"),
        "{}",
        description(&mut harness, FAMILY)
    );
}

#[gpui::test]
fn a_family_installed_while_settings_is_open_reapplies_the_fonts(
    cx: &mut TestAppContext,
) {
    const INSTALLED: &[&str] = &["Menlo", "Missing Sans"];
    let file = ScratchConfig::new(
        "lists-font-installed",
        "[font]\nui_family = \"Missing Sans\"\n",
    );
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.font.ui_family = Some("Missing Sans".into());
    });
    open_settings(&mut harness, &file.path());
    let unavailable = UnavailableFonts {
        terminal: None,
        interface: Some("Missing Sans".into()),
    };
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().unavailable_fonts = unavailable.clone();
    });
    let reads = |harness: &mut DesktopHarness| {
        with_view(harness, |view, _| view.reads_started())
    };
    // A scan that agrees with what Huterm applied reloads nothing.
    let before = reads(&mut harness);
    with_view(&mut harness, |view, cx| view.set_families(&["Menlo"], cx));
    harness.cx.run_until_parked();
    assert_eq!(reads(&mut harness), before, "an agreeing scan reloaded");
    // The family installs while Settings is open, and the window comes
    // forward: its scan finds it and reloads, so the fonts apply.
    with_view(&mut harness, |view, cx| view.set_families(INSTALLED, cx));
    harness.wait_for("the reload", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.status_text() == "Reloaded")
                    .unwrap_or(false)
            })
    });
    // A reload that still cannot apply it, as here where the test platform
    // lists no fonts, is not tried again for the same families.
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().unavailable_fonts = unavailable;
    });
    harness.cx.run_until_parked();
    let before = reads(&mut harness);
    with_view(&mut harness, |view, cx| view.set_families(INSTALLED, cx));
    harness.cx.run_until_parked();
    assert_eq!(reads(&mut harness), before, "the same scan reloaded again");
    // Once a reload applies it, a scan agrees, and a family installed
    // later, here for a new interface family, reloads again.
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().unavailable_fonts =
            UnavailableFonts::default();
    });
    with_view(&mut harness, |view, cx| view.set_families(INSTALLED, cx));
    harness.cx.run_until_parked();
    assert_eq!(reads(&mut harness), before, "an agreeing scan reloaded");
    harness.cx.update(|cx| {
        let desktop = cx.global_mut::<Desktop>();
        desktop.config.font.ui_family = Some("Other Sans".into());
        desktop.unavailable_fonts = UnavailableFonts {
            terminal: None,
            interface: Some("Other Sans".into()),
        };
    });
    with_view(&mut harness, |view, cx| {
        view.set_families(&["Menlo", "Missing Sans", "Other Sans"], cx);
    });
    harness.cx.run_until_parked();
    assert!(
        reads(&mut harness) > before,
        "a later installation did not reload"
    );
}

#[gpui::test]
fn a_reload_notes_which_configured_fonts_fell_back(cx: &mut TestAppContext) {
    // The test platform lists no fonts, so both families fall back.
    let file = ScratchConfig::new(
        "lists-font-noted",
        "[font]\nfamily = \"Nope Mono\"\nui_family = \"Nope Sans\"\n",
    );
    let mut harness = DesktopHarness::new(cx);
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().config_path = file.path();
        let reload = CommandInvocation::new(ids::RELOAD_CONFIG, Vec::new());
        Desktop::invoke(cx, &reload, None).unwrap();
    });
    let noted = UnavailableFonts {
        terminal: Some("Nope Mono".into()),
        interface: Some("Nope Sans".into()),
    };
    harness.wait_for("the reload", |cx| {
        cx.global::<Desktop>().unavailable_fonts == noted
    });
}
