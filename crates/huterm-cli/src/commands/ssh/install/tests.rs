use std::ffi::OsStr;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::process::Output;

use super::*;

/// A directory removed on drop, unique per test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir()
            .join(format!("huterm-cli-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Read-only directories from a test would stop removal.
        let _ = Command::new("chmod")
            .arg("-R")
            .arg("u+rwx")
            .arg(&self.0)
            .status();
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn candidates_prefer_an_absolute_runtime_directory() {
    let candidates = |runtime: Option<&str>, temporary: &str| {
        super::candidates(
            |name| {
                assert_eq!(name, "XDG_RUNTIME_DIR");
                runtime.map(OsString::from)
            },
            PathBuf::from(temporary),
            501,
        )
    };
    assert_eq!(
        candidates(Some("/run/user/501"), "/var/folders/x/T"),
        [
            PathBuf::from("/run/user/501/huterm-ssh-501"),
            PathBuf::from("/var/folders/x/T/huterm-ssh-501"),
            PathBuf::from("/tmp/huterm-ssh-501"),
        ]
    );
    assert_eq!(
        candidates(Some("relative"), "/tmp"),
        [PathBuf::from("/tmp/huterm-ssh-501")],
        "a relative runtime directory is ignored, and repeats collapse"
    );
    for temporary in ["", "relative/T", "/var/../T"] {
        assert_eq!(
            candidates(Some("/run/../user"), temporary),
            [PathBuf::from("/tmp/huterm-ssh-501")],
            "an empty, relative, or escaping {temporary:?} is ignored"
        );
    }
}

#[test]
fn the_first_candidate_that_fits_the_address_budget_is_chosen() {
    let accept = |_: &Path| Ok(());
    let name = "123-0123456789abcdef";
    let fitting = PathBuf::from("/tmp/huterm-ssh-501");
    let long = PathBuf::from("/").join("d".repeat(CONTROL_PATH_BUDGET));
    // The longest directory whose socket path still fits.
    let edge = PathBuf::from("/")
        .join("e".repeat(CONTROL_PATH_BUDGET - 2 - name.len()));
    assert_eq!(edge.join(name).as_os_str().len(), CONTROL_PATH_BUDGET);
    let over = PathBuf::from("/")
        .join("o".repeat(CONTROL_PATH_BUDGET - 1 - name.len()));
    assert_eq!(
        choose(&[long.clone(), fitting.clone()], name, accept),
        Ok(fitting.join(name))
    );
    assert_eq!(
        choose(std::slice::from_ref(&edge), name, accept),
        Ok(edge.join(name))
    );
    assert_eq!(
        choose(&[over, long], name, accept),
        Err(NoSocket::Ineligible)
    );
    for syntax in ["/with space", "/per%cent", "/quo\"te", "/tab\there"] {
        assert_eq!(
            choose(&[PathBuf::from(syntax)], name, accept),
            Err(NoSocket::Ineligible),
            "{syntax:?}"
        );
    }
}

#[test]
fn a_refused_directory_falls_through_to_the_next_candidate() {
    let name = "1-2";
    let first = PathBuf::from("/first");
    let second = PathBuf::from("/second");
    let refused = first.clone();
    let refuse_first = |directory: &Path| {
        if directory == refused {
            Err("unsafe".to_owned())
        } else {
            Ok(())
        }
    };
    assert_eq!(
        choose(&[first.clone(), second.clone()], name, refuse_first),
        Ok(second.join(name))
    );
    assert_eq!(
        choose(&[first], name, refuse_first),
        Err(NoSocket::Refused("unsafe".into()))
    );
}

#[test]
fn socket_directories_are_created_private_and_checked() {
    let scratch = Scratch::new("directories");
    let created = scratch.0.join("created");
    prepare(&created).unwrap();
    assert_eq!(mode(&created), 0o700);
    prepare(&created).unwrap();

    let open = scratch.0.join("open");
    std::fs::create_dir(&open).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755))
        .unwrap();
    let error = prepare(&open).unwrap_err();
    assert!(error.contains("grants group or other"), "{error}");

    let link = scratch.0.join("link");
    symlink(&created, &link).unwrap();
    let error = prepare(&link).unwrap_err();
    assert!(error.contains("symbolic link"), "{error}");

    let file = scratch.0.join("file");
    std::fs::write(&file, "").unwrap();
    let error = prepare(&file).unwrap_err();
    assert!(error.contains("not a directory"), "{error}");
}

#[test]
fn only_the_computed_acknowledgement_counts() {
    assert_eq!(acknowledgement(41), "huterm-terminfo-42");
    assert_eq!(acknowledgement(u32::MAX), "huterm-terminfo-4294967296");
    assert!(acknowledged(b"banner\nhuterm-terminfo-42\n", 41));
    for output in [
        &b""[..],
        b"huterm-terminfo-41\n",
        b"huterm-terminfo-420\n",
        b" huterm-terminfo-42\n",
        // A forced command that echoes the command it was given.
        remote_command(41).as_bytes(),
    ] {
        assert!(
            !acknowledged(output, 41),
            "{}",
            String::from_utf8_lossy(output)
        );
    }
}

