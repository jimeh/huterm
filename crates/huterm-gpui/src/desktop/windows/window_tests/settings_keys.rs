//! The Settings window's keybinding editor through the desktop harness.

use huterm_protocol::CommandId;

use super::super::settings_window::SettingsView;
use super::settings::{
    ScratchConfig, open_settings, settings, settled, with_view, with_window,
};
use super::settings_controls::{a11y_node, visual};
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
        harness.cx.dispatch_keystroke(
            window,
            gpui::Keystroke::parse_with_os(
                key,
                crate::keymap::Platform::current().os(),
            )
            .unwrap(),
        );
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
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.recording(cx), Some(vec![CLOSE.to_owned()]));
    });
    // With no second stroke, the recording ends after the chord timeout.
    // Close Pane holds the key, so Settings asks before taking it.
    harness
        .cx
        .executor()
        .advance_clock(super::super::settings_window::CHORD_TIMEOUT);
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert_eq!(view.dialog_name(), "reassign-key");
    });
    keystrokes(&mut harness, "tab enter");
    saved(&mut harness, &file, |text| text.contains(CLOSE));
    assert_eq!(
        file.text(),
        format!(
            "[[keybinding]]\nkey = \"ctrl-tab\"\ncommand = \"next_tab\"\n\
             disabled = true\n\n[[keybinding]]\nkey = \"{CLOSE}\"\n\
             command = \"next_tab\"\n"
        )
    );
    // A Return meant to save, landing just after the timeout saved a key
    // no command holds, does not start another recording on the key cap
    // focus followed to; a later one does.
    harness.cx.run_until_parked();
    keystrokes(&mut harness, "enter ctrl-alt-y");
    harness
        .cx
        .executor()
        .advance_clock(super::super::settings_window::CHORD_TIMEOUT);
    saved(&mut harness, &file, |text| text.contains("ctrl-alt-y"));
    harness.cx.run_until_parked();
    let followed = with_window(&mut harness, |view, window, cx| {
        view.focus_name(window, cx)
    });
    assert!(followed.starts_with("action:krec:"), "{followed}");
    keystrokes(&mut harness, "enter");
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.recording(cx), None);
    });
    harness
        .cx
        .executor()
        .advance_clock(super::super::settings_window::CHORD_TIMEOUT);
    keystrokes(&mut harness, "enter");
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.recording(cx), Some(Vec::new()));
    });
    keystrokes(&mut harness, "escape");
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
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.recording(cx), None);
    });
    // So does Escape after a first stroke.
    press(&mut harness, "kadd:new_tab");
    keystrokes(&mut harness, "ctrl-k escape");
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.recording(cx), None);
    });
    harness
        .cx
        .executor()
        .advance_clock(super::super::settings_window::CHORD_TIMEOUT);
    harness.cx.run_until_parked();
    assert_eq!(file.text(), before);
}

#[gpui::test]
fn taking_another_commands_key_asks_first_and_cancel_keeps_it(
    cx: &mut TestAppContext,
) {
    let new_tab = if cfg!(target_os = "macos") {
        "cmd-t"
    } else {
        "ctrl-shift-t"
    };
    let file = ScratchConfig::new("keys-reassign", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let record = |harness: &mut DesktopHarness| {
        press(harness, "kadd:new_window");
        keystrokes(harness, new_tab);
        harness
            .cx
            .executor()
            .advance_clock(super::super::settings_window::CHORD_TIMEOUT);
        harness.cx.run_until_parked();
    };
    record(&mut harness);
    with_view(&mut harness, |view, _| {
        assert_eq!(view.dialog_name(), "reassign-key");
    });
    let title = a11y_node(&mut harness, "settings-reassign-key");
    let keys = crate::desktop::key_hint::KeyHint::parse(
        new_tab,
        crate::keymap::Platform::current(),
    )
    .text();
    assert_eq!(
        title["aria"]["label"],
        format!("{keys} is assigned to New Tab"),
        "{title}"
    );
    // Cancel, which has focus, writes nothing, and New Tab keeps the key.
    keystrokes(&mut harness, "escape");
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert_eq!(view.dialog_name(), "none");
    });
    assert_eq!(file.text(), "");
    // Reassign gives the key to New Window.
    record(&mut harness);
    keystrokes(&mut harness, "tab enter");
    saved(&mut harness, &file, |text| text.contains(new_tab));
    assert_eq!(
        file.text(),
        format!(
            "[[keybinding]]\nkey = \"{new_tab}\"\ncommand = \"new_window\"\n"
        )
    );
}

/// Two user entries for different commands under the same predicate, the
/// later one holding `ctrl-alt-b`.
const TWO_USER_KEYS: &str = "[[keybinding]]\nkey = \"ctrl-alt-a\"\n\
                             command = \"new_window\"\n\n[[keybinding]]\n\
                             key = \"ctrl-alt-b\"\ncommand = \"new_tab\"\n";

/// Records `keys` on `row`, letting the chord timeout end the recording.
fn record(harness: &mut DesktopHarness, row: &str, keys: &str) {
    press(harness, &format!("krec:{row}"));
    keystrokes(harness, keys);
    harness
        .cx
        .executor()
        .advance_clock(super::super::settings_window::CHORD_TIMEOUT);
    harness.cx.run_until_parked();
}

