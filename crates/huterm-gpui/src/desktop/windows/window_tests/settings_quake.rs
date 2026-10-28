//! The Settings window's Quake windows section through the desktop
//! harness: profiles and global shortcuts.

use toml::Value;

use super::super::settings_window::SettingsView;

use super::settings::{
    ScratchConfig, open_settings, settings, settled, with_view, with_window,
};
use super::settings_controls::{a11y_node, keys, visual};
use super::*;

const LOGS: &str = "# Mine.\n\n[quake.profiles.logs]\nheight = 0.3 # short\n";

fn press(harness: &mut DesktopHarness, key: &str) {
    with_window(harness, |view, window, cx| view.press(key, window, cx));
    harness.cx.run_until_parked();
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
    harness.wait_for("the save", |cx| settled(cx) && check(&file.text()));
}

/// Opens the delete alert from Delete profile and answers Delete.
fn confirm_delete(harness: &mut DesktopHarness) {
    press(harness, "qdel");
    keys(harness, "tab enter");
}

/// Opens Settings on the Quake windows section.
fn open_quake(harness: &mut DesktopHarness, file: &ScratchConfig) {
    open_settings(harness, &file.path());
    press(harness, "qgo");
}

#[gpui::test]
fn profile_fields_write_and_reset_only_their_key(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("quake-fields", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.shown_profile_name(), "default");
        view.set(
            "quake.profiles.default.position",
            Some(Value::String("bottom".into())),
            cx,
        );
    });
    saved(&mut harness, &file, |text| text.contains("bottom"));
    assert_eq!(
        file.text(),
        format!("{LOGS}\n[quake.profiles.default]\nposition = \"bottom\"\n")
    );
    with_view(&mut harness, |view, cx| {
        view.set("quake.profiles.logs.height", None, cx);
    });
    saved(&mut harness, &file, |text| !text.contains("0.3"));
    assert_eq!(
        file.text(),
        "# Mine.\n\n[quake.profiles.logs]\n\n[quake.profiles.default]\n\
         position = \"bottom\"\n"
    );
}

