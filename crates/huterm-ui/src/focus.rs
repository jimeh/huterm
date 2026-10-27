//! Focus visibility: whether a focused control shows its focus ring.
//!
//! A ring shows for keyboard focus and not after a pointer press, as macOS,
//! GNOME, and CSS `:focus-visible` behave. GPUI's own `focus_visible` style
//! rechecks the input mode every frame, and any pointer move switches it to
//! mouse, so its ring vanishes when the pointer is nudged after Tab. This
//! module decides visibility when focus moves instead:
//!
//! - Each window remembers whether its last press was a key or a pointer
//!   button. Pointer motion is not a press, so it changes nothing.
//! - When the focused handle changes, the ring follows the press that came
//!   before the change.
//! - A pointer press anywhere in the window hides the ring, because a press
//!   on the control that already has focus does not change focus.
//!
//! A window takes part by mounting [`tracker`] once in its root view.
//! Components draw their ring through [`ring`].

use std::collections::HashMap;

use gpui::{
    App, DispatchPhase, FocusHandle, Global, IntoElement, MouseDownEvent,
    Styled, Subscription, WeakFocusHandle, Window, WindowId, canvas,
};

use crate::tokens::Tokens;

/// One window's focus record.
#[derive(Default)]
struct Record {
    /// The handle focused when the record was last taken.
    focused: Option<WeakFocusHandle>,
    /// Whether that handle shows its ring.
    keyboard: bool,
    /// Whether the window's last press was a key rather than a pointer
    /// button: the input that decides a focus change not yet recorded.
    last_press_was_key: bool,
    /// Handles whose ring this frame applied.
    #[cfg(feature = "test-support")]
    pending: Vec<WeakFocusHandle>,
    /// Handles whose ring the last painted frame applied.
    #[cfg(feature = "test-support")]
    drawn: Vec<WeakFocusHandle>,
}

#[derive(Default)]
struct Records {
    windows: HashMap<WindowId, Record>,
    /// Notes key presses in every window. Key listeners on an element see
    /// only keys dispatched along the focus path, which the tracker is not
    /// on, so the record uses an app-wide keystroke interceptor instead.
    keys: Option<Subscription>,
}

impl Global for Records {}

fn window_id(window: &Window) -> WindowId {
    window.window_handle().window_id()
}

/// Whether `handle` shows a focus ring: it has focus, and the press that
/// focused it was a key, with no pointer press since.
#[must_use]
pub fn visible(handle: &FocusHandle, window: &Window, cx: &App) -> bool {
    if !handle.is_focused(window) {
        return false;
    }
    let record = cx
        .try_global::<Records>()
        .and_then(|records| records.windows.get(&window_id(window)));
    match record {
        Some(record)
            if record
                .focused
                .as_ref()
                .is_some_and(|focused| focused == handle) =>
        {
            record.keyboard
        }
        // Focus moved since the last frame: the press before it decides.
        Some(record) => record.last_press_was_key,
        // A window without a tracker falls back to GPUI's input modality.
        None => window.last_input_was_keyboard(),
    }
}

/// `element` with `handle`'s focus ring while [`visible`]. Apply it last
/// among an element's shadows, because a later `shadow` replaces it.
#[must_use]
pub fn ring<E: Styled>(
    element: E,
    handle: &FocusHandle,
    tokens: Tokens,
    window: &Window,
    cx: &mut App,
) -> E {
    if !visible(handle, window, cx) {
        return element;
    }
    // Only a tracked window keeps a record; creating one here would change
    // `visible` for a window without a tracker in test builds alone.
    #[cfg(feature = "test-support")]
    with_record(window, cx, |record| record.pending.push(handle.downgrade()));
    element.shadow(tokens.focus_ring())
}

/// Whether the last painted frame applied `handle`'s ring through [`ring`].
/// Tests assert the applied ring rather than the window's record, so a
/// control still styled some other way fails them.
#[cfg(feature = "test-support")]
#[must_use]
pub fn ring_drawn(handle: &FocusHandle, window: &Window, cx: &App) -> bool {
    cx.try_global::<Records>()
        .and_then(|records| records.windows.get(&window_id(window)))
        .is_some_and(|record| record.drawn.iter().any(|drawn| drawn == handle))
}