#[gpui::test]
fn reassigning_from_a_later_user_entry_removes_it_so_the_key_moves(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-reassign-later", TWO_USER_KEYS);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let window = row(&mut harness, ids::NEW_WINDOW, "ctrl-alt-a");
    record(&mut harness, &window, "ctrl-alt-b");
    with_view(&mut harness, |view, _| {
        assert_eq!(view.dialog_name(), "reassign-key");
    });
    keystrokes(&mut harness, "tab enter");
    saved(&mut harness, &file, |text| !text.contains("new_tab"));
    // Edited in place, the earlier entry would lose to the later one.
    assert_eq!(
        file.text(),
        "[[keybinding]]\nkey = \"ctrl-alt-b\"\ncommand = \"new_window\"\n"
    );
}

#[gpui::test]
fn reassigning_removes_a_holder_that_a_removed_one_had_replaced(
    cx: &mut TestAppContext,
) {
    // Close Window's entry replaces New Tab's, which would take the key
    // back once Close Window's goes.
    let file = ScratchConfig::new(
        "keys-reassign-shadowed",
        &format!(
            "{TWO_USER_KEYS}\n[[keybinding]]\nkey = \"ctrl-alt-b\"\n\
             command = \"close_window\"\n"
        ),
    );
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let window = row(&mut harness, ids::NEW_WINDOW, "ctrl-alt-a");
    record(&mut harness, &window, "ctrl-alt-b");
    keystrokes(&mut harness, "tab enter");
    saved(&mut harness, &file, |text| !text.contains("close_window"));
    assert_eq!(
        file.text(),
        "[[keybinding]]\nkey = \"ctrl-alt-b\"\ncommand = \"new_window\"\n"
    );
}

#[gpui::test]
fn a_reassignment_whose_row_a_queued_write_removes_writes_nothing(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-reassign-queued", TWO_USER_KEYS);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let window = row(&mut harness, ids::NEW_WINDOW, "ctrl-alt-a");
    // The row's removal is still being written when the reassignment
    // queues behind it.
    with_window(&mut harness, |view, window_, cx| {
        view.press(&format!("kdel:{window}"), window_, cx);
        view.reassign_row(&window, "ctrl-alt-b", cx);
    });
    saved(&mut harness, &file, |text| !text.contains("new_window"));
    harness.cx.run_until_parked();
    // New Tab keeps its key rather than losing it to a row that is gone.
    assert_eq!(
        file.text(),
        "[[keybinding]]\nkey = \"ctrl-alt-b\"\ncommand = \"new_tab\"\n"
    );
}

#[gpui::test]
fn a_reassign_prompt_that_a_read_overtook_writes_nothing(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-reassign-stale", TWO_USER_KEYS);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let window = row(&mut harness, ids::NEW_WINDOW, "ctrl-alt-a");
    record(&mut harness, &window, "ctrl-alt-b");
    // Another editor puts an entry first while the prompt is open, so the
    // recorded row's name now names that entry.
    let moved = format!(
        "[[keybinding]]\nkey = \"ctrl-alt-c\"\ncommand = \"split_right\"\n\n\
         {TWO_USER_KEYS}"
    );
    std::fs::write(file.path(), &moved).unwrap();
    with_view(&mut harness, SettingsView::reread);
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert_eq!(view.dialog_name(), "reassign-key");
    });
    keystrokes(&mut harness, "tab enter");
    harness.cx.run_until_parked();
    assert_eq!(file.text(), moved);
    with_view(&mut harness, |view, _| {
        assert_eq!(view.dialog_name(), "none");
        assert!(
            view.notice_text().contains("Record it again"),
            "{}",
            view.notice_text()
        );
    });
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
        // Leaving the field shows the same error.
        view.cancel_key(&amount, cx);
        view.type_key(&amount, "500", window, cx);
        view.blur_key(&amount, cx);
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
        assert_eq!(view.recording(cx), None);
    });
    harness.cx.run_until_parked();
    assert_eq!(file.text(), broken);
}

#[gpui::test]
fn recording_a_key_keeps_the_condition_and_argument_drafts(
    cx: &mut TestAppContext,
) {
    let original = "[[keybinding]]\nkey = \"ctrl-alt-l\"\n\
                    command = \"resize_pane_right\"\nargs = { amount = 5 }\n";
    let file = ScratchConfig::new("keys-record-drafts", original);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    // Drafts that cannot save stay in their fields, with their errors.
    let drafts = |harness: &mut DesktopHarness, row: &str, argument: &str| {
        with_window(harness, |view, window, cx| {
            let when = format!("when:{row}");
            let argument = format!("arg:{row}:{argument}");
            view.type_key(&when, "Terminal &&", window, cx);
            view.enter_key(&when, cx);
            view.type_key(&argument, "five", window, cx);
            view.enter_key(&argument, cx);
            assert!(view.key_error(row).is_some());
        });
    };
    let kept = |harness: &mut DesktopHarness, row: &str, argument: &str| {
        with_view(harness, |view, cx| {
            assert_eq!(
                view.key_text(&format!("when:{row}"), cx),
                "Terminal &&"
            );
            assert_eq!(
                view.key_text(&format!("arg:{row}:{argument}"), cx),
                "five"
            );
            assert!(view.key_error(row).is_some());
        });
    };
    // A user entry changes its key in place.
    drafts(&mut harness, "u0", "amount");
    press(&mut harness, "krec:u0");
    keystrokes(&mut harness, "ctrl-alt-k enter");
    saved(&mut harness, &file, |text| text.contains("ctrl-alt-k"));
    assert_eq!(file.text(), original.replace("ctrl-alt-l", "ctrl-alt-k"));
    kept(&mut harness, "u0", "amount");
    // A changed default becomes a new entry, which takes its drafts.
    let first = row(&mut harness, ids::SELECT_TAB, SELECT_FIRST);
    drafts(&mut harness, &first, "index");
    press(&mut harness, &format!("krec:{first}"));
    keystrokes(&mut harness, "ctrl-alt-1 enter");
    saved(&mut harness, &file, |text| text.contains("ctrl-alt-1"));
    let changed = row(&mut harness, ids::SELECT_TAB, "ctrl-alt-1");
    kept(&mut harness, &changed, "index");
}

