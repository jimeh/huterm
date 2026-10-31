//! Tier 2 tests for the Settings window: opening and closing it, saving
//! and resetting through the patch service, conflicts, and invalid files.

use std::path::{Path, PathBuf};

use super::super::settings_window::SettingsView;
use super::*;

/// A configuration file in its own temporary directory, removed on drop
/// even when an assertion fails first.
pub(super) struct ScratchConfig(pub(super) PathBuf);

impl ScratchConfig {
    pub(super) fn new(name: &str, text: &str) -> Self {
        let directory = std::env::temp_dir()
            .join(format!("huterm-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("config.toml"), text).unwrap();
        Self(directory)
    }

    pub(super) fn path(&self) -> PathBuf {
        self.0.join("config.toml")
    }

    pub(super) fn text(&self) -> String {
        std::fs::read_to_string(self.path()).unwrap()
    }
}

impl Drop for ScratchConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) fn settings(cx: &App) -> Option<gpui::WindowHandle<SettingsView>> {
    cx.global::<Desktop>().settings
}

/// Points the desktop at `path`, opens Settings as its shortcut does, and
/// waits for its read.
pub(super) fn open_settings(harness: &mut DesktopHarness, path: &Path) {
    let path = path.to_owned();
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().config_path = path;
        let open = CommandInvocation::new(ids::OPEN_SETTINGS, Vec::new());
        Desktop::invoke(cx, &open, None).unwrap();
    });
    harness.wait_for("Settings to read the file", settled);
}

pub(super) fn settled(cx: &mut App) -> bool {
    settings(cx).is_some_and(|handle| {
        handle
            .read_with(cx, |view, _| view.settled())
            .unwrap_or(false)
    })
}

pub(super) fn with_view<R>(
    harness: &mut DesktopHarness,
    update: impl FnOnce(
        &mut SettingsView,
        &mut gpui::Context<'_, SettingsView>,
    ) -> R,
) -> R {
    harness.cx.update(|cx| {
        settings(cx)
            .expect("Settings is open")
            .update(cx, |view, _, cx| update(view, cx))
            .unwrap()
    })
}

#[gpui::test]
fn settings_opens_once_and_closing_it_forgets_the_window(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("open", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let first = harness.cx.update(|cx| settings(cx)).unwrap();
    let windows = harness.cx.update(|cx| cx.windows().len());
    harness.cx.update(|cx| {
        let open = CommandInvocation::new(ids::OPEN_SETTINGS, Vec::new());
        Desktop::invoke(cx, &open, None).unwrap();
    });
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert_eq!(settings(cx), Some(first));
        assert_eq!(cx.windows().len(), windows);
        // Settings is no terminal window, so it never hosts Quit.
        assert!(
            cx.global::<Desktop>()
                .windows
                .record(first.window_id())
                .is_none()
        );
    });
    harness.cx.update(|cx| {
        first
            .update(cx, |view, window, cx| {
                assert_eq!(view.status_text(), "Watching config.toml");
                // Terminal-window commands are not available here.
                assert!(view.command(ids::NEW_TAB, window, cx).is_err());
                view.command(ids::CLOSE_PANE, window, cx)
            })
            .unwrap()
            .unwrap();
    });
    harness.wait_for("Settings to close", |cx| settings(cx).is_none());
}

#[expect(
    clippy::float_cmp,
    reason = "the font size parses from an exact literal"
)]
#[gpui::test]
fn a_change_and_a_reset_write_only_their_key_and_apply(
    cx: &mut TestAppContext,
) {
    let original = "# Shared through dotfiles.\n[font]\nsize = 13.0 # small\n\n[tabs]\nstyle = \"strip\"\n";
    let file = ScratchConfig::new("write", original);
    let mut harness = DesktopHarness::with_config(cx, |config| {
        // Saving must apply the change without the watcher.
        config.general.watch_config = false;
    });
    open_settings(&mut harness, &file.path());
    with_view(&mut harness, |view, cx| {
        view.set("tabs.always_show", Some(toml::Value::Boolean(true)), cx);
        view.set("font.size", Some(toml::Value::Float(15.0)), cx);
    });
    harness.wait_for("both saves and the reload", |cx| {
        settled(cx) && cx.global::<Desktop>().config.font.size == 15.0
    });
    assert_eq!(
        file.text(),
        "# Shared through dotfiles.\n[font]\nsize = 15.0 # small\n\n[tabs]\nstyle = \"strip\"\nalways_show = true\n"
    );
    harness.cx.update(|cx| {
        assert!(cx.global::<Desktop>().config.tabs.always_show);
    });
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.status_text(), "Saved");
        view.set("tabs.always_show", None, cx);
        view.set("font.size", None, cx);
    });
    harness.wait_for("both resets", |cx| {
        settled(cx) && !cx.global::<Desktop>().config.tabs.always_show
    });
    assert_eq!(
        file.text(),
        "# Shared through dotfiles.\n[font]\n\n[tabs]\nstyle = \"strip\"\n"
    );
}

