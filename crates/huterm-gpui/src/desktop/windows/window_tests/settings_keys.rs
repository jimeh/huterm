//! The Settings window's keybinding editor through the desktop harness.

use huterm_protocol::CommandId;

use super::settings::{
    ScratchConfig, open_settings, settings, settled, with_view, with_window,
};
use super::*;

/// The default that closes a pane, which closes Settings unless the
/// recorder takes it.
const CLOSE: &str = if cfg!(target_os = "macos") {
    "cmd-w"
} else {
    "ctrl-shift-w"
};

fn row(harness: &mut DesktopHarness, command: CommandId, key: &str) -> String {
    with_view(harness, |view, _| view.key_row(command, key))
        .unwrap_or_else(|| panic!("a row for {} on {key}", command.as_str()))
}

fn press(harness: &mut DesktopHarness, key: &str) {
    with_window(harness, |view, window, cx| view.press(key, window, cx));
}

fn keystrokes(harness: &mut DesktopHarness, keys: &str) {
    let window: gpui::AnyWindowHandle =
        harness.cx.update(|cx| settings(cx).unwrap().into());
    for key in keys.split(' ') {
        harness
            .cx
            .dispatch_keystroke(window, gpui::Keystroke::parse(key).unwrap());
    }
}

fn saved(
    harness: &mut DesktopHarness,
    file: &ScratchConfig,
    check: impl Fn(&str) -> bool,
) {
    harness.wait_for("the keybinding save", |cx| {
        settled(cx) && check(&file.text())
    });
}

#[gpui::test]
fn switching_a_default_off_and_on_writes_and_removes_its_entry(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-switch", "# Mine.\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let next = row(&mut harness, ids::NEXT_TAB, "ctrl-tab");
    press(&mut harness, &format!("kon:{next}"));
    saved(&mut harness, &file, |text| text.contains("disabled = true"));
    assert_eq!(
        file.text(),
        "# Mine.\n\n[[keybinding]]\nkey = \"ctrl-tab\"\ncommand = \"next_tab\"\n\
         disabled = true\n"
    );
    press(&mut harness, &format!("kon:{next}"));
    saved(&mut harness, &file, |text| !text.contains("keybinding"));
    assert_eq!(file.text(), "# Mine.\n");
}

#[gpui::test]
fn the_recorder_takes_bound_keys_records_chords_and_cancels_on_escape(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-record", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let next = row(&mut harness, ids::NEXT_TAB, "ctrl-tab");
    // Keys match against the drawn frame, so the key cap must show.
    press(&mut harness, "kgo:next_tab");
    harness.cx.run_until_parked();
    press(&mut harness, &format!("krec:{next}"));
    harness.cx.run_until_parked();
    // Close Pane's key reaches the recorder, not the binding.
    keystrokes(&mut harness, CLOSE);
    harness.cx.run_until_parked();
    let open = harness.cx.update(|cx| {
        let handle = settings(cx).map(|handle| handle.window_id());
        cx.windows()
            .iter()
            .any(|window| Some(window.window_id()) == handle)
    });
    assert!(open, "Close Pane's key closed Settings");
    with_view(&mut harness, |view, _| {
        assert_eq!(view.recording(), Some(vec![CLOSE.to_owned()]));
    });
    // With no second stroke, the recording ends after the chord timeout.
    harness
        .cx
        .executor()
        .advance_clock(super::super::settings_window::CHORD_TIMEOUT);
    saved(&mut harness, &file, |text| text.contains(CLOSE));
    assert_eq!(
        file.text(),
        format!(
            "[[keybinding]]\nkey = \"ctrl-tab\"\ncommand = \"next_tab\"\n\
             disabled = true\n\n[[keybinding]]\nkey = \"{CLOSE}\"\n\
             command = \"next_tab\"\n"
        )
    );
    // A second stroke makes a chord.
    press(&mut harness, "kadd:new_window");
    keystrokes(&mut harness, "ctrl-k ctrl-t");
    saved(&mut harness, &file, |text| text.contains("ctrl-k ctrl-t"));
    assert!(file.text().ends_with(
        "[[keybinding]]\nkey = \"ctrl-k ctrl-t\"\ncommand = \"new_window\"\n"
    ));
    // Escape before any stroke cancels and writes nothing.
    let before = file.text();
    press(&mut harness, "kadd:new_tab");
    keystrokes(&mut harness, "escape");
    with_view(&mut harness, |view, _| assert_eq!(view.recording(), None));
    // So does Escape after a first stroke.
    press(&mut harness, "kadd:new_tab");
    keystrokes(&mut harness, "ctrl-k escape");
    with_view(&mut harness, |view, _| assert_eq!(view.recording(), None));
    harness
        .cx
        .executor()
        .advance_clock(super::super::settings_window::CHORD_TIMEOUT);
    harness.cx.run_until_parked();
    assert_eq!(file.text(), before);
}