/// Resize Pane Right's default, which sets no `amount`.
const RESIZE_RIGHT: &str = if cfg!(target_os = "macos") {
    "cmd-ctrl-right"
} else {
    "ctrl-super-shift-right"
};

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
    with_view(&mut harness, |view, cx| assert!(!view.completion_open(cx)));
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
    with_view(&mut harness, |view, cx| assert!(!view.completion_open(cx)));
    keystrokes(&mut harness, "enter");
    harness.cx.run_until_parked();
    assert_eq!(file.text(), "");
    // A typed name completes on Return, and the next Return saves it.
    with_window(&mut harness, |view, window, cx| {
        view.focus_key(&when, window, cx);
    });
    harness.cx.run_until_parked();
    let mut typing = visual(&harness);
    typing.simulate_input("Te");
    typing.run_until_parked();
    with_view(&mut harness, |view, cx| assert!(view.completion_open(cx)));
    keystrokes(&mut harness, "enter");
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.key_text(&when, cx), "Terminal");
        assert!(!view.completion_open(cx));
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
fn a_default_takes_one_change_until_it_lands(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("keys-default-once", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let next = row(&mut harness, ids::NEXT_TAB, "ctrl-tab");
    let first = row(&mut harness, ids::SELECT_TAB, SELECT_FIRST);
    let index = format!("arg:{first}:index");
    press(&mut harness, "kgo:select_tab");
    harness.cx.run_until_parked();
    // A second press and a step reach each default before its first
    // change lands, while the list still shows it active.
    with_window(&mut harness, |view, window, cx| {
        view.press(&format!("kon:{next}"), window, cx);
        view.press(&format!("kon:{next}"), window, cx);
        view.focus_key(&index, window, cx);
        view.type_key(&index, "4", window, cx);
        view.enter_key(&index, cx);
        view.step_key(&index, true, cx);
    });
    saved(&mut harness, &file, |text| text.contains("index = 4"));
    harness.cx.run_until_parked();
    let text = file.text();
    assert_eq!(text.matches("[[keybinding]]").count(), 3, "{text}");
    assert_eq!(text.matches("disabled = true").count(), 2, "{text}");
    assert!(!text.contains("index = 5"), "{text}");
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

#[gpui::test]
fn focus_follows_a_changed_default_only_while_it_stays_put(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-follow", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let next = row(&mut harness, ids::NEXT_TAB, "ctrl-tab");
    let when = format!("when:{next}");
    press(&mut harness, "kgo:next_tab");
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.focus_key(&when, window, cx);
    });
    harness.cx.run_until_parked();
    // Saving and then searching before the write lands: the search keeps
    // focus, so typed text never lands in the new row's condition.
    with_window(&mut harness, |view, window, cx| {
        view.type_key(&when, "Terminal", window, cx);
        view.enter_key(&when, cx);
        view.command(ids::SETTINGS_SEARCH, window, cx).unwrap();
    });
    saved(&mut harness, &file, |text| {
        text.contains("when = \"Terminal\"")
    });
    harness.cx.run_until_parked();
    assert_eq!(focus_name(&mut harness), "key:search");
}

#[gpui::test]
fn focus_on_a_field_that_goes_away_returns_to_the_window(
    cx: &mut TestAppContext,
) {
    let original = "[[keybinding]]\nkey = \"ctrl-alt-l\"\n\
                    command = \"resize_pane_right\"\nwhen = \"Terminal\"\n";
    let file = ScratchConfig::new("keys-stranded", original);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let find = if cfg!(target_os = "macos") {
        "cmd-f"
    } else {
        "ctrl-f"
    };
    // The section's search, when the section changes.
    press(&mut harness, "kgo:resize_pane_right");
    harness.cx.run_until_parked();
    keystrokes(&mut harness, find);
    harness.cx.run_until_parked();
    assert_eq!(focus_name(&mut harness), "key:search");
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::NEXT_TAB, window, cx).unwrap();
    });
    harness.cx.run_until_parked();
    keystrokes(&mut harness, find);
    harness.cx.run_until_parked();
    assert_eq!(focus_name(&mut harness), "search");
    // A condition field whose entry another editor removed.
    press(&mut harness, "kgo:resize_pane_right");
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.focus_key("when:u0", window, cx);
    });
    harness.cx.run_until_parked();
    assert_eq!(focus_name(&mut harness), "key:when:u0");
    std::fs::write(file.path(), "").unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the reread", settled);
    harness.cx.run_until_parked();
    keystrokes(&mut harness, find);
    harness.cx.run_until_parked();
    assert_eq!(focus_name(&mut harness), "key:search");
}

#[gpui::test]
fn shift_tab_from_an_edited_default_lands_on_its_new_row(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-shift-tab", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let next = row(&mut harness, ids::NEXT_TAB, "ctrl-tab");
    let when = format!("when:{next}");
    press(&mut harness, "kgo:next_tab");
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.focus_key(&when, window, cx);
    });
    harness.cx.run_until_parked();
    // Shift-Tab saves the condition and moves to the default's key cap,
    // which the save switches off; focus goes on to the new row's cap.
    with_window(&mut harness, |view, window, cx| {
        view.type_key(&when, "Terminal", window, cx);
        view.command(ids::SETTINGS_FOCUS_PREVIOUS, window, cx)
            .unwrap();
        // The test window does not deliver the blur that saves.
        view.blur_key(&when, cx);
    });
    saved(&mut harness, &file, |text| {
        text.contains("when = \"Terminal\"")
    });
    harness.cx.run_until_parked();
    assert_eq!(focus_name(&mut harness), "action:krec:u1");
    // An empty number argument has no value to step from.
    let resize = row(&mut harness, ids::RESIZE_PANE_RIGHT, RESIZE_RIGHT);
    let amount = format!("arg:{resize}:amount");
    let before = file.text();
    with_window(&mut harness, |view, window, cx| {
        view.focus_key(&amount, window, cx);
        view.step_key(&amount, true, cx);
    });
    harness.cx.run_until_parked();
    assert_eq!(file.text(), before);
}