#[gpui::test]
fn the_watchers_reload_of_another_editors_change_reports_itself_after_a_save(
    cx: &mut TestAppContext,
) {
    let file =
        ScratchConfig::new("own-save-then-edit", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_view(&mut harness, |view, cx| {
        view.set("font.size", Some(toml::Value::Float(15.0)), cx);
    });
    harness
        .wait_for("the save", |cx| settled(cx) && file.text().contains("15.0"));
    harness.cx.run_until_parked();
    // Another editor's change reaches a Settings read before the watcher's
    // reload applies it.
    std::fs::write(file.path(), "[font]\nsize = 17.0\n").unwrap();
    with_view(&mut harness, SettingsView::reread);
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert_eq!(view.status_text(), "Saved");
    });
    harness.cx.update(|cx| {
        super::handle_watch_report(cx, super::config_changed());
    });
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert_eq!(view.status_text(), "Reloaded");
    });
}

#[gpui::test]
fn the_windows_own_save_is_not_an_edit_made_elsewhere(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("own-save", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.general.watch_config = false;
    });
    open_settings(&mut harness, &file.path());
    // Coming forward while a save is in flight reads the file the save
    // writes; that is not a change made elsewhere.
    with_view(&mut harness, |view, cx| {
        view.set("font.size", Some(toml::Value::Float(15.0)), cx);
        view.look_at_disk(cx);
    });
    harness
        .wait_for("the save", |cx| settled(cx) && file.text().contains("15.0"));
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert_eq!(view.status_text(), "Saved");
    });
}

#[gpui::test]
fn a_conflict_keeps_the_entered_value_until_the_user_picks(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("conflict", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    // Another editor changes the same setting after Settings read it.
    std::fs::write(file.path(), "[font]\nsize = 20.0\n").unwrap();
    with_view(&mut harness, |view, cx| {
        view.set("font.size", Some(toml::Value::Float(15.0)), cx);
    });
    harness.wait_for("the conflict", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.conflicted("font.size"))
                    .unwrap_or(false)
            })
    });
    assert_eq!(file.text(), "[font]\nsize = 20.0\n");
    with_view(&mut harness, |view, cx| view.use_mine("font.size", cx));
    harness.wait_for("the entered value to save", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| !view.conflicted("font.size"))
                    .unwrap_or(false)
            })
    });
    assert_eq!(file.text(), "[font]\nsize = 15.0\n");
}

#[gpui::test]
fn an_invalid_file_makes_settings_read_only(cx: &mut TestAppContext) {
    let broken = "[window]\npadding_x 8.0\n";
    let file = ScratchConfig::new("invalid", broken);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_view(&mut harness, |view, cx| {
        assert!(view.read_only());
        assert_eq!(view.status_text(), "config.toml has an error");
        view.set("tabs.always_show", Some(toml::Value::Boolean(true)), cx);
    });
    harness.cx.run_until_parked();
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), broken);
}

/// Runs `update` on the open Settings view with its window.
pub(super) fn with_window<R>(
    harness: &mut DesktopHarness,
    update: impl FnOnce(
        &mut SettingsView,
        &mut gpui::Window,
        &mut gpui::Context<'_, SettingsView>,
    ) -> R,
) -> R {
    harness.cx.update(|cx| {
        settings(cx)
            .expect("Settings is open")
            .update(cx, |view, window, cx| update(view, window, cx))
            .unwrap()
    })
}

pub(super) fn conflicted(cx: &mut App, path: &str) -> bool {
    settings(cx).is_some_and(|handle| {
        handle
            .read_with(cx, |view, _| view.conflicted(path))
            .unwrap_or(false)
    })
}