#[gpui::test]
fn new_profiles_need_a_new_bare_name(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("quake-new", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    for name in ["", "my logs", "logs", "default"] {
        with_window(&mut harness, |view, window, cx| {
            view.type_profile_name(name, window, cx);
            view.answer("qcreate", window, cx);
            assert!(view.quake_message(cx).is_some(), "{name:?} was accepted");
        });
    }
    harness.cx.run_until_parked();
    assert_eq!(file.text(), LOGS);
    with_window(&mut harness, |view, window, cx| {
        view.type_profile_name("build-1", window, cx);
        view.answer("qcreate", window, cx);
    });
    saved(&mut harness, &file, |text| text.contains("build-1"));
    assert_eq!(file.text(), format!("{LOGS}\n[quake.profiles.build-1]\n"));
    with_view(&mut harness, |view, _| {
        assert_eq!(view.shown_profile_name(), "build-1");
        assert_eq!(view.profile_names(), ["default", "build-1", "logs"]);
    });
}

#[gpui::test]
fn deleting_a_profile_waits_for_its_global_shortcuts(cx: &mut TestAppContext) {
    let original = format!(
        "{LOGS}\n[[global_keybinding]]\nkey = \"ctrl-alt-shift-y\"\n\
         command = \"toggle_quake\"\nargs = {{ profile = \"logs\" }}\n"
    );
    let file = ScratchConfig::new("quake-delete", &original);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    with_view(&mut harness, |view, _| view.pick_profile("logs"));
    // Delete profile says why at once, in an alert with only OK.
    press(&mut harness, "qdel");
    visual(&harness).run_until_parked();
    with_window(&mut harness, |view, window, cx| {
        assert_eq!(view.dialog_name(), "delete-refused");
        assert_eq!(view.focus_name(window, cx), "dialog:qdelno");
    });
    let alert = a11y_node(&mut harness, "settings-delete-profile");
    assert_eq!(
        alert["aria"]["label"],
        "The \u{201c}logs\u{201d} profile cannot be deleted"
    );
    let why = alert["aria"]["description"].as_str().unwrap_or_default();
    assert!(why.contains("uses this profile"), "{alert}");
    keys(&harness, "enter");
    with_window(&mut harness, |view, window, cx| {
        assert_eq!(view.dialog_name(), "none");
        assert_eq!(view.focus_name(window, cx), "action:qdel");
    });
    assert_eq!(file.text(), original);
    // With its shortcut gone, the profile goes.
    press(&mut harness, "gdel:0");
    saved(&mut harness, &file, |text| {
        !text.contains("global_keybinding")
    });
    with_view(&mut harness, |view, _| view.pick_profile("logs"));
    confirm_delete(&mut harness);
    saved(&mut harness, &file, |text| !text.contains("logs"));
    assert_eq!(file.text(), "# Mine.\n");
    with_view(&mut harness, |view, _| {
        assert_eq!(view.shown_profile_name(), "default");
    });
}

#[gpui::test]
fn global_shortcuts_record_change_and_go(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("quake-globals", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    // A new shortcut toggles the profile on show.
    with_view(&mut harness, |view, _| view.pick_profile("logs"));
    press(&mut harness, "gadd");
    keystrokes(&mut harness, "ctrl-alt-shift-y");
    saved(&mut harness, &file, |text| {
        text.contains("global_keybinding")
    });
    assert_eq!(
        file.text(),
        format!(
            "{LOGS}\n[[global_keybinding]]\nkey = \"ctrl-alt-shift-y\"\n\
             command = \"toggle_quake\"\nargs = {{ profile = \"logs\" }}\n"
        )
    );
    with_view(&mut harness, |view, cx| {
        view.pick_global(0, "command", "show_quake", cx);
    });
    saved(&mut harness, &file, |text| text.contains("show_quake"));
    with_view(&mut harness, |view, cx| {
        view.pick_global(0, "profile", "default", cx);
    });
    saved(&mut harness, &file, |text| !text.contains("args"));
    press(&mut harness, "grec:0");
    keystrokes(&mut harness, "ctrl-alt-shift-u");
    saved(&mut harness, &file, |text| {
        text.contains("ctrl-alt-shift-u")
    });
    assert_eq!(
        file.text(),
        format!(
            "{LOGS}\n[[global_keybinding]]\nkey = \"ctrl-alt-shift-u\"\n\
             command = \"show_quake\"\n"
        )
    );
    press(&mut harness, "gdel:0");
    saved(&mut harness, &file, |text| {
        !text.contains("global_keybinding")
    });
    assert_eq!(file.text(), LOGS);
}

#[gpui::test]
fn a_global_key_an_in_app_binding_starts_with_writes_nothing(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-conflict", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    press(&mut harness, "gadd");
    keystrokes(
        &mut harness,
        if cfg!(target_os = "macos") {
            "cmd-t"
        } else {
            "ctrl-shift-t"
        },
    );
    harness.wait_for("the refused save", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| {
                        view.global_error("gadd").is_some()
                    })
                    .unwrap_or(false)
            })
    });
    assert_eq!(file.text(), LOGS);
    // The message goes once another editor changes the shortcuts.
    std::fs::write(
        file.path(),
        format!(
            "{LOGS}\n[[global_keybinding]]\nkey = \"ctrl-alt-shift-y\"\n\
             command = \"toggle_quake\"\n"
        ),
    )
    .unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the reread", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| {
                        view.global_error("gadd").is_none()
                    })
                    .unwrap_or(false)
            })
    });
}

#[gpui::test]
fn menus_open_on_the_value_their_control_shows(cx: &mut TestAppContext) {
    let original = format!(
        "{LOGS}\n[[global_keybinding]]\nkey = \"ctrl-alt-shift-y\"\n\
         command = \"hide_quake\"\nargs = {{ profile = \"logs\" }}\n"
    );
    let file = ScratchConfig::new("quake-menus", &original);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    with_view(&mut harness, |view, _| view.pick_profile("logs"));
    // Return in a menu just opened keeps what its control shows.
    for key in ["qprof", "gcmd:0", "gprofile:0"] {
        press(&mut harness, key);
        with_window(&mut harness, |view, window, cx| {
            view.command(ids::MENU_CONFIRM, window, cx).unwrap();
        });
        harness.cx.run_until_parked();
    }
    with_view(&mut harness, |view, _| {
        assert_eq!(view.shown_profile_name(), "logs");
    });
    harness.wait_for("any save", settled);
    assert_eq!(file.text(), original);
}

#[gpui::test]
fn return_and_escape_reach_the_name_field_and_the_delete_prompt(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-keys", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.type_profile_name("scratch", window, cx);
    });
    harness.cx.run_until_parked();
    keystrokes(&mut harness, "escape");
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert_eq!(view.quake_prompts(), (false, false));
    });
    with_window(&mut harness, |view, window, cx| {
        view.type_profile_name("build", window, cx);
    });
    harness.cx.run_until_parked();
    keystrokes(&mut harness, "enter");
    saved(&mut harness, &file, |text| {
        text.contains("[quake.profiles.build]")
    });
    press(&mut harness, "qdel");
    with_view(&mut harness, |view, _| {
        assert_eq!(view.quake_prompts(), (false, true));
    });
    keystrokes(&mut harness, "escape");
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert_eq!(view.quake_prompts(), (false, false));
    });
}

