//! Focus visibility: whether a focused control shows its focus ring.
//!
//! A ring shows for keyboard focus and not after a pointer press, as macOS,
//! GNOME, and CSS `:focus-visible` behave. GPUI's own `focus_visible` style
//! rechecks the input mode every frame, and any pointer move switches it to
//! mouse, so its ring vanishes when the pointer is nudged after Tab. This
//! module decides visibility when focus moves instead:
//!
//! - Each window records whether the last input was the keyboard at the
//!   moment its focused handle changed.
//! - A pointer press anywhere in the window clears the record, because a
//!   press on the control that already has focus does not change focus.
//! - The ring follows the record until focus moves again.
//!
//! A window takes part by mounting [`tracker`] once in its root view.
//! Components draw their ring through [`ring`].

use std::collections::HashMap;

use gpui::{
    App, BoxShadow, DispatchPhase, FocusHandle, Global, IntoElement,
    MouseDownEvent, Styled, WeakFocusHandle, Window, WindowId, canvas,
};

use crate::tokens::Tokens;

/// One window's focus record.
#[derive(Default)]
struct Record {
    /// The handle focused when the record was last taken.
    focused: Option<WeakFocusHandle>,
    /// Whether that focus arrived from the keyboard, until a pointer press.
    keyboard: bool,
    /// Handles whose ring this frame's render requested.
    #[cfg(feature = "test-support")]
    pending: Vec<WeakFocusHandle>,
    /// Handles whose ring the last completed frame drew.
    #[cfg(feature = "test-support")]
    drawn: Vec<WeakFocusHandle>,
}

#[derive(Default)]
struct Records(HashMap<WindowId, Record>);

impl Global for Records {}

fn window_id(window: &Window) -> WindowId {
    window.window_handle().window_id()
}

/// Whether `handle` shows a focus ring: it has focus, and that focus came
/// from the keyboard with no pointer press since.
#[must_use]
pub fn visible(handle: &FocusHandle, window: &Window, cx: &App) -> bool {
    if !handle.is_focused(window) {
        return false;
    }
    let record = cx
        .try_global::<Records>()
        .and_then(|records| records.0.get(&window_id(window)));
    match record {
        Some(record)
            if record
                .focused
                .as_ref()
                .is_some_and(|focused| focused == handle) =>
        {
            record.keyboard
        }
        // Focus moved since the last frame: the input that moved it decides.
        _ => window.last_input_was_keyboard(),
    }
}

/// The ring `handle` draws: `tokens`' focus ring while [`visible`], and no
/// shadow otherwise. Apply it with `Styled::shadow`.
#[must_use]
pub fn ring(
    handle: &FocusHandle,
    tokens: Tokens,
    window: &Window,
    cx: &mut App,
) -> Vec<BoxShadow> {
    if !visible(handle, window, cx) {
        return Vec::new();
    }
    #[cfg(feature = "test-support")]
    cx.default_global::<Records>()
        .0
        .entry(window_id(window))
        .or_default()
        .pending
        .push(handle.downgrade());
    tokens.focus_ring()
}

/// Whether the last completed frame drew `handle`'s ring through [`ring`].
/// Tests assert the drawn ring rather than the window's record, so a control
/// still styled some other way fails them.
#[cfg(feature = "test-support")]
#[must_use]
pub fn ring_drawn(handle: &FocusHandle, window: &Window, cx: &App) -> bool {
    cx.try_global::<Records>()
        .and_then(|records| records.0.get(&window_id(window)))
        .is_some_and(|record| record.drawn.iter().any(|drawn| drawn == handle))
}

/// The element that keeps a window's focus record. Mount it once in the
/// window's root view; it takes no space.
#[must_use]
pub fn tracker() -> impl IntoElement {
    canvas(
        |_, window, cx| {
            let keyboard = window
                .focused(cx)
                .is_some_and(|handle| visible(&handle, window, cx));
            let focused = window.focused(cx).map(|handle| handle.downgrade());
            let id = window_id(window);
            let open: Vec<WindowId> = cx
                .windows()
                .iter()
                .map(gpui::AnyWindowHandle::window_id)
                .collect();
            let records = cx.default_global::<Records>();
            // A window seen for the first time is a cue to drop the records
            // of windows that have closed.
            if !records.0.contains_key(&id) {
                records.0.retain(|id, _| open.contains(id));
            }
            let record = records.0.entry(id).or_default();
            record.focused = focused;
            record.keyboard = keyboard;
            #[cfg(feature = "test-support")]
            {
                record.drawn = std::mem::take(&mut record.pending);
            }
        },
        |_, (), window, _| {
            window.on_mouse_event(|_: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                let id = window_id(window);
                if let Some(record) = cx
                    .default_global::<Records>()
                    .0
                    .get_mut(&id)
                    .filter(|record| record.keyboard)
                {
                    record.keyboard = false;
                    window.refresh();
                }
            });
        },
    )
    .absolute()
    .size(gpui::px(0.0))
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use gpui::{
        FocusHandle, Modifiers, TestAppContext, VisualTestContext, div, point,
        prelude::*, px,
    };

    use super::{ring, ring_drawn};
    use crate::testing::{host, tokens};

    /// A focusable 40-point square at the window's origin that draws its
    /// ring through [`ring`].
    fn control(
        cx: &mut TestAppContext,
    ) -> (FocusHandle, &mut VisualTestContext) {
        let handle = cx.update(|cx| cx.focus_handle());
        let drawn = handle.clone();
        let (_, cx) = host(cx, move |window, cx| {
            div()
                .id("control")
                .track_focus(&drawn)
                .size(px(40.0))
                .shadow(ring(&drawn, tokens(), window, cx))
                .into_any_element()
        });
        (handle, cx)
    }

    fn drawn(handle: &FocusHandle, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| ring_drawn(handle, window, cx))
    }

    /// Moves focus to `handle` from the keyboard, as Tab does.
    fn focus_from_keyboard(handle: &FocusHandle, cx: &mut VisualTestContext) {
        // A key press sets GPUI's input modality; nothing binds it here.
        cx.simulate_keystrokes("a");
        cx.update(|window, cx| handle.focus(window, cx));
        cx.run_until_parked();
    }

    #[gpui::test]
    fn keyboard_focus_keeps_its_ring_until_a_press(cx: &mut TestAppContext) {
        let (handle, cx) = control(cx);
        focus_from_keyboard(&handle, cx);
        assert!(drawn(&handle, cx), "keyboard focus draws the ring");

        cx.simulate_mouse_move(
            point(px(200.0), px(200.0)),
            None,
            Modifiers::none(),
        );
        cx.run_until_parked();
        assert!(drawn(&handle, cx), "a pointer move keeps it");

        // A press on the control that already has focus changes no focus.
        cx.simulate_click(point(px(10.0), px(10.0)), Modifiers::none());
        cx.run_until_parked();
        assert!(cx.update(|window, _| handle.is_focused(window)));
        assert!(!drawn(&handle, cx), "a press removes it");
    }

    #[gpui::test]
    fn pointer_focus_draws_no_ring(cx: &mut TestAppContext) {
        let (handle, cx) = control(cx);
        cx.simulate_click(point(px(10.0), px(10.0)), Modifiers::none());
        cx.run_until_parked();
        assert!(cx.update(|window, _| handle.is_focused(window)));
        assert!(!drawn(&handle, cx));
    }
}