#[gpui::test]
fn use_file_keeps_the_file_even_when_focus_leaves_the_field(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("use-file", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    std::fs::write(file.path(), "[font]\nsize = 20.0\n").unwrap();
    with_window(&mut harness, |view, window, cx| {
        view.type_into("font.size", "15", window, cx);
        view.enter("font.size", cx);
    });
    harness.wait_for("the conflict", |cx| {
        settled(cx) && conflicted(cx, "font.size")
    });
    // Pressing Use file first moves focus out of the field.
    with_window(&mut harness, |view, _, cx| {
        view.blur("font.size", cx);
        view.use_file("font.size", cx);
    });
    harness.wait_for("the file's value", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, cx| view.field_value("font.size", cx))
                    .is_ok_and(|text| text == "20")
            })
    });
    assert_eq!(file.text(), "[font]\nsize = 20.0\n");
    // Settling the conflict clears its "Not saved" report.
    with_view(&mut harness, |view, _| {
        assert_eq!(view.status_text(), "Watching config.toml");
    });
}

#[gpui::test]
fn a_draft_that_does_not_parse_keeps_its_conflict_and_later_edits_still_conflict(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("conflict-parse", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.type_into("font.size", "21", window, cx);
    });
    std::fs::write(file.path(), "[font]\nsize = 22.0\n").unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the draft's conflict", |cx| {
        settled(cx) && conflicted(cx, "font.size")
    });
    // Saving text that does not parse keeps the conflict to settle.
    with_window(&mut harness, |view, window, cx| {
        view.type_into("font.size", "abc", window, cx);
        view.enter("font.size", cx);
        assert!(view.conflicted("font.size"));
    });
    // Another editor changes it again; a later change made here must
    // conflict with that, not overwrite it from a fresh read.
    with_view(&mut harness, |view, cx| view.use_file("font.size", cx));
    std::fs::write(file.path(), "[font]\nsize = 23.0\n").unwrap();
    with_view(&mut harness, |view, cx| {
        view.set("font.size", Some(toml::Value::Float(15.0)), cx);
    });
    harness.wait_for("the refused write", |cx| {
        settled(cx) && conflicted(cx, "font.size")
    });
    assert_eq!(file.text(), "[font]\nsize = 23.0\n");
}

#[gpui::test]
fn a_reload_keeps_a_draft_and_reports_the_file_change(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("draft", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.type_into("font.size", "21", window, cx);
    });
    std::fs::write(file.path(), "[font]\nsize = 22.0\n").unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the draft's conflict", |cx| {
        settled(cx) && conflicted(cx, "font.size")
    });
    with_window(&mut harness, |view, _, cx| {
        assert_eq!(view.field_value("font.size", cx), "21");
        // Saving the draft again keeps it over the file's change.
        view.enter("font.size", cx);
    });
    harness.wait_for("the draft to save", |cx| {
        settled(cx) && !conflicted(cx, "font.size")
    });
    assert_eq!(file.text(), "[font]\nsize = 21.0\n");
}

#[gpui::test]
fn a_conflict_waits_while_the_file_is_invalid(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("invalid-conflict", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.type_into("font.size", "21", window, cx);
    });
    std::fs::write(file.path(), "[font]\nsize = 22.0\n").unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the draft's conflict", |cx| {
        settled(cx) && conflicted(cx, "font.size")
    });
    let broken = "[font]\nsize = 22.0\n[window\n";
    std::fs::write(file.path(), broken).unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the invalid file", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.read_only())
                    .unwrap_or(false)
            })
    });
    // Neither choice, nor leaving the field, can save while the file is
    // invalid, so the conflict and the draft stay.
    with_view(&mut harness, |view, cx| {
        view.use_mine("font.size", cx);
        view.use_file("font.size", cx);
        view.blur("font.size", cx);
    });
    harness.cx.run_until_parked();
    assert!(harness.cx.update(|cx| conflicted(cx, "font.size")));
    assert_eq!(file.text(), broken);
    std::fs::write(file.path(), "[font]\nsize = 22.0\n").unwrap();
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the fixed file", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| !view.read_only())
                    .unwrap_or(false)
            })
    });
    with_view(&mut harness, |view, cx| view.use_mine("font.size", cx));
    harness.wait_for("the draft to save", |cx| {
        settled(cx) && !conflicted(cx, "font.size")
    });
    assert_eq!(file.text(), "[font]\nsize = 21.0\n");
}

#[gpui::test]
fn opening_settings_creates_the_file_from_the_template(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("template", "");
    std::fs::remove_file(file.path()).unwrap();
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    assert!(file.text().starts_with("#:schema "), "{}", file.text());
}