#[gpui::test]
fn closing_saves_a_focused_condition_or_stays_to_show_its_error(
    cx: &mut TestAppContext,
) {
    let original = "[[keybinding]]\nkey = \"ctrl-alt-l\"\n\
                    command = \"resize_pane_right\"\n";
    let file = ScratchConfig::new("keys-close", original);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    press(&mut harness, "kgo:resize_pane_right");
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.focus_key("when:u0", window, cx);
    });
    harness.cx.run_until_parked();
    // A condition that does not parse keeps the window open.
    with_window(&mut harness, |view, window, cx| {
        view.type_key("when:u0", "Terminal &&", window, cx);
        view.command(ids::CLOSE_WINDOW, window, cx).unwrap();
    });
    harness.cx.run_until_parked();
    assert!(harness.cx.update(|cx| settings(cx).is_some()));
    assert_eq!(file.text(), original);
    // A valid one saves, and the window closes once it has.
    with_window(&mut harness, |view, window, cx| {
        view.type_key("when:u0", "Terminal", window, cx);
        view.command(ids::CLOSE_WINDOW, window, cx).unwrap();
    });
    harness.wait_for("the save and the close", |cx| settings(cx).is_none());
    assert!(
        file.text().contains("when = \"Terminal\""),
        "{}",
        file.text()
    );
}

/// Three shortcuts the user added, in file order.
const THREE: &str = "[[keybinding]]\nkey = \"ctrl-alt-a\"\ncommand = \"new_tab\"\n\n\
                     [[keybinding]]\nkey = \"ctrl-alt-b\"\ncommand = \"new_window\"\n\n\
                     [[keybinding]]\nkey = \"ctrl-alt-c\"\ncommand = \"close_tab\"\n";

#[gpui::test]
fn a_queued_edit_after_a_removal_changes_the_entry_the_user_saw(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-queued", THREE);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    // The switch is pressed before the removal ahead of it lands.
    with_window(&mut harness, |view, window, cx| {
        view.press("kdel:u0", window, cx);
        view.press("kon:u1", window, cx);
    });
    saved(&mut harness, &file, |text| !text.contains("ctrl-alt-a"));
    harness.cx.run_until_parked();
    assert!(harness.cx.update(settled));
    assert_eq!(
        file.text(),
        "[[keybinding]]\nkey = \"ctrl-alt-b\"\ncommand = \"new_window\"\n\
         disabled = true\n\n[[keybinding]]\nkey = \"ctrl-alt-c\"\ncommand = \"close_tab\"\n"
    );
}

#[gpui::test]
fn a_recorded_key_queued_behind_a_removal_keeps_the_condition_draft(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-record-queued", THREE);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    press(&mut harness, "kgo:new_window");
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.type_key("when:u1", "Terminal &&", window, cx);
        view.enter_key("when:u1", cx);
        assert!(view.key_error("u1").is_some());
        view.press("kdel:u0", window, cx);
        view.press("krec:u1", window, cx);
    });
    keystrokes(&mut harness, "ctrl-alt-k enter");
    // The removal had not landed when the recording ended.
    assert!(file.text().contains("ctrl-alt-a"));
    saved(&mut harness, &file, |text| {
        !text.contains("ctrl-alt-a") && text.contains("ctrl-alt-k")
    });
    harness.cx.run_until_parked();
    assert_eq!(row(&mut harness, ids::NEW_WINDOW, "ctrl-alt-k"), "u0");
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.key_text("when:u0", cx), "Terminal &&");
        assert!(view.key_error("u0").is_some());
    });
}

#[gpui::test]
fn a_recorded_key_keeps_its_own_draft_when_another_editor_removes_a_twin(
    cx: &mut TestAppContext,
) {
    // Two shortcuts that differ only in their keys.
    let twins = "[[keybinding]]\nkey = \"ctrl-alt-a\"\ncommand = \"new_window\"\n\n\
                 [[keybinding]]\nkey = \"ctrl-alt-b\"\ncommand = \"new_window\"\n";
    let file = ScratchConfig::new("keys-record-twin", twins);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    press(&mut harness, "kgo:new_window");
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.type_key("when:u0", "Terminal ||", window, cx);
        view.enter_key("when:u0", cx);
        view.type_key("when:u1", "Terminal &&", window, cx);
        view.enter_key("when:u1", cx);
        assert!(view.key_error("u1").is_some());
        view.press("krec:u1", window, cx);
    });
    // Another editor removes the first shortcut before the key lands.
    let shifted = twins.split_once("\n\n").unwrap().1.to_owned();
    std::fs::write(file.path(), &shifted).unwrap();
    keystrokes(&mut harness, "ctrl-alt-k enter");
    saved(&mut harness, &file, |text| {
        !text.contains("ctrl-alt-a") && text.contains("ctrl-alt-k")
    });
    harness.cx.run_until_parked();
    assert_eq!(row(&mut harness, ids::NEW_WINDOW, "ctrl-alt-k"), "u0");
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.key_text("when:u0", cx), "Terminal &&");
    });
}

