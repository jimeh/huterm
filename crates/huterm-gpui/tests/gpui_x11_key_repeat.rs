//! Compile the vendored X11 held-key tracker directly, without copying
//! behavior.
#[rustfmt::skip]
#[path = "../../../third-party/vendor/gpui-pre-linux-0.3.9/src/linux/x11/key_repeat.rs"]
mod key_repeat;

use key_repeat::HeldKeys;

#[test]
fn a_press_of_a_key_still_down_is_held_with_detectable_auto_repeat() {
    let mut keys = HeldKeys::new(true);
    assert!(!keys.press(38));
    assert!(keys.press(38), "a repeat has no release before it");
    assert!(keys.press(38));
    // Another key is its own press.
    assert!(!keys.press(39));
    keys.release(38);
    assert!(!keys.press(38), "a fresh press after a release");
    // Focus left, so a release may have gone to another client.
    keys.clear();
    assert!(!keys.press(38));
    assert!(!keys.press(39));
}

#[test]
fn no_press_is_held_without_detectable_auto_repeat() {
    // The server sends each repeat as a release and a press.
    let mut keys = HeldKeys::new(false);
    assert!(!keys.press(38));
    assert!(!keys.press(38));
    keys.release(38);
    assert!(!keys.press(38));
}