#[gpui::test]
fn an_edit_before_the_first_read_writes_nothing(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("early-edit", "");
    std::fs::remove_file(file.path()).unwrap();
    let mut harness = DesktopHarness::new(cx);
    let path = file.path();
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().config_path = path;
        let open = CommandInvocation::new(ids::OPEN_SETTINGS, Vec::new());
        Desktop::invoke(cx, &open, None).unwrap();
    });
    // The window exists, but its first read has not run.
    with_view(&mut harness, |view, cx| {
        assert!(view.read_only());
        view.set("font.size", Some(toml::Value::Float(15.0)), cx);
    });
    harness.wait_for("Settings to read the file", settled);
    let text = file.text();
    assert!(text.starts_with("#:schema "), "{text}");
    assert!(!text.contains("15.0"), "{text}");
}

#[cfg(unix)]
#[gpui::test]
fn settings_reports_a_file_it_cannot_create_or_open(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    let file = ScratchConfig::new("uncreatable", "");
    std::fs::remove_file(file.path()).unwrap();
    let mode = |bits| std::fs::Permissions::from_mode(bits);
    std::fs::set_permissions(&file.0, mode(0o500)).unwrap();
    if std::fs::write(file.0.join("probe"), "").is_ok() {
        // Permissions do not bind this user, such as root.
        std::fs::set_permissions(&file.0, mode(0o700)).unwrap();
        return;
    }
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let status = |harness: &mut DesktopHarness| {
        with_view(harness, |view, _| view.notice_text())
    };
    assert!(status(&mut harness).starts_with("Cannot create "));
    // Open config.toml in the footer reports its failure here too.
    harness.cx.update(|cx| {
        settings(cx)
            .unwrap()
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        super::super::open_config_file(cx);
    });
    let text = status(&mut harness);
    assert!(text.starts_with("Cannot open "), "{text}");
    std::fs::set_permissions(&file.0, mode(0o700)).unwrap();
}

#[gpui::test]
fn leaving_an_untouched_field_keeps_the_file(cx: &mut TestAppContext) {
    let original = "[font]\nsize = 13\n";
    let file = ScratchConfig::new("untouched-blur", original);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        // The field shows "13" for the file's integer; leave it unchanged.
        view.type_into("font.size", "13", window, cx);
        view.blur("font.size", cx);
    });
    harness.cx.run_until_parked();
    assert!(harness.cx.update(settled));
    assert_eq!(file.text(), original);
}

#[gpui::test]
fn the_size_limit_accepts_a_size_and_a_byte_count(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("size", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.type_into("terminal.scrollback_bytes", "64MiB", window, cx);
        view.enter("terminal.scrollback_bytes", cx);
    });
    harness.wait_for("the size to save", |cx| {
        settled(cx) && file.text().contains("scrollback_bytes")
    });
    assert_eq!(file.text(), "[terminal]\nscrollback_bytes = \"64MiB\"\n");
    with_window(&mut harness, |view, window, cx| {
        view.type_into("terminal.scrollback_bytes", "67108864", window, cx);
        view.enter("terminal.scrollback_bytes", cx);
    });
    harness.wait_for("the byte count to save", |cx| {
        settled(cx) && file.text().contains("67108864")
    });
    assert_eq!(file.text(), "[terminal]\nscrollback_bytes = 67108864\n");
}

#[gpui::test]
fn reset_on_the_theme_clears_only_its_name(cx: &mut TestAppContext) {
    let original = "theme = { name = \"nord\", background = \"#000000\" }\n\n[[keybinding]]\nkey = \"cmd-k\"\ncommand = \"copy\"\n";
    let file = ScratchConfig::new("reset-theme", original);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_view(&mut harness, |view, cx| {
        // Keybindings have no Reset until their editor exists.
        view.set("keybinding", None, cx);
        view.set("theme", None, cx);
    });
    harness.wait_for("the theme name to reset", |cx| {
        settled(cx) && !file.text().contains("nord")
    });
    assert_eq!(
        file.text(),
        "theme = { background = \"#000000\" }\n\n[[keybinding]]\nkey = \"cmd-k\"\ncommand = \"copy\"\n"
    );
}