#[gpui::test]
fn a_profile_whose_name_needs_quoting_stays_in_the_file(
    cx: &mut TestAppContext,
) {
    let original = "[quake.profiles.\"my logs\"]\nheight = 0.3\n";
    let file = ScratchConfig::new("quake-quoted", original);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    with_view(&mut harness, |view, _| {
        assert_eq!(view.profile_names(), ["default", "my logs"]);
        view.pick_profile("my logs");
    });
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert_eq!(view.shown_profile_name(), "my logs");
        assert!(view.profile_field("my logs", "height").is_none());
        assert!(view.profile_field("logs", "height").is_some());
    });
}

#[gpui::test]
fn a_refused_profile_write_settles_and_keeps_the_name(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("quake-refused", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    // Another editor adds the same profile before Settings rereads.
    let theirs = format!("{LOGS}\n[quake.profiles.build]\nheight = 0.2\n");
    std::fs::write(file.path(), &theirs).unwrap();
    with_window(&mut harness, |view, window, cx| {
        view.type_profile_name("build", window, cx);
        view.answer("qcreate", window, cx);
    });
    harness.wait_for("the refused create", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, cx| view.quake_message(cx).is_some())
                    .unwrap_or(false)
            })
    });
    harness.cx.run_until_parked();
    assert_eq!(file.text(), theirs);
    with_window(&mut harness, |view, window, cx| {
        assert_eq!(view.quake_prompts(), (true, false));
        assert_eq!(view.profile_name_text(cx), "build");
        assert!(!view.conflicted("quake.profiles.build"));
        assert_eq!(view.profile_names(), ["default", "build", "logs"]);
        // Nothing unresolved keeps the window open.
        view.command(ids::CLOSE_WINDOW, window, cx).unwrap();
    });
    harness.wait_for("the window to close", |cx| settings(cx).is_none());
}

#[gpui::test]
fn deleting_a_profile_reads_the_file_afresh(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("quake-delete-fresh", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    // Another editor changes the profile before Settings rereads.
    std::fs::write(file.path(), LOGS.replace("0.3", "0.4")).unwrap();
    with_view(&mut harness, |view, _| view.pick_profile("logs"));
    confirm_delete(&mut harness);
    saved(&mut harness, &file, |text| !text.contains("logs"));
    assert_eq!(file.text(), "# Mine.\n");
}

#[gpui::test]
fn a_global_key_needs_a_modifier(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("quake-bare-key", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    press(&mut harness, "gadd");
    keystrokes(&mut harness, "a");
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert!(view.global_error("gadd").is_some());
    });
    harness.wait_for("any save", settled);
    assert_eq!(file.text(), LOGS);
}

#[gpui::test]
fn a_create_on_an_invalid_file_keeps_the_name(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("quake-locked", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.type_profile_name("build", window, cx);
    });
    // The file turns invalid while the name is being typed.
    std::fs::write(file.path(), format!("{LOGS}[window\n")).unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the invalid reread", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.read_only())
                    .unwrap_or(false)
            })
    });
    with_window(&mut harness, |view, window, cx| {
        view.answer("qcreate", window, cx);
        assert_eq!(view.quake_prompts(), (true, false));
        assert_eq!(view.profile_name_text(cx), "build");
        assert!(view.quake_message(cx).is_some());
    });
}

#[gpui::test]
fn another_editors_change_closes_an_open_global_menu(cx: &mut TestAppContext) {
    let one = "[[global_keybinding]]\nkey = \"ctrl-alt-shift-y\"\n\
               command = \"toggle_quake\"\n";
    let file = ScratchConfig::new("quake-menu-identity", one);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    press(&mut harness, "gcmd:0");
    with_view(&mut harness, |view, _| assert!(view.global_menu_open()));
    // Another shortcut now sits where the menu's was.
    let two = format!(
        "[[global_keybinding]]\nkey = \"ctrl-alt-shift-u\"\n\
         command = \"hide_quake\"\n\n{one}"
    );
    std::fs::write(file.path(), &two).unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the reread", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| !view.global_menu_open())
                    .unwrap_or(false)
            })
    });
    // Its popover closed too, so no invisible guard stops the next press.
    with_view(&mut harness, |view, _| assert!(!view.menu_open()));
    assert_eq!(file.text(), two);
}

#[gpui::test]
fn picking_a_display_identifier_again_writes_nothing(cx: &mut TestAppContext) {
    let original = "[quake.profiles.default]\ndisplay = \"id:ABC-123\"\n";
    let file = ScratchConfig::new("quake-display-id", original);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    let display = "quake.profiles.default.display";
    with_window(&mut harness, |view, window, cx| {
        view.open_select_for(display, window, cx);
        view.command(ids::MENU_CONFIRM, window, cx).unwrap();
    });
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert_eq!(view.setting_error(display), None);
    });
    assert_eq!(file.text(), original);
}

