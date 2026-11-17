//! Installing the `xterm-huterm` entry on a remote host through a control
//! master that the session then reuses, so the user authenticates once.
use std::ffi::OsString;
use std::fs::DirBuilder;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Huterm's terminfo source, which the remote `tic` compiles.
pub(crate) const SOURCE: &str =
    include_str!("../../../../../assets/terminfo/xterm-huterm.terminfo");

/// The most standard output an install keeps; it reads and discards the
/// rest.
const OUTPUT_LIMIT: u64 = 64 << 10;

/// The size of `sockaddr_un.sun_path`, its terminating NUL included.
#[cfg(any(target_os = "linux", target_os = "android"))]
const SUN_PATH_BYTES: usize = 108;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const SUN_PATH_BYTES: usize = 104;

/// The longest control path: OpenSSH first binds the path plus `.` and 16
/// random characters, and the address needs its NUL.
pub(crate) const CONTROL_PATH_BUDGET: usize = SUN_PATH_BYTES - 18;

/// The per-user directories a control socket may live in, in order.
pub(crate) fn candidates(
    environment: impl Fn(&str) -> Option<OsString>,
    temporary: PathBuf,
    uid: u32,
) -> Vec<PathBuf> {
    // An empty or relative root would put the directory under the working
    // directory, and `..` could leave the root.
    let usable = |root: &PathBuf| {
        root.is_absolute()
            && !root
                .components()
                .any(|component| component == std::path::Component::ParentDir)
    };
    let name = format!("huterm-ssh-{uid}");
    let mut roots = Vec::new();
    roots.extend(environment("XDG_RUNTIME_DIR").map(PathBuf::from));
    roots.push(temporary);
    roots.retain(usable);
    roots.push(PathBuf::from("/tmp"));
    let mut directories: Vec<PathBuf> = Vec::new();
    for root in roots {
        let directory = root.join(&name);
        if !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    directories
}

/// Why no control socket path was chosen.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum NoSocket {
    /// Every candidate path is too long or holds characters OpenSSH would
    /// read as syntax, so the install does not apply.
    Ineligible,
    /// A candidate that fits is unsafe to use.
    Refused(String),
}

/// The first candidate whose socket path fits and whose directory
/// `prepare` creates and accepts.
pub(crate) fn choose(
    candidates: &[PathBuf],
    name: &str,
    prepare: impl Fn(&Path) -> Result<(), String>,
) -> Result<PathBuf, NoSocket> {
    let mut refused = None;
    for directory in candidates {
        let socket = directory.join(name);
        let bytes = socket.as_os_str().as_encoded_bytes();
        // `%` starts an OpenSSH token, and whitespace or quotes would split
        // or quote the option value.
        if bytes.len() > CONTROL_PATH_BUDGET
            || bytes.iter().any(|byte| {
                byte.is_ascii_whitespace()
                    || byte.is_ascii_control()
                    || b"%\"'\\".contains(byte)
            })
        {
            continue;
        }
        match prepare(directory) {
            Ok(()) => return Ok(socket),
            Err(problem) => refused = refused.or(Some(problem)),
        }
    }
    Err(refused.map_or(NoSocket::Ineligible, NoSocket::Refused))
}

/// Creates `directory` with mode `0700` when it is missing, then checks
/// that it is a private directory of the effective user.
pub(crate) fn prepare(directory: &Path) -> Result<(), String> {
    match DirBuilder::new().mode(0o700).create(directory) {
        Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
            return Err(format!(
                "cannot create {}: {error}",
                directory.display()
            ));
        }
        _ => {}
    }
    huterm_socket::check_private_directory(
        directory,
        huterm_socket::effective_uid(),
    )
    .map_err(|error| error.to_string())
}

/// The line a successful install prints for `nonce`. The remote shell
/// computes it, so a command that echoes its input cannot produce it.
pub(crate) fn acknowledgement(nonce: u32) -> String {
    format!("huterm-terminfo-{}", u64::from(nonce) + 1)
}

