//! Click handling shared by components.
//!
//! GPUI turns Enter and Space on a focused element with a click listener into
//! a `ClickEvent::Keyboard`. Huterm's keys are catalog commands the user can
//! rebind, so a component acts on keys only through its methods, which the
//! owner routes commands to, and its click listeners ignore keyboard clicks.
//! Otherwise removing a binding would not remove the key. Pointer clicks and
//! accessibility Click actions, which GPUI delivers as a synthesized pointer
//! press, still act.

use gpui::ClickEvent;

/// Whether `event` is GPUI's implicit Enter or Space click, which component
/// click listeners ignore.
#[must_use]
pub fn from_keyboard(event: &ClickEvent) -> bool {
    matches!(event, ClickEvent::Keyboard(_))
}
