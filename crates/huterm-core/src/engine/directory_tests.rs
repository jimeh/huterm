use super::*;

fn directory(value: &str, hostname: Option<&str>) -> Option<TerminalDirectory> {
    match normalize_directory(value, hostname) {
        DirectoryUpdate::Set(directory) => Some(directory),
        DirectoryUpdate::Ignore | DirectoryUpdate::Clear => None,
    }
}

#[test]
fn file_directories_decode_and_classify_exact_local_hosts() {
    for host in ["", "localhost", "localhost.", "WORKSTATION", "workstation."] {
        let value = format!("file://{host}/tmp/hello%20world/%E2%98%83");
        let directory = directory(&value, Some("workstation")).unwrap();
        assert_eq!(directory.path(), "/tmp/hello world/☃");
        assert!(directory.is_local(), "{host:?}");
    }
    for host in ["workstation.local", "short", "workstation.example"] {
        let value = format!("file://{host}/tmp/project");
        let directory = directory(&value, Some("workstation")).unwrap();
        assert_eq!(directory.host(), Some(host));
        assert!(!directory.is_local(), "{host:?}");
    }
}

#[test]
fn bare_paths_are_display_only_and_empty_clears() {
    let bare = directory("/tmp/project", Some("workstation")).unwrap();
    assert_eq!(bare.host(), None);
    assert_eq!(bare.path(), "/tmp/project");
    assert!(!bare.is_local());
    assert_eq!(
        normalize_directory("", Some("workstation")),
        DirectoryUpdate::Clear
    );
}

#[test]
fn malformed_unsafe_and_oversized_reports_are_rejected() {
    for value in [
        "relative/path",
        "https://localhost/tmp",
        "file://user@localhost/tmp",
        "file://localhost:22/tmp",
        "file://localhost/tmp/%gg",
        "file://localhost/tmp?query",
        "file://localhost/tmp#fragment",
        "file://localhost/tmp%00name",
    ] {
        assert_eq!(
            normalize_directory(value, Some("localhost")),
            DirectoryUpdate::Ignore,
            "{value}"
        );
    }
    assert_eq!(
        normalize_directory(&"x".repeat(METADATA_BYTE_LIMIT + 1), None),
        DirectoryUpdate::Ignore
    );
}