#[gpui::test]
fn a_queued_edit_conflicts_with_an_external_change_to_its_setting(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("queued", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    // Another editor changes the font size after Settings read the file.
    std::fs::write(file.path(), "[font]\nsize = 20.0\n").unwrap();
    with_view(&mut harness, |view, cx| {
        // The first write rebases over the external change; the second,
        // queued behind it, must not overwrite it.
        view.set("tabs.always_show", Some(toml::Value::Boolean(true)), cx);
        view.set("font.size", Some(toml::Value::Float(15.0)), cx);
    });
    harness.wait_for("the queued conflict", |cx| {
        settled(cx) && conflicted(cx, "font.size")
    });
    assert_eq!(
        file.text(),
        "[font]\nsize = 20.0\n\n[tabs]\nalways_show = true\n"
    );
}

#[gpui::test]
fn repeated_steps_build_on_each_other_before_they_save(
    cx: &mut TestAppContext,
) {
    // The file writes an integer for a float setting.
    let file = ScratchConfig::new("steps", "[font]\nsize = 13\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_view(&mut harness, |view, cx| {
        view.step_once("font.size", true, cx);
        view.step_once("font.size", true, cx);
        // The second step starts from the first's pending choice, not the
        // shown default.
        view.step_once("tabs.width", false, cx);
        view.step_once("tabs.width", true, cx);
    });
    harness.wait_for("both settings to save", |cx| {
        settled(cx) && file.text().contains("15.0")
    });
    assert_eq!(
        file.text(),
        "[font]\nsize = 15.0\n\n[tabs]\nwidth = \"fit\"\n"
    );
}

#[gpui::test]
fn a_saved_field_shows_its_value_without_a_conflict(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("display", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.type_into("font.size", "15.0", window, cx);
        view.enter("font.size", cx);
    });
    harness
        .wait_for("the save", |cx| settled(cx) && file.text().contains("15.0"));
    with_window(&mut harness, |view, _, cx| {
        assert!(!view.conflicted("font.size"));
        assert_eq!(view.field_value("font.size", cx), "15");
    });
    // A reload after the save leaves it alone too.
    with_view(&mut harness, SettingsView::follow_reload);
    harness.wait_for("the reload's read", settled);
    harness
        .cx
        .update(|cx| assert!(!conflicted(cx, "font.size")));
}

#[cfg(unix)]
#[gpui::test]
fn a_save_that_is_not_durable_shows_its_value_without_a_conflict(
    cx: &mut TestAppContext,
) {
    use std::os::unix::fs::PermissionsExt;
    let file = ScratchConfig::new("not-durable", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    // Without read permission the directory cannot be opened to sync the
    // rename, so the write lands but reports that it is not durable.
    let mode = |bits| std::fs::Permissions::from_mode(bits);
    std::fs::set_permissions(&file.0, mode(0o300)).unwrap();
    if std::fs::read_dir(&file.0).is_ok() {
        // Permissions do not bind this user, such as root.
        std::fs::set_permissions(&file.0, mode(0o700)).unwrap();
        return;
    }
    with_window(&mut harness, |view, window, cx| {
        view.type_into("font.size", "15.0", window, cx);
        view.enter("font.size", cx);
    });
    harness
        .wait_for("the save", |cx| settled(cx) && file.text().contains("15.0"));
    std::fs::set_permissions(&file.0, mode(0o700)).unwrap();
    with_window(&mut harness, |view, _, cx| {
        assert_eq!(view.status_text(), "Saved with a warning");
        assert!(!view.conflicted("font.size"));
        assert_eq!(view.field_value("font.size", cx), "15");
    });
}

#[gpui::test]
fn escape_in_a_field_puts_back_its_value(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("escape", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.type_into("font.size", "99", window, cx);
        view.escape("font.size", cx);
        assert_eq!(view.field_value("font.size", cx), "13");
        // Leaving the field afterwards saves nothing.
        view.blur("font.size", cx);
    });
    harness.wait_for("Settings to settle", settled);
    assert_eq!(file.text(), "[font]\nsize = 13.0\n");
}

#[gpui::test]
fn closing_settings_waits_for_its_saves_and_applies_them(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("close", "");
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.general.watch_config = false;
    });
    open_settings(&mut harness, &file.path());
    harness.cx.update(|cx| {
        settings(cx)
            .unwrap()
            .update(cx, |view, window, cx| {
                view.set(
                    "tabs.always_show",
                    Some(toml::Value::Boolean(true)),
                    cx,
                );
                view.set("font.size", Some(toml::Value::Float(16.0)), cx);
                view.command(ids::CLOSE_PANE, window, cx)
            })
            .unwrap()
            .unwrap();
    });
    harness.wait_for("Settings to close after saving", |cx| {
        settings(cx).is_none() && cx.global::<Desktop>().config.font.size > 15.0
    });
    assert_eq!(
        file.text(),
        "[tabs]\nalways_show = true\n\n[font]\nsize = 16.0\n"
    );
}