#[gpui::test]
fn a_refused_create_keeps_a_newer_name_and_drops_its_queued_edits(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-refused-draft", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    // Another editor adds an empty `build` before Settings rereads.
    let theirs = format!("{LOGS}\n[quake.profiles.build]\n");
    std::fs::write(file.path(), &theirs).unwrap();
    let duration = "quake.profiles.build.animation_ms";
    with_window(&mut harness, |view, window, cx| {
        view.type_profile_name("build", window, cx);
        view.answer("qcreate", window, cx);
        // Queued behind the creation, and a newer name in the field.
        view.type_into(duration, "400", window, cx);
        view.enter(duration, cx);
        view.type_profile_name("other", window, cx);
    });
    harness.wait_for("the refused create", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, cx| view.quake_message(cx).is_some())
                    .unwrap_or(false)
            })
    });
    harness.cx.run_until_parked();
    assert_eq!(file.text(), theirs);
    with_window(&mut harness, |view, _, cx| {
        assert_eq!(view.profile_name_text(cx), "other");
        assert!(view.quake_message(cx).unwrap().starts_with("build: "));
        // Leaving the field does not submit the dropped edit again.
        view.blur(duration, cx);
    });
    harness.wait_for("any save", settled);
    harness.cx.run_until_parked();
    assert_eq!(file.text(), theirs);
}

#[gpui::test]
fn a_queued_create_gives_its_name_back_when_the_file_turns_invalid(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-queued-invalid", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.set("font.size", Some(Value::Float(15.0)), cx);
        view.type_profile_name("build", window, cx);
        view.answer("qcreate", window, cx);
    });
    // The file turns invalid before the write ahead of the creation runs.
    std::fs::write(file.path(), format!("{LOGS}[window\n")).unwrap();
    harness.wait_for("the refused writes", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.quake_prompts().0)
                    .unwrap_or(false)
            })
    });
    with_window(&mut harness, |view, _, cx| {
        assert_eq!(view.profile_name_text(cx), "build");
        assert!(view.quake_message(cx).is_some());
    });
}

#[gpui::test]
fn a_global_menu_stays_open_while_another_shortcut_saves(
    cx: &mut TestAppContext,
) {
    let two = "[[global_keybinding]]\nkey = \"ctrl-alt-shift-u\"\n\
               command = \"hide_quake\"\n\n[[global_keybinding]]\n\
               key = \"ctrl-alt-shift-y\"\ncommand = \"toggle_quake\"\n";
    let file = ScratchConfig::new("quake-menu-own-write", two);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    press(&mut harness, "gprofile:1");
    with_view(&mut harness, |view, cx| {
        view.pick_global(0, "command", "show_quake", cx);
    });
    saved(&mut harness, &file, |text| text.contains("show_quake"));
    with_view(&mut harness, |view, _| assert!(view.global_menu_open()));
}

#[gpui::test]
fn a_delete_on_an_invalid_file_says_why(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("quake-locked-delete", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    with_view(&mut harness, |view, _| view.pick_profile("logs"));
    press(&mut harness, "qdel");
    std::fs::write(file.path(), format!("{LOGS}[window\n")).unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the invalid reread", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.read_only())
                    .unwrap_or(false)
            })
    });
    // The alert asks again in its place, saying why.
    keys(&harness, "tab enter");
    with_view(&mut harness, |view, _| {
        assert_eq!(view.dialog_name(), "delete-refused");
    });
    let alert = a11y_node(&mut harness, "settings-delete-profile");
    assert_eq!(
        alert["aria"]["description"],
        "config.toml has an error. Not saved."
    );
}

/// Renames the profile shown to `to` from its Rename profile dialog.
fn rename(harness: &mut DesktopHarness, from: &str, to: &str) {
    with_window(harness, |view, window, cx| {
        renaming(view, from, to, window, cx);
    });
}

/// Starts renaming `from` to `to` from its Rename profile dialog, for a
/// test that acts before the save runs.
fn renaming(
    view: &mut SettingsView,
    from: &str,
    to: &str,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<'_, SettingsView>,
) {
    view.pick_profile(from);
    view.press("qren", window, cx);
    assert_eq!(view.dialog_name(), "rename");
    view.type_dialog_name(to, cx);
    view.answer("qrename", window, cx);
    assert_eq!(view.dialog_name(), "none", "{:?}", view.quake_message(cx));
}

/// Waits for a refused rename's alert.
fn refused_rename(harness: &mut DesktopHarness) {
    harness.wait_for("the refused rename", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| {
                        view.dialog_name() == "rename-refused"
                    })
                    .unwrap_or(false)
            })
    });
}