#[gpui::test]
fn a_recorded_key_keeps_its_draft_on_the_rebound_one_of_identical_entries(
    cx: &mut TestAppContext,
) {
    let entry = "[[keybinding]]\nkey = \"ctrl-alt-a\"\ncommand = \"new_tab\"\n";
    let file = ScratchConfig::new(
        "keys-record-identical",
        &format!("{entry}\n{entry}"),
    );
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    press(&mut harness, "kgo:new_tab");
    harness.cx.run_until_parked();
    // The second entry is the active one.
    with_window(&mut harness, |view, window, cx| {
        view.type_key("when:u1", "Terminal &&", window, cx);
        view.enter_key("when:u1", cx);
        assert!(view.key_error("u1").is_some());
        view.press("krec:u1", window, cx);
    });
    keystrokes(&mut harness, "ctrl-alt-k enter");
    saved(&mut harness, &file, |text| text.contains("ctrl-alt-k"));
    harness.cx.run_until_parked();
    assert_eq!(row(&mut harness, ids::NEW_TAB, "ctrl-alt-k"), "u1");
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.key_text("when:u1", cx), "Terminal &&");
        assert_eq!(view.key_text("when:u0", cx), "");
        assert!(view.key_error("u0").is_none());
    });
}

#[gpui::test]
fn a_recorded_key_and_another_draft_both_follow_entries_another_editor_swapped(
    cx: &mut TestAppContext,
) {
    let pair = "[[keybinding]]\nkey = \"ctrl-alt-a\"\ncommand = \"new_window\"\n\n\
                [[keybinding]]\nkey = \"ctrl-alt-b\"\ncommand = \"new_window\"\n\
                when = \"Terminal\"\n";
    let file = ScratchConfig::new("keys-record-swap", pair);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    press(&mut harness, "kgo:new_window");
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.type_key("when:u0", "Terminal ||", window, cx);
        view.enter_key("when:u0", cx);
        view.type_key("when:u1", "Terminal &&", window, cx);
        view.enter_key("when:u1", cx);
        view.press("krec:u1", window, cx);
    });
    // Another editor swaps the two entries before the key lands.
    let (first, second) = pair.split_once("\n\n").unwrap();
    std::fs::write(file.path(), format!("{second}\n{first}")).unwrap();
    keystrokes(&mut harness, "ctrl-alt-k enter");
    saved(&mut harness, &file, |text| text.contains("ctrl-alt-k"));
    harness.cx.run_until_parked();
    assert_eq!(row(&mut harness, ids::NEW_WINDOW, "ctrl-alt-k"), "u0");
    assert_eq!(row(&mut harness, ids::NEW_WINDOW, "ctrl-alt-a"), "u1");
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.key_text("when:u0", cx), "Terminal &&");
        assert_eq!(view.key_text("when:u1", cx), "Terminal ||");
    });
}

#[gpui::test]
fn a_recorded_key_keeps_its_draft_when_another_editor_moved_entries_before_a_queued_write(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-record-queued-shift", THREE);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    press(&mut harness, "kgo:close_tab");
    harness.cx.run_until_parked();
    // Another editor removes the second entry before this window's removal
    // of the first reads the file, so the queued key's locator, moved up
    // past one removal, names a row that no longer exists.
    let without_second = THREE.replacen(
        "[[keybinding]]\nkey = \"ctrl-alt-b\"\ncommand = \"new_window\"\n\n",
        "",
        1,
    );
    with_window(&mut harness, |view, window, cx| {
        view.type_key("when:u2", "Terminal &&", window, cx);
        view.enter_key("when:u2", cx);
        assert!(view.key_error("u2").is_some());
        view.press("kdel:u0", window, cx);
        view.press("krec:u2", window, cx);
        std::fs::write(file.path(), &without_second).unwrap();
    });
    keystrokes(&mut harness, "ctrl-alt-k enter");
    saved(&mut harness, &file, |text| {
        !text.contains("ctrl-alt-a") && text.contains("ctrl-alt-k")
    });
    harness.cx.run_until_parked();
    assert_eq!(row(&mut harness, ids::CLOSE_TAB, "ctrl-alt-k"), "u0");
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.key_text("when:u0", cx), "Terminal &&");
    });
}

#[gpui::test]
fn a_switched_off_shortcuts_key_cap_says_why_it_refuses(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new(
        "keys-off-cap",
        "[[keybinding]]\nkey = \"ctrl-alt-b\"\ncommand = \"new_window\"\n\
         disabled = true\n",
    );
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    press(&mut harness, "kgo:new_window");
    harness.cx.run_until_parked();
    let name = row(&mut harness, ids::NEW_WINDOW, "ctrl-alt-b");
    let aria =
        a11y_node(&mut harness, &format!("settings-krec-{name}"))["aria"]
            .clone();
    assert_eq!(aria["disabled"], true, "{aria}");
    assert_eq!(
        aria["description"], "Only an active shortcut can change",
        "{aria}"
    );
}

#[gpui::test]
fn a_timer_from_an_earlier_recording_does_not_end_a_later_one(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-timers", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let timeout = super::super::settings_window::CHORD_TIMEOUT;
    press(&mut harness, "kadd:new_window");
    keystrokes(&mut harness, "ctrl-k");
    harness.cx.executor().advance_clock(timeout / 5);
    keystrokes(&mut harness, "escape");
    press(&mut harness, "kadd:new_window");
    keystrokes(&mut harness, "ctrl-j");
    // The first recording's timer fires now; the second's has time left.
    harness.cx.executor().advance_clock(timeout * 9 / 10);
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.recording(cx), Some(vec!["ctrl-j".to_owned()]));
    });
    assert_eq!(file.text(), "");
}

