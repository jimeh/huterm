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

#[test]
fn a_fast_fresh_press_after_a_real_release_is_not_held_with_detectable_auto_repeat()
 {
    let mut keys = HeldKeys::new(true);
    assert!(!keys.press(38));
    // The next press of the same key arrives 10 ms after its release, in the
    // same event batch; the release is real, so the reducer delivers it.
    if keys.keeps_release_before_press(38, 1_000, 38, 1_010) {
        keys.release(38);
    }
    assert!(!keys.press(38), "a fast second press is a fresh press");
}

#[test]
fn a_release_right_before_a_press_of_its_key_is_synthetic_without_detectable_auto_repeat()
 {
    let keys = HeldKeys::new(false);
    assert!(!keys.keeps_release_before_press(38, 1_000, 38, 1_000));
    assert!(!keys.keeps_release_before_press(38, 1_000, 38, 1_020));
    assert!(keys.keeps_release_before_press(38, 1_000, 38, 1_021));
    assert!(keys.keeps_release_before_press(38, 1_000, 39, 1_000));
}