#[gpui::test]
fn an_invalid_file_settles_every_pending_save(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("settle", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let broken = "[font]\nsize 13.0\n";
    std::fs::write(file.path(), broken).unwrap();
    with_view(&mut harness, |view, cx| {
        view.set("tabs.always_show", Some(toml::Value::Boolean(true)), cx);
        view.set("font.size", Some(toml::Value::Float(15.0)), cx);
    });
    harness.wait_for("the queue to settle", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| view.read_only())
                    .unwrap_or(false)
            })
    });
    assert_eq!(file.text(), broken);
}

#[gpui::test]
fn a_semantically_invalid_file_opens_read_only(cx: &mut TestAppContext) {
    let invalid = "[tabs]\nmin_width = 300.0\nmax_width = 100.0\n";
    let file = ScratchConfig::new("semantic", invalid);
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_view(&mut harness, |view, _| assert!(view.read_only()));
}

#[gpui::test]
fn a_reload_refused_outside_the_file_keeps_settings_editable(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("not-applied", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    // A save first, so the file stays as this window last wrote it.
    with_view(&mut harness, |view, cx| {
        view.set("font.size", Some(toml::Value::Float(15.0)), cx);
    });
    harness
        .wait_for("the save", |cx| settled(cx) && file.text().contains("15.0"));
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, cx| {
        view.follow_failed_reload("font \"Nope\" is not installed", cx);
    });
    // The reload's read decides whether the file is to blame.
    harness.wait_for("the reload's read", |cx| {
        settings(cx).is_some_and(|handle| {
            handle
                .read_with(cx, |view, _| {
                    view.status_text().starts_with("Not applied")
                })
                .unwrap_or(false)
        })
    });
    with_view(&mut harness, |view, _| {
        assert!(!view.read_only());
        assert_eq!(view.status_text(), "Not applied");
        assert_eq!(view.notice_text(), "font \"Nope\" is not installed");
    });
    // The font is installed, and the watcher's reload of the same file
    // applies it.
    harness.cx.update(|cx| {
        super::handle_watch_report(cx, super::config_changed());
    });
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert_eq!(view.status_text(), "Reloaded");
    });
}

#[gpui::test]
fn a_refused_global_shortcut_warns_in_place_of_the_saves_report(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("applied-warning", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_view(&mut harness, |view, cx| {
        view.set("font.size", Some(toml::Value::Float(15.0)), cx);
    });
    harness
        .wait_for("the save", |cx| settled(cx) && file.text().contains("15.0"));
    harness.cx.run_until_parked();
    let warnings = [
        "cannot register cmd-shift-space: already in use by another \
         application",
        "cannot register ctrl-alt-t: registration rejected",
    ];
    with_view(&mut harness, |view, cx| {
        view.follow_reload_with_warnings(&warnings, cx);
    });
    harness.wait_for("the reload's read", |cx| {
        settings(cx).is_some_and(|handle| {
            handle
                .read_with(cx, |view, _| {
                    view.status_text().starts_with("Applied with")
                })
                .unwrap_or(false)
        })
    });
    with_view(&mut harness, |view, _| {
        assert!(!view.read_only());
        assert_eq!(view.status_text(), "Applied with 2 warnings");
        assert_eq!(view.notice_text(), warnings.join("\n"));
    });
}

#[gpui::test]
fn settings_opens_with_the_warnings_the_desktop_already_reported(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("opens-warned", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    let refused = "cannot register ctrl-alt-t: registration rejected";
    // Startup registered the global shortcuts before any window opened.
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>()
            .config_report
            .refuse(vec![refused.to_owned()]);
    });
    open_settings(&mut harness, &file.path());
    harness.wait_for("the first read", settled);
    with_view(&mut harness, |view, _| {
        assert_eq!(view.status_text(), "Applied with a warning");
        assert_eq!(view.notice_text(), refused);
    });
}

