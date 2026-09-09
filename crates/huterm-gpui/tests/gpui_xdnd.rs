//! Compile the vendored production decoder directly, without copying behavior.
#[cfg(unix)]
#[rustfmt::skip]
#[path = "../../../third-party/vendor/gpui-0.2.2/src/platform/linux/x11/xdnd_payload.rs"]
mod payload;

#[cfg(unix)]
#[test]
fn native_uri_lists_preserve_spelling_and_reject_partial_decoding() {
    use std::os::unix::ffi::OsStrExt as _;
    let paths = payload::parse_uri_list(b"# comment\r\nfile:///tmp/a%20b\r\nfile://localhost/tmp/symlink/../folder\nfile:///tmp/%FF", 0).unwrap();
    assert_eq!(paths[0].as_os_str().as_bytes(), b"/tmp/a b");
    assert_eq!(paths[1].as_os_str().as_bytes(), b"/tmp/symlink/../folder");
    assert_eq!(paths[2].as_os_str().as_bytes(), b"/tmp/\xff");
    for invalid in [
        "https://example.test/a",
        "file://remote/tmp/a",
        "file:///tmp/a?query",
        "file:///tmp/a#fragment",
        "file:///tmp/%",
        "file:///tmp/%GG",
        "file:relative",
        "file:///tmp/raw space",
    ] {
        assert!(
            payload::parse_uri_list(
                format!("file:///valid\n{invalid}").as_bytes(),
                0
            )
            .is_none(),
            "{invalid}"
        );
    }
    assert!(payload::parse_uri_list(b"file:///valid", 1).is_none());
    assert!(
        payload::parse_uri_list(
            &vec![b'a'; payload::MAX_URI_LIST_BYTES + 1],
            0
        )
        .is_none()
    );
    assert!(payload::parse_uri_list(b"# no files\n", 0).is_none());
}

#[cfg(unix)]
#[test]
fn native_conversion_identity_rejects_late_and_duplicate_replies() {
    let mut transfer = payload::Transfer::Requested(10);
    assert!(transfer.accepts_reply(10));
    transfer = payload::Transfer::Ready(10);
    assert!(transfer.ready());
    assert_eq!(transfer.requestor(), Some(10));
    assert!(!transfer.accepts_reply(10));
    transfer = payload::Transfer::Requested(11);
    assert!(!transfer.accepts_reply(10));
    assert!(transfer.accepts_reply(11));
    transfer = payload::Transfer::Rejected;
    assert!(transfer.requestor().is_none());
    assert!(!transfer.accepts_reply(11));
    transfer = payload::Transfer::default();
    assert!(!transfer.ready());
}
