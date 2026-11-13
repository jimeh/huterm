use std::os::unix::fs::symlink;

use super::testing::TestDirectory;
use super::*;

const UID: u32 = 501;

fn resolve(
    name: &str,
    pairs: &[(&str, &str)],
    xdg_owner: Option<u32>,
    xdg_runtime: bool,
) -> Result<SocketPath, SocketError> {
    resolve_in(
        name,
        |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, value)| OsString::from(value))
        },
        UID,
        |_| xdg_owner,
        xdg_runtime,
    )
}

fn resolved(
    name: &str,
    pairs: &[(&str, &str)],
    xdg_owner: Option<u32>,
    xdg_runtime: bool,
) -> PathBuf {
    resolve(name, pairs, xdg_owner, xdg_runtime)
        .unwrap()
        .path()
        .to_path_buf()
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
}

#[test]
fn the_override_wins_then_an_owned_xdg_runtime_dir_on_linux_then_tmp() {
    let all = [
        (SOCKET_DIR_VARIABLE, "/override"),
        ("XDG_RUNTIME_DIR", "/run/user/501"),
    ];
    assert_eq!(
        resolved("default", &all, Some(UID), true),
        PathBuf::from("/override/default")
    );
    assert_eq!(
        resolved("work", &all[1..], Some(UID), true),
        PathBuf::from("/run/user/501/huterm/work")
    );
    let tmp = PathBuf::from("/tmp/huterm-501/default");
    // Not on macOS, not owned by the user, not absolute, or empty.
    assert_eq!(resolved("default", &all[1..], Some(UID), false), tmp);
    assert_eq!(resolved("default", &all[1..], Some(0), true), tmp);
    assert_eq!(resolved("default", &all[1..], None, true), tmp);
    assert_eq!(
        resolved("default", &[("XDG_RUNTIME_DIR", "run")], Some(UID), true),
        tmp
    );
    assert_eq!(
        resolved(
            "default",
            &[(SOCKET_DIR_VARIABLE, ""), ("XDG_RUNTIME_DIR", "")],
            Some(UID),
            true
        ),
        tmp
    );
    let socket = resolve("default", &[], None, true).unwrap();
    assert_eq!(socket.directory(), Path::new("/tmp/huterm-501"));
}

#[test]
fn invalid_names_and_a_relative_override_are_refused() {
    let longest = format!("a{}", "b".repeat(MAX_SOCKET_NAME_BYTES - 1));
    for name in ["default", "A.b_c-9", longest.as_str()] {
        assert!(resolve(name, &[], None, false).is_ok(), "{name}");
    }
    let too_long = format!("{longest}c");
    for name in [
        "",
        ".hidden",
        "-dash",
        "a/b",
        "..",
        "a b",
        too_long.as_str(),
    ] {
        assert!(
            matches!(
                resolve(name, &[], None, false),
                Err(SocketError::InvalidName(_))
            ),
            "{name}"
        );
    }
    assert!(matches!(
        resolve("default", &[(SOCKET_DIR_VARIABLE, "sockets")], None, false),
        Err(SocketError::InvalidPath(_))
    ));
    for path in ["relative/socket", "/"] {
        assert!(
            matches!(
                SocketPath::explicit(path),
                Err(SocketError::InvalidPath(_))
            ),
            "{path}"
        );
    }
}

#[test]
fn a_path_that_would_overflow_sun_path_fails_with_a_clear_error() {
    let fits = format!("/{}", "d".repeat(MAX_SOCKET_PATH_BYTES - 1 - 2));
    let socket = resolve("s", &[(SOCKET_DIR_VARIABLE, &fits)], None, false)
        .expect("a path of exactly the limit fits");
    assert_eq!(socket.path().as_os_str().len(), MAX_SOCKET_PATH_BYTES);

    let over = format!("{fits}d");
    let error =
        resolve("s", &[(SOCKET_DIR_VARIABLE, &over)], None, false).unwrap_err();
    let SocketError::PathTooLong { path, limit } = &error else {
        panic!("expected PathTooLong, got {error:?}");
    };
    assert_eq!(path.as_os_str().len(), MAX_SOCKET_PATH_BYTES + 1);
    assert_eq!(*limit, MAX_SOCKET_PATH_BYTES);
    assert!(error.to_string().contains("longer than"), "{error}");
    assert!(matches!(
        SocketPath::explicit(format!("{over}/s")),
        Err(SocketError::PathTooLong { .. })
    ));
}