#[gpui::test]
fn a_user_entry_changes_in_place_and_a_bad_condition_writes_nothing(
    cx: &mut TestAppContext,
) {
    let original = "# Resize faster.\n[[keybinding]]\nkey = \"ctrl-alt-l\"\n\
                    command = \"resize_pane_right\"\nargs = { amount = 5 }\n";
    let file = ScratchConfig::new("keys-user", original);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let resize = row(&mut harness, ids::RESIZE_PANE_RIGHT, "ctrl-alt-l");
    assert_eq!(resize, "u0");
    let when = format!("when:{resize}");
    with_window(&mut harness, |view, window, cx| {
        view.type_key(&when, "Terminal &&", window, cx);
        view.enter_key(&when, cx);
        assert!(view.key_error(&resize).is_some());
    });
    harness.cx.run_until_parked();
    assert_eq!(file.text(), original);
    with_window(&mut harness, |view, window, cx| {
        view.type_key(&when, "Terminal && split", window, cx);
        view.enter_key(&when, cx);
    });
    saved(&mut harness, &file, |text| text.contains("when"));
    let amount = format!("arg:{resize}:amount");
    with_window(&mut harness, |view, window, cx| {
        view.type_key(&amount, "500", window, cx);
        view.enter_key(&amount, cx);
        assert!(view.key_error(&resize).is_some());
        view.type_key(&amount, "", window, cx);
        view.enter_key(&amount, cx);
    });
    saved(&mut harness, &file, |text| !text.contains("amount"));
    assert_eq!(
        file.text(),
        "# Resize faster.\n[[keybinding]]\nkey = \"ctrl-alt-l\"\n\
         command = \"resize_pane_right\"\nwhen = \"Terminal && split\"\n"
    );
    press(&mut harness, &format!("kdel:{resize}"));
    saved(&mut harness, &file, |text| !text.contains("keybinding"));
    assert_eq!(file.text(), "");
}

#[gpui::test]
fn restoring_defaults_removes_the_commands_entries_only(
    cx: &mut TestAppContext,
) {
    let original = "[[keybinding]]\nkey = \"ctrl-tab\"\ncommand = \"next_tab\"\n\
                    disabled = true\n\n[[keybinding]]\nkey = \"alt-n\"\n\
                    command = \"next_tab\"\n\n[[keybinding]]\nkey = \"alt-m\"\n\
                    command = \"new_tab\"\n\n[[keybinding]]\nkey = \"alt-q\"\n\
                    command = \"unbind\"\n";
    let file = ScratchConfig::new("keys-restore", original);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    press(&mut harness, "kreset:next_tab");
    saved(&mut harness, &file, |text| !text.contains("next_tab"));
    assert_eq!(
        file.text(),
        "[[keybinding]]\nkey = \"alt-m\"\ncommand = \"new_tab\"\n\n\
         [[keybinding]]\nkey = \"alt-q\"\ncommand = \"unbind\"\n"
    );
}

#[gpui::test]
fn the_editor_writes_nothing_while_the_file_is_invalid(
    cx: &mut TestAppContext,
) {
    let broken = "[window\n";
    let file = ScratchConfig::new("keys-invalid", broken);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        assert!(view.read_only());
        assert_eq!(view.key_row(ids::NEXT_TAB, "ctrl-tab"), None);
        view.press("kreset:next_tab", window, cx);
        view.press("kadd:next_tab", window, cx);
        assert_eq!(view.recording(), None);
    });
    harness.cx.run_until_parked();
    assert_eq!(file.text(), broken);
}