const SHORTCUTS: &str = "\n[[global_keybinding]]\nkey = \"ctrl-alt-shift-y\"\n\
                         command = \"toggle_quake\"\nargs = { profile = \"logs\" }\n\
                         \n[[global_keybinding]]\nkey = \"ctrl-alt-shift-u\"\n\
                         command = \"toggle_quake\"\n\
                         \n[[keybinding]]\nkey = \"alt-l\"\ncommand = \"show_quake\"\n\
                         args = { profile = \"logs\" } # mine\n\
                         \n[[keybinding]]\nkey = \"alt-p\"\ncommand = \"new_tab\"\n";

#[gpui::test]
fn a_rename_moves_the_table_and_the_shortcuts_naming_it_in_one_save(
    cx: &mut TestAppContext,
) {
    let original = format!("{LOGS}{SHORTCUTS}");
    let file = ScratchConfig::new("quake-rename", &original);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    let before = with_view(&mut harness, |view, _| view.writes_finished());
    rename(&mut harness, "logs", "work");
    // The new name shows at once, while the save runs.
    with_view(&mut harness, |view, _| {
        assert_eq!(view.shown_profile_name(), "work");
        assert_eq!(view.profile_names(), ["default", "work"]);
    });
    saved(&mut harness, &file, |text| text.contains("work"));
    // The table's comments go with the old key.
    assert_eq!(
        file.text(),
        format!(
            "# Mine.\n\n[quake.profiles.work]\nheight = 0.3\n{}",
            SHORTCUTS.replace("\"logs\"", "\"work\"")
        )
    );
    let after = with_view(&mut harness, |view, _| view.writes_finished());
    assert_eq!(after - before, 1, "one save");
    with_view(&mut harness, |view, _| {
        assert_eq!(view.shown_profile_name(), "work");
        assert_eq!(view.dialog_name(), "none");
    });
}

#[gpui::test]
fn a_rename_that_fails_to_save_changes_nothing_and_says_why(
    cx: &mut TestAppContext,
) {
    let original = format!("{LOGS}{SHORTCUTS}");
    let file = ScratchConfig::new("quake-rename-conflict", &original);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    // Another editor changes the profile before Settings rereads.
    let theirs = original.replace("0.3", "0.4");
    std::fs::write(file.path(), &theirs).unwrap();
    rename(&mut harness, "logs", "work");
    harness.wait_for("the refused rename", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| {
                        view.dialog_name() == "rename-refused"
                    })
                    .unwrap_or(false)
            })
    });
    assert_eq!(file.text(), theirs);
    visual(&harness).run_until_parked();
    let alert = a11y_node(&mut harness, "settings-rename-refused");
    let why = alert["aria"]["description"].as_str().unwrap_or_default();
    assert!(why.starts_with("config.toml changed"), "{alert}");
    keys(&harness, "enter");
    with_window(&mut harness, |view, window, cx| {
        assert_eq!(view.dialog_name(), "none");
        assert_eq!(view.focus_name(window, cx), "action:qren");
        assert_eq!(view.shown_profile_name(), "logs");
        assert_eq!(view.profile_names(), ["default", "logs"]);
    });
}

#[gpui::test]
fn a_rename_refuses_a_shortcut_it_cannot_rewrite_and_names_it(
    cx: &mut TestAppContext,
) {
    // An inline array of global shortcuts has no entry to edit in place.
    let original = format!(
        "global_keybinding = [{{ key = \"ctrl-alt-shift-y\", command = \
         \"toggle_quake\", args = {{ profile = \"logs\" }} }}]\n\n{LOGS}"
    );
    let file = ScratchConfig::new("quake-rename-inline", &original);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    rename(&mut harness, "logs", "work");
    harness.wait_for("the refused rename", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| {
                        view.dialog_name() == "rename-refused"
                    })
                    .unwrap_or(false)
            })
    });
    assert_eq!(file.text(), original);
    visual(&harness).run_until_parked();
    let alert = a11y_node(&mut harness, "settings-rename-refused");
    let why = alert["aria"]["description"].as_str().unwrap_or_default();
    assert!(why.contains("global_keybinding"), "{alert}");
    with_view(&mut harness, |view, _| {
        assert_eq!(view.shown_profile_name(), "logs");
    });
}