/// The element that keeps a window's focus record. Mount it once in the
/// window's root view; it takes no space. It records during paint, which
/// follows every render and prepaint, including lists that render their
/// rows while prepainting and deferred overlays.
#[must_use]
pub fn tracker() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |_, (), window, cx| {
            let focused = window.focused(cx);
            let keyboard = focused
                .as_ref()
                .is_some_and(|handle| visible(handle, window, cx));
            let id = window_id(window);
            let open: Vec<WindowId> = cx
                .windows()
                .iter()
                .map(gpui::AnyWindowHandle::window_id)
                .collect();
            if cx.default_global::<Records>().keys.is_none() {
                let keys = cx.intercept_keystrokes(|_, window, cx| {
                    with_record(window, cx, |record| {
                        record.last_press_was_key = true;
                    });
                });
                cx.global_mut::<Records>().keys = Some(keys);
            }
            let records = cx.global_mut::<Records>();
            // A window seen for the first time is a cue to drop the records
            // of windows that have closed.
            if !records.windows.contains_key(&id) {
                records.windows.retain(|id, _| open.contains(id));
            }
            let record = records.windows.entry(id).or_default();
            record.focused = focused.map(|handle| handle.downgrade());
            record.keyboard = keyboard;
            #[cfg(feature = "test-support")]
            {
                record.drawn = std::mem::take(&mut record.pending);
            }
            // Capture runs before any handler moves focus for this press.
            window.on_mouse_event(|_: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                let hid = with_record(window, cx, |record| {
                    record.last_press_was_key = false;
                    std::mem::take(&mut record.keyboard)
                });
                if hid {
                    window.refresh();
                }
            });
        },
    )
    .absolute()
    .size(gpui::px(0.0))
}

fn with_record<T: Default>(
    window: &Window,
    cx: &mut App,
    update: impl FnOnce(&mut Record) -> T,
) -> T {
    cx.default_global::<Records>()
        .windows
        .get_mut(&window_id(window))
        .map(update)
        .unwrap_or_default()
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
            let control =
                div().id("control").track_focus(&drawn).size(px(40.0));
            ring(control, &drawn, tokens(), window, cx).into_any_element()
        });
        (handle, cx)
    }

    fn drawn(handle: &FocusHandle, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| ring_drawn(handle, window, cx))
    }

    /// Moves focus to `handle` from the keyboard, as Tab does.
    fn focus_from_keyboard(handle: &FocusHandle, cx: &mut VisualTestContext) {
        // A key press the tracker notes; nothing binds it here.
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
    fn pointer_motion_before_the_first_draw_keeps_the_ring(
        cx: &mut TestAppContext,
    ) {
        let (handle, cx) = control(cx);
        cx.simulate_keystrokes("a");
        // Focus moves and the pointer moves before any frame shows it.
        cx.update(|window, cx| {
            handle.focus(window, cx);
            window.dispatch_event(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position: point(px(200.0), px(200.0)),
                    pressed_button: None,
                    modifiers: Modifiers::none(),
                }),
                cx,
            );
        });
        cx.run_until_parked();
        assert!(drawn(&handle, cx));
    }

    #[gpui::test]
    fn a_key_press_after_pointer_focus_shows_no_ring(cx: &mut TestAppContext) {
        let (handle, cx) = control(cx);
        cx.simulate_click(point(px(10.0), px(10.0)), Modifiers::none());
        cx.run_until_parked();
        // The key moves no focus, so the pointer press still decides. A key
        // that changes nothing draws no frame, so force one: any later
        // redraw would otherwise show the wrong ring.
        cx.simulate_keystrokes("a");
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert!(cx.update(|window, _| handle.is_focused(window)));
        assert!(!drawn(&handle, cx));
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