#[gpui::test]
fn argument_picks_write_only_real_changes_on_active_shortcuts(
    cx: &mut TestAppContext,
) {
    use huterm_protocol::ArgumentKind;
    let original = "[[keybinding]]\nkey = \"ctrl-alt-q\"\ncommand = \"toggle_quake\"\n\
                    disabled = true\n\n[[keybinding]]\nkey = \"ctrl-alt-w\"\n\
                    command = \"toggle_quake\"\n";
    let file = ScratchConfig::new("keys-picks", original);
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config
            .quake
            .profiles
            .insert("logs".into(), crate::quake::Profile::default());
    });
    open_settings(&mut harness, &file.path());
    let quake = ArgumentKind::QuakeProfile;
    with_view(&mut harness, |view, cx| {
        // A switched-off shortcut's argument does not edit.
        view.pick_key_argument("u0", "profile", quake, "logs", cx);
        // An unset profile is already `default`.
        view.pick_key_argument("u1", "profile", quake, "default", cx);
    });
    harness.cx.run_until_parked();
    assert_eq!(file.text(), original);
    with_view(&mut harness, |view, cx| {
        view.pick_key_argument("u1", "profile", quake, "logs", cx);
    });
    saved(&mut harness, &file, |text| text.contains("profile"));
    let picked = file.text();
    // Picking the value it already has writes nothing.
    with_view(&mut harness, |view, cx| {
        view.pick_key_argument("u1", "profile", quake, "logs", cx);
    });
    harness.cx.run_until_parked();
    assert_eq!(file.text(), picked);
}

#[gpui::test]
fn queued_steps_on_one_shortcut_build_on_each_other(cx: &mut TestAppContext) {
    let original = "[[keybinding]]\nkey = \"ctrl-alt-l\"\n\
                    command = \"resize_pane_right\"\nargs = { amount = 5 }\n";
    let file = ScratchConfig::new("keys-steps", original);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.type_key("arg:u0:amount", "5", window, cx);
        // Two steps before the first write lands.
        view.step_key("arg:u0:amount", true, cx);
        view.step_key("arg:u0:amount", true, cx);
    });
    saved(&mut harness, &file, |text| text.contains("amount = 7"));
    with_view(&mut harness, |view, _| {
        assert_eq!(view.key_error("u0"), None);
    });
}

#[gpui::test]
fn a_draft_follows_its_shortcut_past_one_sharing_its_key(
    cx: &mut TestAppContext,
) {
    let original = "[[keybinding]]\nkey = \"ctrl-alt-a\"\ncommand = \"new_tab\"\n\n\
                    [[keybinding]]\nkey = \"ctrl-alt-b\"\ncommand = \"copy\"\n\
                    when = \"selection\"\n\n[[keybinding]]\nkey = \"ctrl-alt-b\"\n\
                    command = \"copy\"\nwhen = \"Terminal\"\n";
    let file = ScratchConfig::new("keys-twins", original);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    press(&mut harness, "kgo:copy");
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.focus_key("when:u1", window, cx);
    });
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.type_key("when:u1", "selection && split", window, cx);
    });
    // Another editor removes the first entry; the draft's shortcut is now
    // row u0, and row u1 is the other copy.
    let shifted = original.split_once("\n\n").unwrap().1.to_owned();
    std::fs::write(file.path(), &shifted).unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the reread", settled);
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.key_text("when:u0", cx), "selection && split");
        assert_eq!(view.key_text("when:u1", cx), "Terminal");
        view.blur_key("when:u0", cx);
    });
    saved(&mut harness, &file, |text| text.contains("split"));
    assert_eq!(
        file.text(),
        shifted.replacen(
            "when = \"selection\"",
            "when = \"selection && split\"",
            1
        )
    );
}

#[gpui::test]
fn a_draft_refused_by_an_invalid_file_waits_for_the_fix(
    cx: &mut TestAppContext,
) {
    let original =
        "[[keybinding]]\nkey = \"ctrl-alt-l\"\ncommand = \"new_tab\"\n";
    let file = ScratchConfig::new("keys-refused", original);
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.general.watch_config = false;
    });
    open_settings(&mut harness, &file.path());
    // The file breaks after Settings read it, so the write is refused.
    std::fs::write(file.path(), format!("{original}[window\n")).unwrap();
    with_window(&mut harness, |view, window, cx| {
        view.type_key("when:u0", "Terminal", window, cx);
        view.enter_key("when:u0", cx);
    });
    harness.wait_for("the refusal", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.key_error("u0").is_some())
                    .unwrap_or(false)
            })
    });
    // Fixed and reread, the draft is still there to save.
    std::fs::write(file.path(), original).unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the reread", settled);
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.key_text("when:u0", cx), "Terminal");
        view.enter_key("when:u0", cx);
    });
    saved(&mut harness, &file, |text| {
        text.contains("when = \"Terminal\"")
    });
}

#[cfg(unix)]
#[gpui::test]
fn a_failed_condition_save_stays_a_draft_to_retry(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    let original =
        "[[keybinding]]\nkey = \"ctrl-alt-l\"\ncommand = \"new_tab\"\n";
    let file = ScratchConfig::new("keys-retry", original);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let mode = |bits| std::fs::Permissions::from_mode(bits);
    // A directory the save cannot write its temporary file into.
    std::fs::set_permissions(&file.0, mode(0o500)).unwrap();
    if std::fs::write(file.0.join("probe"), "").is_ok() {
        std::fs::set_permissions(&file.0, mode(0o700)).unwrap();
        return;
    }
    with_window(&mut harness, |view, window, cx| {
        view.type_key("when:u0", "Terminal", window, cx);
        view.enter_key("when:u0", cx);
    });
    harness.wait_for("the failed save", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.key_error("u0").is_some())
                    .unwrap_or(false)
            })
    });
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.key_text("when:u0", cx), "Terminal");
    });
    std::fs::set_permissions(&file.0, mode(0o700)).unwrap();
    // Return again retries the same text.
    with_view(&mut harness, |view, cx| view.enter_key("when:u0", cx));
    saved(&mut harness, &file, |text| {
        text.contains("when = \"Terminal\"")
    });
}