#[gpui::test]
fn a_queued_rename_carries_the_edit_saved_before_it(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("quake-rename-queued", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    with_view(&mut harness, |view, cx| {
        view.set("quake.profiles.logs.height", Some(Value::Float(0.5)), cx);
    });
    // The rename waits behind the height's write and builds on it.
    rename(&mut harness, "logs", "work");
    saved(&mut harness, &file, |text| text.contains("work"));
    assert_eq!(
        file.text(),
        "# Mine.\n\n[quake.profiles.work]\nheight = 0.5\n"
    );
}

#[gpui::test]
fn a_rename_rewrites_a_keybinding_added_since_settings_read_the_file(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-rename-added", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    // Another editor binds the profile before Settings rereads.
    let binding = "\n[[keybinding]]\nkey = \"alt-l\"\ncommand = \"show_quake\"\n\
                   args = { profile = \"logs\" }\n";
    std::fs::write(file.path(), format!("{LOGS}{binding}")).unwrap();
    rename(&mut harness, "logs", "work");
    saved(&mut harness, &file, |text| text.contains("work"));
    assert_eq!(
        file.text(),
        format!(
            "# Mine.\n\n[quake.profiles.work]\nheight = 0.3\n{}",
            binding.replace("\"logs\"", "\"work\"")
        )
    );
}

#[gpui::test]
fn an_edit_made_under_the_new_name_while_renaming_builds_on_the_rename(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-rename-edit", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    let height = "quake.profiles.work.height";
    with_window(&mut harness, |view, window, cx| {
        renaming(view, "logs", "work", window, cx);
        // The card shows what the rename copies; leaving it writes nothing.
        assert_eq!(view.field_shows(height, window, cx), "0.3");
        view.blur(height, cx);
        view.type_into(height, "0.6", window, cx);
        view.enter(height, cx);
    });
    harness.wait_for("both saves", settled);
    assert_eq!(
        file.text(),
        "# Mine.\n\n[quake.profiles.work]\nheight = 0.6\n"
    );
    with_view(&mut harness, |view, _| {
        assert!(!view.conflicted(height));
        assert_eq!(view.setting_error(height), None);
    });
}

#[gpui::test]
fn a_refused_rename_drops_the_edits_made_under_the_new_name(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-rename-refused-edit", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    // Another editor changes the profile before Settings rereads.
    let theirs = LOGS.replace("0.3", "0.4");
    std::fs::write(file.path(), &theirs).unwrap();
    let height = "quake.profiles.work.height";
    with_window(&mut harness, |view, window, cx| {
        renaming(view, "logs", "work", window, cx);
        // Queued behind the rename; written alone, it would create `work`.
        view.type_into(height, "0.6", window, cx);
        view.enter(height, cx);
    });
    refused_rename(&mut harness);
    assert_eq!(file.text(), theirs);
    visual(&harness).run_until_parked();
    let alert = a11y_node(&mut harness, "settings-rename-refused");
    let why = alert["aria"]["description"].as_str().unwrap_or_default();
    assert!(
        why.ends_with("The changes made to work meanwhile were not saved."),
        "{alert}"
    );
    // Leaving the field does not submit the dropped edit again.
    with_view(&mut harness, |view, cx| view.blur(height, cx));
    harness.cx.run_until_parked();
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), theirs);
}

#[gpui::test]
fn a_refused_rename_leaves_the_card_behind_another_dialog(
    cx: &mut TestAppContext,
) {
    let original = format!("{LOGS}\n[quake.profiles.build]\n");
    let file = ScratchConfig::new("quake-rename-refused-behind", &original);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    let theirs = original.replace("0.3", "0.4");
    std::fs::write(file.path(), &theirs).unwrap();
    let before = with_view(&mut harness, |view, _| view.writes_finished());
    with_window(&mut harness, |view, window, cx| {
        renaming(view, "logs", "work", window, cx);
        // The delete alert for another profile is open when the refusal
        // arrives.
        view.pick_profile("build");
        view.press("qdel", window, cx);
        assert_eq!(view.dialog_name(), "delete");
    });
    harness.wait_for("the refused rename", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.writes_finished() > before)
                    .unwrap_or(false)
            })
    });
    assert_eq!(file.text(), theirs);
    with_view(&mut harness, |view, _| {
        assert_eq!(view.dialog_name(), "delete");
        assert_eq!(view.shown_profile_name(), "build");
    });
}

#[gpui::test]
fn a_queued_rename_is_refused_when_the_file_turns_invalid(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-rename-invalid", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        view.set("font.size", Some(Value::Float(15.0)), cx);
        renaming(view, "logs", "work", window, cx);
    });
    // The file turns invalid before the write ahead of the rename runs.
    let broken = format!("{LOGS}[window\n");
    std::fs::write(file.path(), &broken).unwrap();
    harness.wait_for("the refused writes", settled);
    assert_eq!(file.text(), broken);
    with_view(&mut harness, |view, _| {
        assert_eq!(view.dialog_name(), "rename-refused");
        assert_eq!(view.shown_profile_name(), "logs");
        assert_eq!(view.profile_names(), ["default", "logs"]);
    });
    visual(&harness).run_until_parked();
    let alert = a11y_node(&mut harness, "settings-rename-refused");
    assert_eq!(
        alert["aria"]["description"],
        "config.toml has an error. Not saved."
    );
}