/// Select Tab's first default, which carries an `index` argument.
const SELECT_FIRST: &str = if cfg!(target_os = "macos") {
    "cmd-1"
} else {
    "alt-1"
};

fn focus_name(harness: &mut DesktopHarness) -> String {
    with_window(harness, |view, window, cx| view.focus_name(window, cx))
}

#[gpui::test]
fn a_condition_completes_on_return_and_tab_never_writes_a_suggestion(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-complete", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let next = row(&mut harness, ids::NEXT_TAB, "ctrl-tab");
    let when = format!("when:{next}");
    press(&mut harness, "kgo:next_tab");
    harness.cx.run_until_parked();
    // Focusing the empty field offers nothing, and Tab moves on.
    with_window(&mut harness, |view, window, cx| {
        view.focus_key(&when, window, cx);
    });
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| assert!(!view.completion_open()));
    keystrokes(&mut harness, "tab");
    harness.cx.run_until_parked();
    assert_ne!(focus_name(&mut harness), format!("key:{when}"));
    assert_eq!(file.text(), "");
    // Nor does a field emptied while focused, so Return saves nothing.
    with_window(&mut harness, |view, window, cx| {
        view.focus_key(&when, window, cx);
    });
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.type_key(&when, "", window, cx);
    });
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| assert!(!view.completion_open()));
    keystrokes(&mut harness, "enter");
    harness.cx.run_until_parked();
    assert_eq!(file.text(), "");
    // A typed name completes on Return, and the next Return saves it.
    with_window(&mut harness, |view, window, cx| {
        view.focus_key(&when, window, cx);
    });
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.type_key(&when, "Te", window, cx);
    });
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| assert!(view.completion_open()));
    keystrokes(&mut harness, "enter");
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.key_text(&when, cx), "Terminal");
        assert!(!view.completion_open());
    });
    assert_eq!(file.text(), "");
    keystrokes(&mut harness, "enter");
    saved(&mut harness, &file, |text| {
        text.contains("when = \"Terminal\"")
    });
    // Focus follows the changed binding to its new row.
    let follow = row(&mut harness, ids::NEXT_TAB, "ctrl-tab");
    assert_eq!(follow, next, "the default keeps its row");
    harness.cx.run_until_parked();
    assert_eq!(focus_name(&mut harness), "key:when:u1");
}

#[gpui::test]
fn changing_a_default_argument_writes_one_replacement(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("keys-default-arg", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let first = row(&mut harness, ids::SELECT_TAB, SELECT_FIRST);
    let index = format!("arg:{first}:index");
    press(&mut harness, "kgo:select_tab");
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.focus_key(&index, window, cx);
        view.type_key(&index, "4", window, cx);
        view.enter_key(&index, cx);
        // Leaving the field before the save lands writes nothing more.
        view.blur_key(&index, cx);
    });
    saved(&mut harness, &file, |text| text.contains("index = 4"));
    harness.cx.run_until_parked();
    let text = file.text();
    assert_eq!(text.matches("index = 4").count(), 1, "{text}");
    assert_eq!(text.matches("disabled = true").count(), 1, "{text}");
    assert_eq!(focus_name(&mut harness), "key:arg:u1:index");
    // The switched-off default shows the file's value again.
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.key_text(&index, cx), "1");
    });
}

#[gpui::test]
fn restoring_defaults_moves_focus_to_the_command_and_keys_keep_working(
    cx: &mut TestAppContext,
) {
    let original = "[[keybinding]]\nkey = \"ctrl-tab\"\ncommand = \"next_tab\"\ndisabled = true\n";
    let file = ScratchConfig::new("keys-restore-focus", original);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    press(&mut harness, "kgo:next_tab");
    harness.cx.run_until_parked();
    press(&mut harness, "kreset:next_tab");
    saved(&mut harness, &file, str::is_empty);
    harness.cx.run_until_parked();
    assert_eq!(focus_name(&mut harness), "action:kcmd:next_tab");
    let find = if cfg!(target_os = "macos") {
        "cmd-f"
    } else {
        "ctrl-f"
    };
    keystrokes(&mut harness, find);
    harness.cx.run_until_parked();
    assert_eq!(focus_name(&mut harness), "key:search");
}
