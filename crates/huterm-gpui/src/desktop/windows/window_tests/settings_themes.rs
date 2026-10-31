//! Tier 2 tests for the Appearance theme picker's modes: one theme, or one
//! for each of the system's light and dark modes, and the keys each choice
//! writes.

use super::super::settings_window::SettingsView;
use super::super::system_appearance::tests::DARK;
use super::settings::{
    ScratchConfig, open_settings, settled, with_view, with_window,
};
use super::settings_controls::break_file;
use super::*;

fn press(harness: &mut DesktopHarness, key: &str) {
    with_window(harness, |view, window, cx| view.press(key, window, cx));
    harness.wait_for(&format!("{key} to save"), settled);
}

fn mode(harness: &mut DesktopHarness, auto: bool) {
    with_view(harness, |view, cx| view.set_theme_mode(auto, cx));
    harness.wait_for("the mode to save", settled);
}

#[gpui::test]
fn each_mode_picks_its_own_theme_and_one_theme_keeps_the_active_one(
    cx: &mut TestAppContext,
) {
    DARK.set(false);
    let file = ScratchConfig::new(
        "theme-modes",
        "[theme]\nname = \"nord\"\nbackground = \"#000000\"\n",
    );
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    // Following the system starts both modes on the current theme.
    mode(&mut harness, true);
    assert_eq!(
        file.text(),
        "[theme]\nbackground = \"#000000\"\nlight = \"nord\"\ndark = \"nord\"\n"
    );
    // The cards choose the theme for the system's mode now, light.
    press(&mut harness, "theme:dracula");
    assert_eq!(
        file.text(),
        "[theme]\nbackground = \"#000000\"\nlight = \"dracula\"\ndark = \"nord\"\n"
    );
    // A slot card picks the other mode; any theme can go in either.
    press(&mut harness, "theme-slot:dark");
    press(&mut harness, "theme:github-light");
    assert_eq!(
        file.text(),
        "[theme]\nbackground = \"#000000\"\nlight = \"dracula\"\ndark = \"github-light\"\n"
    );
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.selected(cx), "github-light");
    });
    // One theme keeps the theme in use for the system's mode now.
    mode(&mut harness, false);
    assert_eq!(
        file.text(),
        "[theme]\nbackground = \"#000000\"\nname = \"dracula\"\n"
    );
}

#[gpui::test]
fn a_mode_switch_in_dark_mode_keeps_the_dark_theme(cx: &mut TestAppContext) {
    DARK.set(true);
    let file = ScratchConfig::new(
        "theme-modes-dark",
        "[theme]\nlight = \"one-light\"\ndark = \"kanagawa-wave\"\n",
    );
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    // The grid starts on the system's mode, dark.
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.selected(cx), "kanagawa-wave");
    });
    mode(&mut harness, false);
    assert_eq!(file.text(), "[theme]\nname = \"kanagawa-wave\"\n");
    DARK.set(false);
}

#[gpui::test]
fn reset_clears_every_theme_choice_but_keeps_overrides(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new(
        "theme-reset-modes",
        "theme = { name = \"nord\", light = \"one-light\", dark = \"dracula\", background = \"#000000\" }\n",
    );
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    with_view(&mut harness, |view, cx| view.set("theme", None, cx));
    harness.wait_for("the reset", |cx| {
        settled(cx) && !file.text().contains("dracula")
    });
    assert_eq!(file.text(), "theme = { background = \"#000000\" }\n");
}

#[gpui::test]
fn active_marks_the_theme_in_use_not_the_files_choice(cx: &mut TestAppContext) {
    DARK.set(false);
    // The desktop runs the default; the file chooses another theme that
    // no reload has applied yet, as while watching is off.
    let file = ScratchConfig::new(
        "theme-active-applied",
        "[theme]\nlight = \"one-light\"\n",
    );
    let mut harness = DesktopHarness::new(cx);
    open_settings(&mut harness, &file.path());
    harness.wait_for("the first read", settled);
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.selected(cx), "one-light");
        assert_eq!(SettingsView::marked_active(cx), "tokyo-night");
    });
    // A file that fails to load keeps the last working theme in use.
    break_file(&mut harness, &file, "[theme]\ndark = \"nope\"\n");
    harness.cx.update(|cx| {
        assert_eq!(SettingsView::marked_active(cx), "tokyo-night");
    });
}

#[gpui::test]
fn settings_follows_a_mode_change_that_keeps_the_theme(
    cx: &mut TestAppContext,
) {
    DARK.set(false);
    // Both modes use one theme, as Match system starts them.
    let file = ScratchConfig::new(
        "theme-mode-change",
        "[theme]\nlight = \"nord\"\ndark = \"nord\"\n",
    );
    let mut harness = DesktopHarness::with_config(cx, |config| {
        let nord = crate::themes::preview("nord", Path::new("")).unwrap();
        config.themes = crate::themes::ThemePair {
            light: nord.clone(),
            dark: nord,
            light_name: "nord".to_owned(),
            dark_name: "nord".to_owned(),
        };
    });
    open_settings(&mut harness, &file.path());
    harness.wait_for("the first read", settled);
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.target_mode(cx), "theme-slot:light");
    });
    // Settings, the only window, hears the change as the desktop portal's
    // reaches every window on Linux; the theme stays, the mode does not.
    let follows = harness
        .cx
        .update(|cx| cx.global::<Desktop>().appearance_follows);
    DARK.set(true);
    harness.cx.update(SettingsView::appearance_changed);
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert!(cx.global::<Desktop>().system_mode.is_dark());
        assert_eq!(cx.global::<Desktop>().appearance_follows, follows);
    });
    with_view(&mut harness, |view, cx| {
        assert_eq!(view.target_mode(cx), "theme-slot:dark");
        assert_eq!(SettingsView::marked_active(cx), "nord");
    });
    // A keyboard pick now sets the dark mode's theme, as Settings shows.
    press(&mut harness, "theme:dracula");
    assert_eq!(
        file.text(),
        "[theme]\nlight = \"nord\"\ndark = \"dracula\"\n"
    );
    DARK.set(false);
}

#[gpui::test]
fn settings_alone_follows_a_mode_change_to_another_theme(
    cx: &mut TestAppContext,
) {
    DARK.set(false);
    let theme = |name| crate::themes::preview(name, Path::new("")).unwrap();
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.themes = crate::themes::ThemePair {
            light: theme("one-light"),
            dark: theme("dracula"),
            light_name: "one-light".to_owned(),
            dark_name: "dracula".to_owned(),
        };
    });
    let file = ScratchConfig::new(
        "theme-settings-alone",
        "[theme]\nlight = \"one-light\"\ndark = \"dracula\"\n",
    );
    open_settings(&mut harness, &file.path());
    harness.cx.update(|cx| {
        assert_eq!(cx.global::<Desktop>().config.theme, theme("one-light"));
    });
    DARK.set(true);
    harness.cx.update(SettingsView::appearance_changed);
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert_eq!(cx.global::<Desktop>().config.theme, theme("dracula"));
    });
    harness.cx.update(|cx| {
        assert_eq!(SettingsView::marked_active(cx), "dracula");
    });
    DARK.set(false);
}