#[gpui::test]
fn a_shortcut_edit_queued_behind_a_rename_follows_the_rewritten_entry(
    cx: &mut TestAppContext,
) {
    let shortcut = "\n[[global_keybinding]]\nkey = \"ctrl-alt-shift-y\"\n\
                    command = \"toggle_quake\"\nargs = { profile = \"logs\" }\n";
    let file = ScratchConfig::new(
        "quake-rename-queued-key",
        &format!("{LOGS}{shortcut}"),
    );
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    with_window(&mut harness, |view, window, cx| {
        renaming(view, "logs", "work", window, cx);
        // Built against the entry naming `logs`, which the rename rewrites.
        view.pick_global(0, "command", "show_quake", cx);
    });
    harness.wait_for("both saves", settled);
    assert_eq!(
        file.text(),
        format!(
            "# Mine.\n\n[quake.profiles.work]\nheight = 0.3\n{}",
            shortcut
                .replace("\"logs\"", "\"work\"")
                .replace("toggle_quake", "show_quake")
        )
    );
}

/// Records a window serving `profile`, as a summon records it. The test
/// harness opens no native Quake window, so reconcile cannot detach it;
/// its association shows what reconcile decided.
fn serving(harness: &mut DesktopHarness, profile: &str) -> gpui::WindowId {
    use crate::desktop::windows::model::{QuakeRecord, WindowLayout};
    let id = gpui::WindowId::from(9_999);
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().windows.open(
            id,
            Some(QuakeRecord {
                profile: profile.to_owned(),
                visible: true,
            }),
            WindowLayout {
                bounds: gpui::WindowBounds::Windowed(gpui::Bounds::default()),
                sidebar_width: gpui::px(180.0),
            },
        );
    });
    id
}

/// Waits until no reload runs and the applied configuration has `profile`.
fn applied(harness: &mut DesktopHarness, profile: &str) {
    harness.wait_for("the reload", |cx| {
        let desktop = cx.global::<Desktop>();
        !desktop.reloads.reloading()
            && desktop.config.quake.profiles.contains_key(profile)
    });
}

#[gpui::test]
fn a_rename_keeps_the_profiles_open_window_under_the_new_name(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-rename-window", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    let id = serving(&mut harness, "logs");
    rename(&mut harness, "logs", "work");
    saved(&mut harness, &file, |text| text.contains("work"));
    applied(&mut harness, "work");
    harness.cx.update(|cx| {
        let windows = &cx.global::<Desktop>().windows;
        assert_eq!(windows.quake_window("work"), Some(id));
        assert_eq!(windows.quake_window("logs"), None);
    });
}

#[gpui::test]
fn a_reload_read_before_a_second_rename_keeps_the_window(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-rename-held-reload", LOGS);
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.general.watch_config = false;
    });
    open_quake(&mut harness, &file);
    let id = serving(&mut harness, "logs");
    rename(&mut harness, "logs", "work");
    saved(&mut harness, &file, |text| text.contains("work"));
    applied(&mut harness, "work");
    // A reload reads the file naming `work`, and is held there.
    let (read, held) = harness.cx.update(|cx| {
        let desktop = cx.global_mut::<Desktop>();
        assert!(desktop.reloads.manual());
        let read = desktop.windows.quake_renames_noted();
        (read, config::reload(&desktop.config_path))
    });
    // The second rename lands; its reload waits for the held one.
    rename(&mut harness, "work", "build");
    saved(&mut harness, &file, |text| text.contains("build"));
    harness.cx.update(|cx| {
        finish_reload(cx, ReloadKind::Manual, &file.path(), held, read);
        // The held reload applies `work`, which the window still serves.
        let windows = &cx.global::<Desktop>().windows;
        assert_eq!(windows.quake_window("work"), Some(id));
    });
    applied(&mut harness, "build");
    harness.cx.update(|cx| {
        let windows = &cx.global::<Desktop>().windows;
        assert_eq!(windows.quake_window("build"), Some(id));
    });
}

#[gpui::test]
fn a_reload_reading_a_rename_before_its_save_settles_keeps_the_window(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-rename-mid-save", LOGS);
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.general.watch_config = false;
    });
    open_quake(&mut harness, &file);
    let id = serving(&mut harness, "logs");
    let release = with_view(&mut harness, |view, _| view.hold_batch());
    rename(&mut harness, "logs", "work");
    // The save replaced the file and is held before it settles, while a
    // reload reads the file and applies it.
    harness.wait_for("the write", |_| file.text().contains("work"));
    harness.cx.update(|cx| {
        let _ = reload(cx);
    });
    applied(&mut harness, "work");
    harness.cx.update(|cx| {
        let windows = &cx.global::<Desktop>().windows;
        assert_eq!(windows.quake_window("work"), Some(id));
    });
    drop(release);
    saved(&mut harness, &file, |text| text.contains("work"));
    applied(&mut harness, "work");
    harness.cx.update(|cx| {
        let windows = &cx.global::<Desktop>().windows;
        assert_eq!(windows.quake_window("work"), Some(id));
    });
}