/// The remote command: one line without single quotes, run by `sh` so a
/// login shell of any syntax passes it through. It compiles standard input
/// into `~/.terminfo` unless `infocmp` already knows the entry, and prints
/// the acknowledgement only on success.
pub(crate) fn remote_command(nonce: u32) -> String {
    let ack = format!("echo huterm-terminfo-$(({nonce}+1))");
    let script = [
        format!("if infocmp xterm-huterm >/dev/null 2>&1; then {ack}; exit 0; fi"),
        "command -v tic >/dev/null 2>&1 || exit 1".into(),
        "command -v mktemp >/dev/null 2>&1 || exit 1".into(),
        "t=$(mktemp) || exit 1".into(),
        r#"trap "rm -f -- \"\$t\"" EXIT"#.into(),
        r#"trap "exit 1" HUP INT TERM"#.into(),
        r#"cat >"$t" || exit 1"#.into(),
        r#"mkdir -p "$HOME/.terminfo" || exit 1"#.into(),
        r#"tic -x -o "$HOME/.terminfo" "$t" >/dev/null 2>&1 || exit 1"#.into(),
        format!(
            r#"if [ -e "$HOME/.terminfo/x/xterm-huterm" ] || [ -e "$HOME/.terminfo/78/xterm-huterm" ]; then {ack}; else exit 1; fi"#
        ),
    ]
    .join("; ");
    format!("sh -c '{script}'")
}

/// The options that make the install's connection Huterm's own master.
/// They come first, so they win over the user's configuration.
pub(crate) fn master_options(socket: &Path) -> Vec<OsString> {
    let mut path = OsString::from("ControlPath=");
    path.push(socket);
    vec![
        "-o".into(),
        "ControlMaster=yes".into(),
        "-o".into(),
        path,
        "-o".into(),
        "ControlPersist=10".into(),
    ]
}

/// The options that make the session reuse the master, or connect on its
/// own when the master is gone.
pub(crate) fn session_options(socket: &Path) -> Vec<OsString> {
    let mut path = OsString::from("ControlPath=");
    path.push(socket);
    vec!["-o".into(), path, "-o".into(), "ControlMaster=no".into()]
}

/// Runs the install over a new master at `socket`: `ssh` with Huterm's
/// options, the session's `forwarding` options, the user's arguments without
/// `-t`, and the remote command, fed the terminfo source. Prompts and errors
/// reach the user's terminal. Returns whether the remote host acknowledged
/// it.
///
/// A session that reuses the master sends only the variables the master's
/// own `SendEnv` allows, so the master carries the session's `SendEnv`.
pub(crate) fn run(
    socket: &Path,
    forwarding: &[OsString],
    arguments: &[OsString],
    nonce: u32,
) -> bool {
    let mut command = Command::new("ssh");
    command
        .args(master_options(socket))
        .args(forwarding)
        .args(["-T", "-o", "ClearAllForwardings=yes"])
        .args(arguments)
        .arg(remote_command(nonce))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    // The remote end may never read its input, so a closed pipe is fine.
    if let Some(mut input) = child.stdin.take() {
        std::thread::spawn(move || {
            let _ = input.write_all(SOURCE.as_bytes());
        });
    }
    let mut output = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        // Read to the end, discarding output past the limit: closing the
        // pipe early would fail or stall a remote that writes more.
        let _ = (&mut stdout).take(OUTPUT_LIMIT).read_to_end(&mut output);
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
    }
    let succeeded = child.wait().is_ok_and(|status| status.success());
    succeeded && acknowledged(&output, nonce)
}

/// Whether a line of `output` is exactly the acknowledgement for `nonce`.
pub(crate) fn acknowledged(output: &[u8], nonce: u32) -> bool {
    let expected = acknowledgement(nonce);
    output
        .split(|byte| *byte == b'\n')
        .any(|line| line == expected.as_bytes())
}

#[cfg(test)]
mod tests;
