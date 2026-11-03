//! Compile the platform parsers' physical-key helpers directly.
#[rustfmt::skip]
#[path = "../../../third-party/vendor/gpui-pre-macos-0.3.10/src/key_identity.rs"]
mod macos;
#[rustfmt::skip]
#[path = "../../../third-party/vendor/gpui-pre-linux-0.3.10/src/linux/key_identity.rs"]
mod linux;

#[test]
fn native_physical_keys_do_not_depend_on_layout_or_modifiers() {
    for (mac, xkb, expected) in
        [(0, 38, 'a'), (41, 47, ';'), (18, 10, '1'), (49, 65, ' ')]
    {
        assert_eq!(macos::us_key(mac), Some(expected));
        assert_eq!(linux::us_key(xkb), Some(expected));
    }
    assert_eq!(macos::us_key(123), None);
    assert_eq!(linux::us_key(113), None);
}

#[test]
fn native_layout_helper_keeps_shift_and_rejects_control_function_and_dead_keys()
{
    for (base, shifted) in [("a", "A"), (";", ":"), ("ф", "Ф"), (" ", " ")] {
        let identity =
            gpui::native_key_identity(base, shifted, true, Some('a'));
        assert_eq!(identity.unshifted, base.chars().next());
        assert_eq!(identity.shifted, shifted.chars().next());
        assert!(identity.shift);
    }
    for text in ["", "\r", "\u{f700}", "ab"] {
        assert_eq!(
            gpui::native_key_identity(text, text, false, None).unshifted,
            None
        );
    }
}