#[gpui::test]
fn a_warning_holds_its_place_through_a_save_until_a_clean_reload(
    cx: &mut TestAppContext,
) {
    use super::settings_controls::{a11y_node, visual};
    let file = ScratchConfig::new("notice-holds", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    harness.wait_for("the first read", settled);
    let refused = "cannot register ctrl-alt-t: registration rejected";
    let warn = |harness: &mut DesktopHarness| {
        with_view(harness, |view, cx| {
            view.follow_reload_with_warnings(&[refused], cx);
        });
        harness.wait_for("the reload's read", |cx| {
            settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| {
                        view.status_text().starts_with("Applied with")
                    })
                    .unwrap_or(false)
            })
        });
    };
    let shown = |harness: &mut DesktopHarness| {
        with_window(harness, |_, window, _| window.refresh());
        harness.cx.run_until_parked();
        let details = a11y_node(harness, "settings-notice-details");
        let drawn = visual(harness)
            .debug_bounds("settings-notice-messages")
            .is_some();
        assert_eq!(details["aria"]["expanded"], drawn, "{details}");
        drawn
    };
    warn(&mut harness);
    assert!(!shown(&mut harness), "a warning starts folded");
    with_window(&mut harness, |view, window, cx| {
        view.press("notice:details", window, cx);
    });
    assert!(shown(&mut harness), "Details shows the messages");
    // Changing a setting leaves the notice in place while it saves. The
    // save's reload applies the file in full here, which settles it.
    with_view(&mut harness, |view, cx| {
        view.set("font.size", Some(toml::Value::Float(15.0)), cx);
        assert_eq!(view.status_text(), "Saving…");
        assert_eq!(view.notice_text(), refused);
    });
    harness
        .wait_for("the save", |cx| settled(cx) && file.text().contains("15.0"));
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert_eq!(view.status_text(), "Saved");
        assert_eq!(view.notice_text(), "");
    });
    warn(&mut harness);
    assert!(!shown(&mut harness), "a new warning starts folded");
    with_window(&mut harness, |view, window, cx| {
        view.press("notice:details", window, cx);
    });
    warn(&mut harness);
    assert!(shown(&mut harness), "a reload's update keeps it unfolded");
    // A clean reload clears it only once its reread finishes.
    with_view(&mut harness, |view, cx| {
        view.follow_reload(cx);
        assert_eq!(view.notice_text(), refused);
    });
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| {
        assert_eq!(view.status_text(), "Reloaded");
        assert_eq!(view.notice_text(), "");
    });
}

#[gpui::test]
fn check_for_updates_says_why_a_build_cannot(cx: &mut TestAppContext) {
    use super::settings_controls::a11y_node;
    let file = ScratchConfig::new("updates-check", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.show_section(huterm_config::settings::SectionId::Updates, cx);
        window.refresh();
    });
    harness.cx.run_until_parked();
    // Test builds have no updater.
    let check = a11y_node(&mut harness, "settings-updates-check");
    assert_eq!(check["aria"]["disabled"], true, "{check}");
    assert_eq!(
        check["aria"]["description"],
        "self-updates are available only in updater-enabled macOS release \
         builds",
        "{check}"
    );
}

#[gpui::test]
fn theme_cards_and_modifier_chips_work_from_the_keyboard(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("keyboard", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.press("theme:nord", window, cx);
        view.press("chip:terminal.link_modifiers:shift", window, cx);
    });
    harness.wait_for("both saves", |cx| {
        settled(cx) && file.text().contains("link_modifiers")
    });
    let modifier = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    assert_eq!(
        file.text(),
        format!(
            "[theme]\nname = \"nord\"\n\n[terminal]\nlink_modifiers = \"{modifier}-shift\"\n"
        )
    );
}

#[gpui::test]
fn pressing_enter_twice_before_the_save_keeps_the_new_value(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("enter-twice", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.type_into("font.size", "15", window, cx);
        view.enter("font.size", cx);
        view.enter("font.size", cx);
        assert_eq!(view.field_value("font.size", cx), "15");
        view.blur("font.size", cx);
    });
    harness.wait_for("the save", settled);
    assert_eq!(file.text(), "[font]\nsize = 15.0\n");
}

#[gpui::test]
fn a_field_stepped_or_typed_during_its_save_raises_no_conflict(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("field-steps", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        // The field exists, so steps go through it.
        view.type_into("font.size", "13", window, cx);
        view.step_once("font.size", true, cx);
        view.step_once("font.size", true, cx);
    });
    harness.wait_for("both steps", settled);
    with_window(&mut harness, |view, window, cx| {
        assert!(!view.conflicted("font.size"));
        assert_eq!(view.field_value("font.size", cx), "15");
        // Typing on while a save runs keeps the draft without a conflict.
        view.type_into("font.size", "16", window, cx);
        view.enter("font.size", cx);
        view.type_into("font.size", "17", window, cx);
    });
    harness.wait_for("the save", settled);
    with_window(&mut harness, |view, _, cx| {
        assert!(!view.conflicted("font.size"));
        assert_eq!(view.field_value("font.size", cx), "17");
    });
    assert_eq!(file.text(), "[font]\nsize = 16.0\n");
}

