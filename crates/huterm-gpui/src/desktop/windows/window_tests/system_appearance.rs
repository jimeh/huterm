//! Tier 2 tests for `[theme] light` and `dark`: windows follow the
//! application's light or dark mode, and a window's own appearance, which
//! its frame takes from the theme, never picks the theme.

use super::super::system_appearance::{self, tests::DARK};
use super::*;
use crate::themes::ThemePair;

/// A dark-looking theme for light mode and a light-looking one for dark
/// mode, so each mode's frame reads as the other mode.
fn inverted() -> ThemePair {
    let theme = |name| {
        crate::themes::preview(name, Path::new("")).expect("a bundled theme")
    };
    ThemePair {
        light: theme("tokyo-night"),
        dark: theme("tokyo-night-day"),
        light_name: "tokyo-night".to_owned(),
        dark_name: "tokyo-night-day".to_owned(),
    }
}

/// The theme `window`'s view and the terminal of `tab` use.
fn themes_in(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
) -> (Theme, Theme) {
    let terminal = terminal(cx, window, tab).expect("the tab's terminal");
    let terminal = terminal.read(cx).theme.clone();
    let workspace = window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            Some(view.read(cx).config.theme.clone())
        })
        .ok()
        .flatten()
        .expect("a workspace window");
    (workspace, terminal)
}

fn follows(cx: &App) -> u64 {
    cx.global::<Desktop>().appearance_follows
}

#[gpui::test]
fn windows_follow_the_application_appearance(cx: &mut TestAppContext) {
    DARK.set(false);
    let pair = inverted();
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.themes = inverted();
    });
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_for("the child", |cx| {
        screen(cx, window, tab).is_some_and(|text| text.contains("READY"))
    });
    let light = pair.light.clone();
    harness.cx.update(|cx| {
        assert_eq!(themes_in(cx, window, tab), (light.clone(), light));
    });

    DARK.set(true);
    let followed = harness.cx.update(system_appearance::follow);
    assert!(followed, "dark mode changes the theme");
    harness.cx.run_until_parked();
    let dark = pair.dark.clone();
    harness.cx.update(|cx| {
        assert_eq!(themes_in(cx, window, tab), (dark.clone(), dark));
        assert_eq!(cx.global::<Desktop>().config.theme, pair.dark);
    });
    // The same mode again changes nothing.
    assert!(!harness.cx.update(system_appearance::follow));
    DARK.set(false);
}

#[gpui::test]
fn a_window_appearance_change_never_picks_the_theme(cx: &mut TestAppContext) {
    // Dark mode with a light theme for it: on macOS that theme gives the
    // window a light frame, which reports a light window appearance.
    DARK.set(true);
    let pair = inverted();
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.themes = inverted();
    });
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_for("the child", |cx| {
        screen(cx, window, tab).is_some_and(|text| text.contains("READY"))
    });
    let before = harness.cx.update(|cx| follows(cx));
    let dark = pair.dark.clone();
    harness.cx.update(|cx| {
        assert_eq!(themes_in(cx, window, tab), (dark.clone(), dark.clone()));
        assert_eq!(cx.global::<Desktop>().config.theme, dark);
    });
    // GPUI reports the frame's change, repeatedly, as the window's
    // appearance changing; the application's stays dark.
    for _ in 0..3 {
        harness.cx.update(|cx| {
            window
                .update(cx, |root, window, cx| {
                    let view = root.downcast::<WorkspaceView>().unwrap();
                    view.update(cx, |view, cx| {
                        view.window_appearance_changed(window, cx);
                    });
                })
                .unwrap();
        });
        harness.cx.run_until_parked();
    }
    harness.cx.update(|cx| {
        assert_eq!(themes_in(cx, window, tab), (dark.clone(), dark.clone()));
        assert_eq!(cx.global::<Desktop>().config.theme, dark);
        assert_eq!(follows(cx), before, "the theme never switched");
    });

    // A real mode change still reaches the window through its callback, as
    // the desktop portal's does on Linux.
    DARK.set(false);
    harness.cx.update(|cx| {
        window
            .update(cx, |root, window, cx| {
                let view = root.downcast::<WorkspaceView>().unwrap();
                view.update(cx, |view, cx| {
                    view.window_appearance_changed(window, cx);
                });
            })
            .unwrap();
    });
    harness.cx.run_until_parked();
    let light = pair.light;
    harness.cx.update(|cx| {
        assert_eq!(themes_in(cx, window, tab), (light.clone(), light));
        assert_eq!(follows(cx), before + 1);
    });
}

#[gpui::test]
fn a_reload_uses_the_theme_for_the_system_mode(cx: &mut TestAppContext) {
    DARK.set(false);
    let harness = DesktopHarness::new(cx);
    let pair = inverted();
    harness.cx.update(|cx| {
        // A load leaves the dark mode's theme in use until the desktop
        // follows the system's mode.
        let mut loaded = cx.global::<Desktop>().config.clone();
        loaded.themes = inverted();
        loaded.theme = loaded.themes.dark.clone();
        finish_reload(
            cx,
            ReloadKind::Manual,
            Path::new("huterm-test.toml"),
            config::Reloaded {
                config: Ok(loaded),
                inputs: config::Inputs::default(),
                missing: false,
            },
            0,
        );
        assert_eq!(cx.global::<Desktop>().config.theme, pair.light);
        assert!(!cx.global::<Desktop>().system_mode.is_dark());
    });
}