#[gpui::test]
fn focus_stays_on_a_theme_card_across_a_render(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("keys-theme-focus", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.command(ids::NEXT_TAB, window, cx).unwrap();
    });
    harness.cx.run_until_parked();
    press(&mut harness, "theme:nord");
    saved(&mut harness, &file, |text| text.contains("nord"));
    harness.cx.run_until_parked();
    assert_eq!(focus_name(&mut harness), "action:theme:nord");
}

#[gpui::test]
fn an_external_edit_that_shifts_entries_moves_a_draft_with_it(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-shift", THREE);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    press(&mut harness, "kgo:new_window");
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.focus_key("when:u1", window, cx);
    });
    harness.cx.run_until_parked();
    assert_eq!(focus_name(&mut harness), "key:when:u1");
    with_window(&mut harness, |view, window, cx| {
        view.type_key("when:u1", "Terminal", window, cx);
    });
    // Another editor removes the first entry, so the draft's shortcut is
    // now row u0 and row u1 is close_tab.
    let shifted = THREE.split_once("\n\n").unwrap().1.to_owned();
    std::fs::write(file.path(), &shifted).unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the reread", settled);
    assert_eq!(focus_name(&mut harness), "key:when:u0");
    with_view(&mut harness, |view, cx| view.blur_key("when:u0", cx));
    saved(&mut harness, &file, |text| text.contains("when"));
    assert_eq!(
        file.text(),
        shifted.replacen(
            "command = \"new_window\"\n",
            "command = \"new_window\"\nwhen = \"Terminal\"\n",
            1
        )
    );
}

/// Two `copy` shortcuts the user added.
const TWINS: &str = "[[keybinding]]\nkey = \"ctrl-alt-a\"\ncommand = \"copy\"\n\
                     when = \"selection\"\n\n[[keybinding]]\nkey = \"ctrl-alt-b\"\n\
                     command = \"copy\"\nwhen = \"Terminal\"\n";

#[gpui::test]
fn a_queued_edit_never_lands_on_a_shortcut_that_shifted_into_its_place(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-twin-shift", TWINS);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    // A setting's write runs first; the condition edit for the first copy
    // waits behind it, and another editor removes that copy meanwhile.
    let shifted = TWINS.split_once("\n\n").unwrap().1.to_owned();
    with_window(&mut harness, |view, window, cx| {
        view.set("font.size", Some(toml::Value::Float(15.0)), cx);
        view.type_key("when:u0", "selection && split", window, cx);
        view.enter_key("when:u0", cx);
        std::fs::write(file.path(), &shifted).unwrap();
    });
    harness.wait_for("both writes", settled);
    harness.cx.run_until_parked();
    let text = file.text();
    assert!(text.contains("when = \"Terminal\""), "{text}");
    assert!(!text.contains("split"), "{text}");
}

#[gpui::test]
fn a_conflicted_draft_survives_the_refresh(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("keys-conflict", TWINS);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    // Another editor changes the same entry before the write applies.
    with_window(&mut harness, |view, window, cx| {
        view.type_key("when:u0", "selection && split", window, cx);
        view.enter_key("when:u0", cx);
        std::fs::write(
            file.path(),
            TWINS.replacen("when = \"selection\"", "when = \"Workspace\"", 1),
        )
        .unwrap();
    });
    harness.wait_for("the conflict and its refresh", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.key_error("u0").is_some())
                    .unwrap_or(false)
            })
    });
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.key_text("when:u0", cx), "selection && split");
        assert!(view.key_error("u0").is_some());
    });
}

#[gpui::test]
fn a_focused_draft_survives_a_reread_that_finds_the_file_invalid(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keys-invalid-draft", TWINS);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    press(&mut harness, "kgo:copy");
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.focus_key("when:u0", window, cx);
    });
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        view.type_key("when:u0", "selection && split", window, cx);
    });
    std::fs::write(file.path(), format!("{TWINS}[window\n")).unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the invalid reread", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.read_only())
                    .unwrap_or(false)
            })
    });
    // Focus leaves the hidden field; its save waits for the fix.
    with_view(&mut harness, |view, cx| view.blur_key("when:u0", cx));
    std::fs::write(file.path(), TWINS).unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the good reread", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| !view.read_only())
                    .unwrap_or(false)
            })
    });
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.key_text("when:u0", cx), "selection && split");
        view.enter_key("when:u0", cx);
    });
    saved(&mut harness, &file, |text| text.contains("split"));
}

#[gpui::test]
fn a_queued_removal_keeps_its_own_entry_among_identical_twins(
    cx: &mut TestAppContext,
) {
    let entry = "[[keybinding]]\nkey = \"ctrl-alt-a\"\ncommand = \"new_tab\"\n";
    let original = format!("{entry}\n{entry}");
    let file = ScratchConfig::new("keys-identical", &original);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    // The second twin's condition saves first; removing the first waits.
    with_window(&mut harness, |view, window, cx| {
        view.type_key("when:u1", "Terminal", window, cx);
        view.enter_key("when:u1", cx);
        view.press("kdel:u0", window, cx);
    });
    harness.wait_for("both writes", settled);
    harness.cx.run_until_parked();
    assert_eq!(
        file.text(),
        "[[keybinding]]\nkey = \"ctrl-alt-a\"\ncommand = \"new_tab\"\nwhen = \"Terminal\"\n"
    );
}

