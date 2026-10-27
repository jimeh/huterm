//! Tier 2 tests for the Settings window: opening and closing it, saving
//! and resetting through the patch service, conflicts, and invalid files.

use std::path::{Path, PathBuf};

use super::super::settings_window::SettingsView;
use super::*;

/// A configuration file in its own temporary directory, removed on drop
/// even when an assertion fails first.
struct ScratchConfig(PathBuf);

impl ScratchConfig {
    fn new(name: &str, text: &str) -> Self {
        let directory = std::env::temp_dir()
            .join(format!("huterm-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("config.toml"), text).unwrap();
        Self(directory)
    }

    fn path(&self) -> PathBuf {
        self.0.join("config.toml")
    }

    fn text(&self) -> String {
        std::fs::read_to_string(self.path()).unwrap()
    }
}

impl Drop for ScratchConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn settings(cx: &App) -> Option<gpui::WindowHandle<SettingsView>> {
    cx.global::<Desktop>().settings
}

/// Points the desktop at `path`, opens Settings as its shortcut does, and
/// waits for its read.
fn open_settings(harness: &mut DesktopHarness, path: &Path) {
    let path = path.to_owned();
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().config_path = path;
        let open = CommandInvocation::new(ids::OPEN_SETTINGS, Vec::new());
        Desktop::invoke(cx, &open, None).unwrap();
    });
    harness.wait_for("Settings to read the file", settled);
}

fn settled(cx: &mut App) -> bool {
    settings(cx).is_some_and(|handle| {
        handle
            .read_with(cx, |view, _| view.settled())
            .unwrap_or(false)
    })
}

fn with_view<R>(
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
fn with_window<R>(
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

fn conflicted(cx: &mut App, path: &str) -> bool {
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
