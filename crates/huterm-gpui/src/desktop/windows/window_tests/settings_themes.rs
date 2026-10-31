//! Tier 2 tests for the Appearance theme picker's modes: one theme, or one
//! for each of the system's light and dark modes, and the keys each choice
//! writes.

use super::super::system_appearance::tests::DARK;
use super::settings::{
    ScratchConfig, open_settings, settled, with_view, with_window,
};
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
