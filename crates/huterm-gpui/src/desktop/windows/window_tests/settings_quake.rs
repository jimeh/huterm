//! The Settings window's Quake windows section through the desktop
//! harness: profiles and global shortcuts.

use toml::Value;

use super::super::settings_window::SettingsView;

use super::settings::{
    ScratchConfig, open_settings, settings, settled, with_view, with_window,
};
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
            view.press("qcreate", window, cx);
            assert!(view.quake_message().is_some(), "{name:?} was accepted");
        });
    }
    harness.cx.run_until_parked();
    assert_eq!(file.text(), LOGS);
    with_window(&mut harness, |view, window, cx| {
        view.type_profile_name("build-1", window, cx);
        view.press("qcreate", window, cx);
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
    press(&mut harness, "qdel");
    press(&mut harness, "qdelyes");
    with_view(&mut harness, |view, _| {
        assert!(
            view.quake_message().is_some_and(|message| {
                message.contains("uses this profile")
            })
        );
    });
    assert_eq!(file.text(), original);
    // With its shortcut gone, the profile goes.
    press(&mut harness, "gdel:0");
    saved(&mut harness, &file, |text| {
        !text.contains("global_keybinding")
    });
    with_view(&mut harness, |view, _| view.pick_profile("logs"));
    press(&mut harness, "qdel");
    press(&mut harness, "qdelyes");
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
        view.press("qcreate", window, cx);
    });
    harness.wait_for("the refused create", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.quake_message().is_some())
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
    press(&mut harness, "qdel");
    press(&mut harness, "qdelyes");
    saved(&mut harness, &file, |text| !text.contains("logs"));
    assert_eq!(file.text(), "# Mine.\n");
}

#[gpui::test]
fn escape_closes_the_delete_prompt_only_from_its_buttons(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("quake-escape", LOGS);
    let mut harness = DesktopHarness::new(cx);
    open_quake(&mut harness, &file);
    with_view(&mut harness, |view, _| view.pick_profile("logs"));
    press(&mut harness, "qdel");
    let height = "quake.profiles.logs.height";
    with_window(&mut harness, |view, window, cx| {
        view.focus_field(height, window, cx);
        view.type_into(height, "0.9", window, cx);
    });
    harness.cx.run_until_parked();
    // Escape in the field puts its value back and leaves the prompt.
    keystrokes(&mut harness, "escape");
    harness.cx.run_until_parked();
    with_window(&mut harness, |view, _, cx| {
        assert_eq!(view.field_value(height, cx), "0.3");
        assert_eq!(view.quake_prompts(), (false, true));
    });
    assert_eq!(file.text(), LOGS);
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
        view.press("qcreate", window, cx);
        assert_eq!(view.quake_prompts(), (true, false));
        assert_eq!(view.profile_name_text(cx), "build");
        assert!(view.quake_message().is_some());
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
    let height = "quake.profiles.build.height";
    with_window(&mut harness, |view, window, cx| {
        view.type_profile_name("build", window, cx);
        view.press("qcreate", window, cx);
        // Queued behind the creation, and a newer name in the field.
        view.type_into(height, "0.4", window, cx);
        view.enter(height, cx);
        view.type_profile_name("other", window, cx);
    });
    harness.wait_for("the refused create", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.quake_message().is_some())
                    .unwrap_or(false)
            })
    });
    harness.cx.run_until_parked();
    assert_eq!(file.text(), theirs);
    with_window(&mut harness, |view, _, cx| {
        assert_eq!(view.profile_name_text(cx), "other");
        assert!(view.quake_message().unwrap().starts_with("build: "));
        // Leaving the field does not submit the dropped edit again.
        view.blur(height, cx);
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
        view.press("qcreate", window, cx);
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
        assert!(view.quake_message().is_some());
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
    press(&mut harness, "qdelyes");
    with_view(&mut harness, |view, _| {
        assert!(view.quake_message().is_some());
    });
}