#[gpui::test]
fn closing_stays_open_for_a_conflict_or_a_draft_that_does_not_save(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("close-fail", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    std::fs::write(file.path(), "[font]\nsize = 20.0\n").unwrap();
    harness.cx.update(|cx| {
        settings(cx)
            .unwrap()
            .update(cx, |view, window, cx| {
                view.set("font.size", Some(toml::Value::Float(15.0)), cx);
                view.command(ids::CLOSE_PANE, window, cx)
            })
            .unwrap()
            .unwrap();
    });
    harness.wait_for("the conflict", |cx| {
        settled(cx) && conflicted(cx, "font.size")
    });
    harness.cx.run_until_parked();
    harness
        .cx
        .update(|cx| assert!(settings(cx).is_some(), "Settings closed"));
    // An unparseable draft keeps it open too.
    with_window(&mut harness, |view, window, cx| {
        view.use_file("font.size", cx);
        view.type_into("terminal.scrollback_rows", "lots", window, cx);
        view.focus_field("terminal.scrollback_rows", window, cx);
        view.command(ids::CLOSE_PANE, window, cx).unwrap();
    });
    harness.cx.run_until_parked();
    harness
        .cx
        .update(|cx| assert!(settings(cx).is_some(), "Settings closed"));
    assert_eq!(file.text(), "[font]\nsize = 20.0\n");
}

#[gpui::test]
fn escape_in_a_conflicted_field_keeps_the_file(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("escape-conflict", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    std::fs::write(file.path(), "[font]\nsize = 20.0\n").unwrap();
    with_window(&mut harness, |view, window, cx| {
        view.type_into("font.size", "15", window, cx);
        view.enter("font.size", cx);
    });
    harness.wait_for("the conflict", |cx| {
        settled(cx) && conflicted(cx, "font.size")
    });
    with_window(&mut harness, |view, _, cx| {
        view.escape("font.size", cx);
        assert!(!view.conflicted("font.size"));
        assert_eq!(view.field_value("font.size", cx), "20");
    });
    assert_eq!(file.text(), "[font]\nsize = 20.0\n");
}

#[gpui::test]
fn a_chosen_theme_shows_while_it_saves(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("theme-pending", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.press("theme:dracula", window, cx);
        assert_eq!(view.selected(cx), "dracula");
    });
    harness.wait_for("the save", settled);
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.selected(cx), "dracula");
    });
    assert_eq!(file.text(), "[theme]\nname = \"dracula\"\n");
}

#[gpui::test]
fn reset_and_menu_saves_update_an_untouched_field(cx: &mut TestAppContext) {
    let file = ScratchConfig::new(
        "untouched",
        "[font]\nfamily = \"Menlo\"\nsize = 13.0\n",
    );
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        // Rendering the rows creates their fields with the file's values.
        view.type_into("font.size", "13", window, cx);
        view.type_into("font.family", "Menlo", window, cx);
        view.set("font.size", None, cx);
        view.set(
            "font.family",
            Some(toml::Value::String("Monaco".into())),
            cx,
        );
    });
    harness.wait_for("both saves", settled);
    with_window(&mut harness, |view, _, cx| {
        assert_eq!(view.field_value("font.size", cx), "14");
        assert_eq!(view.field_value("font.family", cx), "Monaco");
        // Leaving the fields saves nothing back.
        view.blur("font.size", cx);
        view.blur("font.family", cx);
    });
    harness.wait_for("Settings to settle", settled);
    assert_eq!(file.text(), "[font]\nfamily = \"Monaco\"\n");
}

#[gpui::test]
fn escape_during_a_pending_reset_shows_the_default(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("escape-reset", "[font]\nsize = 13.0\n");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_window(&mut harness, |view, window, cx| {
        view.type_into("font.size", "13", window, cx);
        view.set("font.size", None, cx);
        view.escape("font.size", cx);
        assert_eq!(view.field_value("font.size", cx), "14");
        // Leaving the field showing the pending default keeps the reset.
        view.blur("font.size", cx);
    });
    harness.wait_for("the reset", settled);
    // The reset removes the only key, and the empty table with it.
    assert_eq!(file.text(), "");
}

#[gpui::test]
fn a_live_theme_change_recolors_open_fields(cx: &mut TestAppContext) {
    let file = ScratchConfig::new("live-theme", "");
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    let light = crate::themes::preview("tokyo-night-day", Path::new(""))
        .expect("a bundled theme");
    let expected = crate::desktop::overlay::tokens(&light);
    // The theme changes while Settings is open, as a reload applies it.
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().config.theme = light;
    });
    with_window(&mut harness, |_, window, _| window.refresh());
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.search_colors(cx), (expected.fg, expected.muted));
    });
}