#[gpui::test]
fn a_draft_behind_a_setting_write_refused_by_an_invalid_file_waits_for_the_fix(
    cx: &mut TestAppContext,
) {
    let original =
        "[[keybinding]]\nkey = \"ctrl-alt-l\"\ncommand = \"new_tab\"\n";
    let file = ScratchConfig::new("keys-behind-setting", original);
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.general.watch_config = false;
    });
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.set("font.size", Some(toml::Value::Float(15.0)), cx);
        view.type_key("when:u0", "Terminal", window, cx);
        view.enter_key("when:u0", cx);
        std::fs::write(file.path(), format!("{original}[window\n")).unwrap();
    });
    harness.wait_for("the refusal", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.key_error("u0").is_some())
                    .unwrap_or(false)
            })
    });
    std::fs::write(file.path(), original).unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the reread", settled);
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.key_text("when:u0", cx), "Terminal");
        view.enter_key("when:u0", cx);
    });
    saved(&mut harness, &file, |text| {
        text.contains("when = \"Terminal\"")
    });
}

#[gpui::test]
fn a_read_dropped_behind_a_failed_write_is_read_again(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    let file = ScratchConfig::new("keys-reread", THREE);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let added = format!(
        "{THREE}\n[[keybinding]]\nkey = \"ctrl-alt-d\"\ncommand = \"new_tab\"\n"
    );
    let permissions = |mode| std::fs::Permissions::from_mode(mode);
    // The removal cannot replace the file in a read-only directory, and
    // another editor adds a shortcut while it runs.
    std::fs::set_permissions(&file.0, permissions(0o555)).unwrap();
    with_window(&mut harness, |view, window, cx| {
        view.press("kdel:u0", window, cx);
        std::fs::write(file.path(), &added).unwrap();
        view.follow_reload(cx);
    });
    harness.wait_for("the failed removal", settled);
    harness.cx.run_until_parked();
    std::fs::set_permissions(&file.0, permissions(0o755)).unwrap();
    with_view(&mut harness, |view, _| {
        assert!(view.key_row(ids::NEW_TAB, "ctrl-alt-d").is_some());
    });
}

#[gpui::test]
fn an_arrange_panes_shortcut_picks_its_layout_from_every_layout(
    cx: &mut TestAppContext,
) {
    let original =
        "[[keybinding]]\nkey = \"ctrl-alt-l\"\ncommand = \"arrange_panes\"\n";
    let file = ScratchConfig::new("keys-layouts", original);
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config
            .panes
            .layouts
            .insert("dev".into(), "columns(pane 55%, rows(*))".into());
    });
    open_settings(&mut harness, &file.path());
    // The row offers the argument as a select, not hidden as an unknown
    // kind.
    press(&mut harness, "kgo:arrange_panes");
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert!(view.drew("karg:u0:layout"));
    });
    with_window(&mut harness, |view, window, cx| {
        view.open_key_argument("u0", "layout", window, cx);
    });
    with_view(&mut harness, |view, cx| {
        // Unset, the palette asks for the layout.
        assert_eq!(
            view.menu_labels(cx),
            [
                "Ask",
                "Equal Columns",
                "Equal Rows",
                "Main Left",
                "Main Top",
                "Tiled",
                "dev"
            ]
        );
        // The titles are labels only: each item writes the layout's name.
        assert_eq!(
            view.menu_ids(cx)[1..],
            ["columns", "rows", "main_left", "main_top", "tiled", "dev"]
        );
    });
    with_window(&mut harness, |view, window, cx| {
        view.dismiss_menu(window, cx);
    });
    with_view(&mut harness, |view, cx| {
        view.pick_key_argument(
            "u0",
            "layout",
            huterm_protocol::ArgumentKind::Layout,
            "dev",
            cx,
        );
    });
    saved(&mut harness, &file, |text| text.contains("layout"));
    assert_eq!(
        file.text(),
        "[[keybinding]]\nkey = \"ctrl-alt-l\"\ncommand = \"arrange_panes\"\n\
         args = { layout = \"dev\" }\n"
    );
}

#[gpui::test]
fn removing_an_unbind_entry_reports_the_save_through_the_watchers_reload(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new(
        "keys-unbind-status",
        "[[keybinding]]\nkey = \"alt-q\"\ncommand = \"unbind\"\n",
    );
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    press(&mut harness, "kunbind:0");
    saved(&mut harness, &file, |text| !text.contains("alt-q"));
    harness.cx.run_until_parked();
    // The watcher fingerprinted the write against the inputs before the
    // save's reload, so its report starts another reload.
    let watcher_reload = |harness: &mut DesktopHarness| {
        harness.cx.update(|cx| {
            assert!(!super::reloading(cx));
            super::handle_watch_report(cx, super::config_changed());
            assert!(super::reloading(cx), "the report started no reload");
        });
        harness.cx.run_until_parked();
    };
    watcher_reload(&mut harness);
    with_view(&mut harness, |view, _| {
        assert_eq!(view.status_text(), "Saved");
    });
    // Another editor's change still reports itself.
    std::fs::write(file.path(), "[font]\nsize = 15.0\n").unwrap();
    watcher_reload(&mut harness);
    with_view(&mut harness, |view, _| {
        assert_eq!(view.status_text(), "Reloaded");
    });
}