#[gpui::test]
fn a_rename_to_a_name_whose_window_is_still_closing_is_refused(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-rename-closing", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    // A removed profile's window keeps `build` until its detach ends.
    serving(&mut harness, "build");
    with_window(&mut harness, |view, window, cx| {
        view.pick_profile("logs");
        view.press("qren", window, cx);
        view.type_dialog_name("build", cx);
        view.answer("qrename", window, cx);
        assert_eq!(view.dialog_name(), "rename");
        assert_eq!(
            view.quake_message(cx).as_deref(),
            Some("A window for build is still closing; try again in a moment.")
        );
    });
    harness.cx.run_until_parked();
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), LOGS);
}

#[gpui::test]
fn a_draft_under_the_new_name_outlasts_a_save_ahead_of_the_rename(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-rename-draft", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    let height = "quake.profiles.work.height";
    with_window(&mut harness, |view, window, cx| {
        view.set("font.size", Some(Value::Float(15.0)), cx);
        renaming(view, "logs", "work", window, cx);
        assert_eq!(view.field_shows(height, window, cx), "0.3");
        // Typed over the copied value, and not yet saved.
        view.type_into(height, "0.6", window, cx);
    });
    harness.wait_for("both saves", |cx| {
        settled(cx) && file.text().contains("work")
    });
    with_view(&mut harness, |view, cx| {
        assert!(!view.conflicted(height));
        assert_eq!(view.field_value(height, cx), "0.6");
        view.blur(height, cx);
    });
    saved(&mut harness, &file, |text| text.contains("0.6"));
    assert!(
        file.text()
            .contains("[quake.profiles.work]\nheight = 0.6\n"),
        "{}",
        file.text()
    );
    with_view(&mut harness, |view, _| assert!(!view.conflicted(height)));
}

#[cfg(unix)]
#[gpui::test]
fn an_edit_queued_behind_a_rename_that_is_not_durable_builds_on_it(
    cx: &mut TestAppContext,
) {
    use std::os::unix::fs::PermissionsExt;
    let file = ScratchConfig::new("quake-rename-not-durable", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    // Without read permission the directory cannot be opened to sync the
    // rename, so each write lands but reports that it is not durable.
    let mode = |bits| std::fs::Permissions::from_mode(bits);
    std::fs::set_permissions(&file.0, mode(0o300)).unwrap();
    if std::fs::read_dir(&file.0).is_ok() {
        // Permissions do not bind this user, such as root.
        std::fs::set_permissions(&file.0, mode(0o700)).unwrap();
        return;
    }
    let height = "quake.profiles.work.height";
    with_window(&mut harness, |view, window, cx| {
        renaming(view, "logs", "work", window, cx);
        view.type_into(height, "0.6", window, cx);
        view.enter(height, cx);
    });
    harness.wait_for("both saves", settled);
    std::fs::set_permissions(&file.0, mode(0o700)).unwrap();
    with_view(&mut harness, |view, _| assert!(!view.conflicted(height)));
    assert_eq!(
        file.text(),
        "# Mine.\n\n[quake.profiles.work]\nheight = 0.6\n"
    );
}

#[gpui::test]
fn a_refused_rename_behind_another_dialog_leaves_the_new_name_and_says_why(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-rename-refused-shown", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    let theirs = LOGS.replace("0.3", "0.4");
    std::fs::write(file.path(), &theirs).unwrap();
    let height = "quake.profiles.work.height";
    let before = with_view(&mut harness, |view, _| view.writes_finished());
    with_window(&mut harness, |view, window, cx| {
        renaming(view, "logs", "work", window, cx);
        assert_eq!(view.shown_profile_name(), "work");
        view.type_into(height, "0.6", window, cx);
        view.enter(height, cx);
        // New profile is open over `work`'s card when the refusal arrives.
        view.press("qnew", window, cx);
        assert_eq!(view.dialog_name(), "new-profile");
    });
    harness.wait_for("the refused rename", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.writes_finished() > before)
                    .unwrap_or(false)
            })
    });
    assert_eq!(file.text(), theirs);
    with_view(&mut harness, |view, _| {
        assert_eq!(view.dialog_name(), "new-profile");
        assert_eq!(view.shown_profile_name(), "logs");
        let status = view.status_text();
        assert!(
            status.ends_with(
                "The changes made to work meanwhile were not saved."
            ),
            "{status}"
        );
    });
}