#[test]
fn a_shared_symlinked_foreign_or_non_directory_socket_directory_is_refused() {
    let root = TestDirectory::new();
    let uid = effective_uid();
    let private = root.path().join("private");
    DirBuilder::new().mode(0o700).create(&private).unwrap();
    check_private_directory(&private, uid).unwrap();

    for mode in [0o750, 0o705, 0o710, 0o701] {
        set_mode(&private, mode);
        assert!(
            matches!(
                check_private_directory(&private, uid),
                Err(SocketError::UnsafeDirectory {
                    problem: DirectoryProblem::Permissions { mode: m },
                    ..
                }) if m == mode
            ),
            "{mode:o}"
        );
    }
    set_mode(&private, 0o700);

    // Another user's directory, seen as that user would see this one.
    assert!(matches!(
        check_private_directory(&private, uid + 1),
        Err(SocketError::UnsafeDirectory {
            problem: DirectoryProblem::Owner { .. },
            ..
        })
    ));

    let link = root.path().join("link");
    symlink(&private, &link).unwrap();
    assert!(matches!(
        check_private_directory(&link, uid),
        Err(SocketError::UnsafeDirectory {
            problem: DirectoryProblem::Symlink,
            ..
        })
    ));

    let file = root.path().join("file");
    fs::write(&file, b"").unwrap();
    assert!(matches!(
        check_private_directory(&file, uid),
        Err(SocketError::UnsafeDirectory {
            problem: DirectoryProblem::NotADirectory,
            ..
        })
    ));
}

#[test]
fn bind_creates_a_private_directory_and_a_0600_socket_that_it_removes() {
    let root = TestDirectory::new();
    let socket =
        SocketPath::explicit(root.path().join("sockets").join("default"))
            .unwrap();
    let (listener, file) = bind(&socket).unwrap();
    let directory = fs::symlink_metadata(socket.directory()).unwrap();
    assert!(directory.is_dir());
    assert_eq!(directory.mode() & 0o7777, 0o700);
    assert_eq!(
        fs::symlink_metadata(socket.path()).unwrap().mode() & 0o777,
        0o600
    );

    let client = connect(&socket).unwrap();
    let (accepted, _) = listener.accept().unwrap();
    check_peer(&accepted, effective_uid()).unwrap();
    check_peer(&client, effective_uid()).unwrap();
    assert!(matches!(
        check_peer(&accepted, effective_uid() + 1),
        Err(SocketError::ForeignPeer { .. })
    ));

    // A second server cannot bind over a live socket.
    assert!(matches!(
        bind(&socket),
        Err(SocketError::Io {
            action: "bind socket",
            ..
        })
    ));
    assert!(file.remove().unwrap());
    assert!(!socket.path().exists());
}

#[test]
fn removing_leaves_a_socket_another_server_bound_since() {
    let root = TestDirectory::new();
    let socket = root.socket("default");
    let (first, file) = bind(&socket).unwrap();
    drop(first);
    fs::remove_file(socket.path()).unwrap();
    let (_second, successor) = bind(&socket).unwrap();
    assert!(!file.remove().unwrap());
    assert!(socket.path().exists());
    assert!(successor.remove().unwrap());
}

#[test]
fn bind_and_connect_refuse_a_directory_that_is_not_private() {
    let root = TestDirectory::new();
    let socket = root.socket("default");
    let (_listener, file) = bind(&socket).unwrap();
    set_mode(root.path(), 0o755);
    assert!(matches!(
        connect(&socket),
        Err(SocketError::UnsafeDirectory { .. })
    ));
    let other = root.socket("other");
    assert!(matches!(
        bind(&other),
        Err(SocketError::UnsafeDirectory { .. })
    ));
    assert!(!other.path().exists());
    set_mode(root.path(), 0o700);
    connect(&socket).unwrap();
    file.remove().unwrap();
}