#[test]
fn the_remote_command_is_one_line_any_login_shell_passes_through() {
    let command = remote_command(7);
    let script = command
        .strip_prefix("sh -c '")
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap();
    // Single quotes would end the outer quoting; fish's single quotes
    // decode `\\`; csh expands `!` even inside them.
    for forbidden in ["'", "\n", "\\\\", "!"] {
        assert!(!script.contains(forbidden), "{forbidden:?} in {script}");
    }
}

#[test]
fn control_options_name_the_socket_and_come_in_order() {
    let socket = Path::new("/tmp/huterm-ssh-501/1-2");
    assert_eq!(
        master_options(socket),
        [
            "-o",
            "ControlMaster=yes",
            "-o",
            "ControlPath=/tmp/huterm-ssh-501/1-2",
            "-o",
            "ControlPersist=10"
        ]
    );
    assert_eq!(
        session_options(socket),
        [
            "-o",
            "ControlPath=/tmp/huterm-ssh-501/1-2",
            "-o",
            "ControlMaster=no"
        ]
    );
}

/// An executable on `PATH`, or `None` after reporting a skip. With
/// `HUTERM_REQUIRE_SHELLS=1` a missing shell is a failure.
fn find(name: &str, required: bool) -> Option<PathBuf> {
    let found =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|directory| directory.join(name))
            .find(|file| file.is_file());
    if found.is_none() {
        assert!(
            !(required && std::env::var_os("HUTERM_REQUIRE_SHELLS").is_some()),
            "required shell missing: {name}"
        );
        eprintln!("skipping the remote install script under {name}: missing");
    }
    found
}

/// Runs the remote command as the outer `-c` shell would, with only the
/// given environment, `input` on standard input.
fn remote(
    shell: &Path,
    home: &Path,
    path: &OsStr,
    input: &[u8],
    nonce: u32,
) -> Output {
    let temporary = home.join("tmp");
    std::fs::create_dir_all(&temporary).unwrap();
    let mut child = Command::new(shell)
        .arg("-c")
        .arg(remote_command(nonce))
        .env_clear()
        .env("PATH", path)
        .env("HOME", home)
        .env("TMPDIR", &temporary)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A script that fails or finds the entry installed exits without
    // reading its input, so the write may meet a closed pipe; the exit
    // status, output, and files still show what the script did.
    match child.stdin.take().unwrap().write_all(input) {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
        result => result.unwrap(),
    }
    child.wait_with_output().unwrap()
}

fn installed(home: &Path) -> bool {
    ["x", "78"].iter().any(|layout| {
        home.join(".terminfo")
            .join(layout)
            .join("xterm-huterm")
            .is_file()
    })
}

fn leftover_temporaries(home: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(home.join("tmp"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

#[test]
fn the_remote_script_installs_acknowledges_and_fails_through_each_shell() {
    let Some(tic) = find("tic", false) else {
        return;
    };
    let infocmp = find("infocmp", false);
    let path = std::env::var_os("PATH").unwrap();
    let scratch = Scratch::new("script");
    // A remote without tic or infocmp still has what the script starts with.
    let bare = scratch.0.join("bare-bin");
    std::fs::create_dir_all(&bare).unwrap();
    for tool in ["sh", "cat", "mktemp", "rm", "mkdir"] {
        let file = find(tool, true).unwrap();
        symlink(file, bare.join(tool)).unwrap();
    }
    for (name, required) in
        [("bash", true), ("zsh", true), ("fish", true), ("nu", true)]
    {
        let Some(shell) = find(name, required) else {
            continue;
        };
        let home = scratch.0.join(name);
        std::fs::create_dir_all(&home).unwrap();
        if let Some(infocmp) = &infocmp
            && Command::new(infocmp)
                .arg("xterm-huterm")
                .env_clear()
                .env("HOME", &home)
                .output()
                .unwrap()
                .status
                .success()
        {
            eprintln!(
                "skipping the remote install script: the system has xterm-huterm"
            );
            return;
        }

        let output =
            remote(&shell, &home, bare.as_os_str(), SOURCE.as_bytes(), 1);
        assert!(!output.status.success(), "{name}: succeeded without tic");
        assert!(
            !acknowledged(&output.stdout, 1),
            "{name}: acknowledged a failure"
        );
        assert!(!installed(&home), "{name}");
        assert_eq!(
            leftover_temporaries(&home),
            Vec::<PathBuf>::new(),
            "{name}"
        );

        let output = remote(&shell, &home, &path, SOURCE.as_bytes(), 2);
        assert!(
            output.status.success() && acknowledged(&output.stdout, 2),
            "{name} with {}: {output:?}",
            tic.display()
        );
        assert!(installed(&home), "{name}: no compiled entry");
        assert_eq!(
            leftover_temporaries(&home),
            Vec::<PathBuf>::new(),
            "{name}"
        );

        if infocmp.is_some() {
            // An installed entry is found, so nothing is written: the
            // database is read-only and the input is not terminfo.
            let database = home.join(".terminfo");
            Command::new("chmod")
                .arg("-R")
                .arg("a-w")
                .arg(&database)
                .status()
                .unwrap();
            let output = remote(&shell, &home, &path, b"not terminfo", 3);
            assert!(
                output.status.success() && acknowledged(&output.stdout, 3),
                "{name}: {output:?}"
            );
            Command::new("chmod")
                .arg("-R")
                .arg("u+w")
                .arg(&database)
                .status()
                .unwrap();
        }
        eprintln!("remote install script ran under {name}");
    }
}
