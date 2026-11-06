//! Native hit testing starts with an event in the source window, never the
//! cached GPUI mouse position or an X11 parent-relative window origin.

use super::{Pixels, Window};

#[cfg(target_os = "macos")]
#[path = "window_locator/macos.rs"]
mod platform;
#[cfg(not(target_os = "macos"))]
#[path = "window_locator/x11.rs"]
mod platform;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Located {
    Window(gpui::WindowId, gpui::Point<Pixels>),
    Outside,
    /// An application window that is not a terminal, or a failed native
    /// query. Neither permits tearing a tab off.
    Blocked,
}

pub(super) trait WindowLocator {
    /// The topmost window under `point`, searching below the native window
    /// `below` (the drag preview, when one is open), which never counts.
    fn locate(
        &self,
        source: &Window,
        point: gpui::Point<Pixels>,
        windows: &[(gpui::WindowId, u64)],
        below: Option<u64>,
    ) -> Located;
}

#[derive(Default)]
pub(super) struct NativeLocator;

impl WindowLocator for NativeLocator {
    fn locate(
        &self,
        source: &Window,
        point: gpui::Point<Pixels>,
        windows: &[(gpui::WindowId, u64)],
        below: Option<u64>,
    ) -> Located {
        #[cfg(test)]
        if identity(source).is_none() {
            return Located::Window(source.window_handle().window_id(), point);
        }
        platform::locate(source, point, windows, below)
            .unwrap_or(Located::Blocked)
    }
}

pub(super) fn identity(window: &Window) -> Option<u64> {
    platform::identity(window).ok()
}

/// A native stack complete over viewable windows wins; otherwise use
/// the model's most-recently-active-first order in its entirety.
#[cfg(any(all(test, feature = "test-support"), not(target_os = "macos")))]
fn ordered(
    windows: &[(gpui::WindowId, u64)],
    stack: &[u64],
    viewable: &[u64],
) -> Vec<(gpui::WindowId, u64)> {
    let windows: Vec<_> = windows
        .iter()
        .copied()
        .filter(|(_, native)| viewable.contains(native))
        .collect();
    if windows.iter().all(|(_, native)| stack.contains(native)) {
        stack
            .iter()
            .rev()
            .filter_map(|native| windows.iter().find(|(_, id)| id == native))
            .copied()
            .collect()
    } else {
        windows
    }
}

pub(super) fn screen_point(
    source: &Window,
    pointer: gpui::Point<Pixels>,
) -> Option<gpui::Point<f64>> {
    platform::screen_point(source, pointer).ok()
}

#[cfg(target_os = "macos")]
pub(super) fn place(
    number: u64,
    pointer: gpui::Point<f64>,
    grab: gpui::Point<Pixels>,
) {
    if let Err(error) = platform::place(number, pointer, grab) {
        eprintln!("Tear-off placement: {error}");
    }
}

/// Quake's transition target is not proof that it is on screen yet.
pub(super) fn target_visible(
    visible: bool,
    minimized: bool,
    transitioning: bool,
) -> bool {
    visible && !minimized && !transitioning
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "test-support")]
    use gpui::VisualContext as _;

    #[test]
    fn hidden_minimized_and_transitioning_windows_are_skipped() {
        assert!(target_visible(true, false, false));
        assert!(!target_visible(false, false, false));
        assert!(!target_visible(true, true, false));
        assert!(!target_visible(true, false, true));
    }

    #[cfg(feature = "test-support")]
    #[gpui::test]
    fn a_missing_or_partial_stack_uses_the_activation_order(
        cx: &mut gpui::TestAppContext,
    ) {
        let first = cx.add_empty_window().window_handle().window_id();
        let second = cx.add_empty_window().window_handle().window_id();
        let windows = [(second, 22), (first, 11)];
        assert_eq!(ordered(&windows, &[], &[11, 22]), windows);
        assert_eq!(ordered(&windows, &[11], &[11, 22]), windows);
        assert_eq!(
            ordered(&windows, &[22, 11], &[11, 22]),
            [(first, 11), (second, 22)]
        );
        let hidden = cx.add_empty_window().window_handle().window_id();
        let with_hidden = [(hidden, 33), (second, 22), (first, 11)];
        assert_eq!(
            ordered(&with_hidden, &[22, 11], &[11, 22]),
            [(first, 11), (second, 22)]
        );
        assert_eq!(ordered(&with_hidden, &[11], &[11, 22]), windows);
        assert_eq!(ordered(&with_hidden, &[], &[]), []);
    }
}
