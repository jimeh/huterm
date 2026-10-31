//! `[theme] light` and `dark` follow the system's light or dark mode.
//!
//! The mode is read from the application's appearance, never a window's:
//! each window's frame follows the theme (`native_appearance`), so a light
//! theme for dark mode makes its window light. Reading that back would pick
//! the light-mode theme, whose frame would pick the dark-mode one again.

use gpui::App;

use super::Desktop;
use super::views::broadcast;

/// Whether the system is in dark mode, from the application's appearance:
/// `NSApp.effectiveAppearance` on macOS and the desktop portal's color
/// scheme on Linux, which is light until the portal answers.
#[cfg(not(test))]
pub(super) fn system_dark(cx: &App) -> bool {
    #[cfg(target_os = "macos")]
    match crate::native_system_appearance::is_dark() {
        Ok(dark) => return dark,
        Err(error) => eprintln!("System appearance: {error:#}"),
    }
    matches!(
        cx.window_appearance(),
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    )
}

/// The system mode tests set through [`tests::DARK`]; GPUI's test
/// platform is always light.
#[cfg(test)]
pub(super) fn system_dark(_: &App) -> bool {
    tests::DARK.get()
}

/// Uses the theme for the system's mode in every window when it differs
/// from the theme in use, and returns whether it did.
pub(super) fn follow(cx: &mut App) -> bool {
    let dark = system_dark(cx);
    let desktop = cx.global_mut::<Desktop>();
    if !desktop.config.follow_appearance(dark) {
        return false;
    }
    desktop.appearance_follows += 1;
    let theme = desktop.config.theme.clone();
    broadcast(cx, |view, cx| view.apply_theme(&theme, cx));
    // Settings, the menus, and every window's frame read the theme when
    // they draw.
    cx.refresh_windows();
    true
}

/// Follows the system's mode while the application runs. macOS reports
/// the application's appearance changes here; on Linux the desktop
/// portal's reach every window's appearance callback, which follows too.
pub(super) fn watch(cx: &mut App) {
    #[cfg(target_os = "macos")]
    match crate::native_system_appearance::SystemAppearance::observe() {
        Ok((observer, changes)) => cx
            .spawn(async move |cx| {
                let _observer = observer;
                while changes.recv().await.is_ok() {
                    cx.update(|cx| {
                        follow(cx);
                    });
                }
            })
            .detach(),
        Err(error) => eprintln!("System appearance: {error:#}"),
    }
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
}

#[cfg(test)]
pub(super) mod tests {
    use std::cell::Cell;

    thread_local! {
        /// Whether `system_dark` reports dark mode.
        pub(in super::super) static DARK: Cell<bool> = const { Cell::new(false) };
    }
}
