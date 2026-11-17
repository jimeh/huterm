//! Runs `huterm ssh`, `huterm ssh-cache`, and `huterm sudo` as processes,
//! with stand-in `ssh` and `sudo` scripts first on `PATH` that record what
//! they receive. Nothing here connects anywhere or reads the real home.
#![cfg(unix)]

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const CLI: &str = env!("CARGO_BIN_EXE_huterm");
const SOURCE: &str =
    include_str!("../../../assets/terminfo/xterm-huterm.terminfo");

/// `ssh -G` output for an interactive session to `alice@build.example.org`.
const RESOLVED: &str = "\
user alice
hostname build.example.org
port 22
sessiontype default
forkafterauthentication no
stdinnull no
requesttty auto
";

/// The stand-in `ssh`. With `-G` it prints `resolved`, or with a `hang`
/// file records its process ID and waits forever. With `ControlMaster=yes`
/// it is an install: it records its arguments and input, then answers as
/// `install-mode` says; `flood` acknowledges, then writes 1 MiB more and
/// fails if its output is cut off.
/// Otherwise it is the session: it records its process ID, arguments,
/// environment, and, with a `read-stdin` file, its input, writes to both
/// streams, and exits 42.
const FAKE_SSH: &str = r#"#!/bin/sh
dir=${0%/*}
if [ "$1" = -G ]; then
    printf '%s\0' "$@" > "$dir/resolve.args"
    if [ -e "$dir/hang" ]; then
        echo "$$" > "$dir/resolve.pid"
        exec sleep 1000
    fi
    cat "$dir/resolved"
    exit
fi
for argument in "$@"; do
    if [ "$argument" = ControlMaster=yes ]; then
        printf '%s\0' "$@" > "$dir/install.args"
        cat > "$dir/install.stdin"
        eval "last=\${$#}"
        nonce=$(printf '%s\n' "$last" | sed 's/.*huterm-terminfo-\$((\([0-9]*\)+1)).*/\1/')
        case $(cat "$dir/install-mode") in
            ack) echo banner; echo "huterm-terminfo-$((nonce + 1))" ;;
            wrong) echo "huterm-terminfo-$nonce" ;;
            echo) printf '%s\n' "$last" ;;
            fail) echo "ssh: connect to host failed" >&2; exit 255 ;;
            flood)
                echo "huterm-terminfo-$((nonce + 1))"
                # Like ssh, fail when standard output closes early.
                head -c 1048576 /dev/zero | tr '\0' x || exit 255
                ;;
        esac
        exit 0
    fi
done
echo "$$" > "$dir/session.pid"
printf '%s\0' "$@" > "$dir/session.args"
printf '%s\0' "TERM=$TERM" "COLORTERM=$COLORTERM" > "$dir/session.env"
if [ -e "$dir/read-stdin" ]; then
    cat > "$dir/session.stdin"
fi
echo session-out
echo session-err >&2
exit 42
"#;

/// The stand-in `sudo`: records its process ID, arguments, and `TERM`.
const FAKE_SUDO: &str = r#"#!/bin/sh
dir=${0%/*}
echo "$$" > "$dir/sudo.pid"
printf '%s\0' "$@" > "$dir/sudo.args"
printf '%s' "$TERM" > "$dir/sudo.term"
exit 3
"#;

/// A scratch directory under `/tmp`, short enough for control socket
/// paths, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
        let path = PathBuf::from(format!(
            "/tmp/hcw-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        for directory in ["bin", "run", "tmp"] {
            std::fs::create_dir_all(path.join(directory)).unwrap();
        }
        let scratch = Self(path.canonicalize().unwrap());
        scratch.executable("bin/ssh", FAKE_SSH);
        scratch.executable("bin/sudo", FAKE_SUDO);
        scratch.write("bin/resolved", RESOLVED);
        scratch.write("bin/install-mode", "ack");
        scratch
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.0.join(name), text).unwrap();
    }

    fn executable(&self, name: &str, text: &str) {
        self.write(name, text);
        std::fs::set_permissions(
            self.0.join(name),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }

    fn recorded(&self, name: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join("bin").join(name)).ok()
    }

    /// Arguments a stand-in recorded, NUL-separated.
    fn arguments(&self, name: &str) -> Option<Vec<OsString>> {
        let bytes = self.recorded(name)?;
        Some(
            bytes
                .split(|byte| *byte == 0)
                .map(|part| OsString::from_vec(part.to_vec()))
                .collect::<Vec<_>>()
                .split_last()
                .map(|(_, arguments)| arguments.to_vec())
                .unwrap_or_default(),
        )
    }

    fn cache(&self) -> Option<String> {
        std::fs::read_to_string(self.0.join("state/ssh-cache")).ok()
    }

    /// `huterm` with only the scratch environment: no display, the stand-ins
    /// first on `PATH`, and scratch state and runtime directories.
    fn huterm(&self, term: &str) -> Command {
        let mut command = Command::new(CLI);
        command
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.0.join("bin").display()),
            )
            .env("HOME", self.0.join("home"))
            .env("HUTERM_STATE_DIR", self.0.join("state"))
            .env("XDG_RUNTIME_DIR", self.0.join("run"))
            .env("TMPDIR", self.0.join("tmp"))
            .env("TERM", term)
            .env("COLORTERM", "truecolor")
            .stdin(Stdio::null());
        command
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn run(command: &mut Command) -> Output {
    command.output().unwrap()
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

/// The session's `TERM`, as the stand-in recorded it.
fn session_term(scratch: &Scratch) -> String {
    let env = scratch.recorded("session.env").expect("no session started");
    let env = String::from_utf8(env).unwrap();
    env.split('\0')
        .find_map(|pair| pair.strip_prefix("TERM="))
        .unwrap()
        .to_owned()
}

/// Polls `condition` until `deadline`.
fn eventually(what: &str, timeout: Duration, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Gives `command` a pseudo-terminal as standard input, as an interactive
/// shell does. The returned primary end keeps the terminal open.
fn terminal_stdin(command: &mut Command) -> std::os::fd::OwnedFd {
    let pty = nix::pty::openpty(None, None).unwrap();
    command.stdin(Stdio::from(pty.slave));
    pty.master
}

/// Runs `command` with a pseudo-terminal as standard input.
fn run_interactive(command: &mut Command) -> Output {
    let _terminal = terminal_stdin(command);
    run(command)
}

fn alive(pid: i32) -> bool {
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

#[test]
fn ssh_passes_arguments_streams_status_and_process_through() {
    let scratch = Scratch::new();
    let raw = [
        OsString::from("--"),
        OsString::from("--help"),
        OsString::from("-h"),
        OsString::from("-V"),
        OsString::from_vec(b"h\xffst".to_vec()),
    ];
    let child = scratch
        .huterm("xterm-256color")
        .arg("ssh")
        .args(&raw)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(42), "the session's status");
    assert_eq!(text(&output.stdout), "session-out\n");
    assert_eq!(text(&output.stderr), "session-err\n");
    assert_eq!(scratch.arguments("session.args").unwrap(), raw);
    assert_eq!(
        text(&scratch.recorded("session.pid").unwrap()).trim(),
        pid.to_string(),
        "ssh replaces huterm's process"
    );
    assert_eq!(session_term(&scratch), "xterm-256color");
    assert!(
        scratch.recorded("resolve.args").is_none(),
        "another TERM needs no resolution"
    );
}

#[test]
fn ssh_downgrades_an_unknown_host_and_keeps_a_cached_one() {
    let scratch = Scratch::new();
    let output =
        run(scratch.huterm("xterm-huterm").args(["ssh", "-p22", "work"]));
    assert_eq!(output.status.code(), Some(42));
    assert_eq!(text(&output.stderr), "session-err\n", "no diagnostics");
    assert_eq!(session_term(&scratch), "xterm-256color");
    assert_eq!(
        scratch.arguments("resolve.args").unwrap(),
        args(&["-G", "-p22", "work"])
    );
    assert_eq!(
        scratch.arguments("session.args").unwrap(),
        args(&["-p22", "work"])
    );

    // The key is the resolved user and host name, not the alias.
    let added = run(scratch.huterm("xterm-huterm").args([
        "ssh-cache",
        "add",
        "alice@build.example.org",
    ]));
    assert!(added.status.success(), "{added:?}");
    run(scratch
        .huterm("xterm-huterm")
        .args(["ssh", "--terminfo", "work"]));
    assert_eq!(session_term(&scratch), "xterm-huterm");
    assert!(
        scratch.recorded("install.args").is_none(),
        "a cached host needs no install"
    );

    // Unparsed arguments and a failed resolution both downgrade.
    run(scratch.huterm("xterm-huterm").args(["ssh", "-Z", "work"]));
    assert_eq!(session_term(&scratch), "xterm-256color");
    scratch.executable(
        "bin/ssh",
        &FAKE_SSH.replace("cat \"$dir/resolved\"\n    exit", "exit 255"),
    );
    run(scratch.huterm("xterm-huterm").args(["ssh", "work"]));
    assert_eq!(session_term(&scratch), "xterm-256color");
}

#[test]
fn ssh_forwards_terminal_variables_only_when_asked() {
    let scratch = Scratch::new();
    run(scratch
        .huterm("xterm-256color")
        .args(["ssh", "work", "uptime"]));
    assert_eq!(
        scratch.arguments("session.args").unwrap(),
        args(&["work", "uptime"])
    );
    run(scratch.huterm("xterm-256color").args([
        "ssh",
        "--forward-env",
        "work",
        "uptime",
    ]));
    assert_eq!(
        scratch.arguments("session.args").unwrap(),
        args(&[
            "-o",
            "SendEnv=COLORTERM",
            "-o",
            "SendEnv=TERM_PROGRAM",
            "-o",
            "SendEnv=TERM_PROGRAM_VERSION",
            "work",
            "uptime"
        ])
    );
}

#[test]
fn ssh_install_success_caches_the_host_and_reuses_the_master() {
    let scratch = Scratch::new();
    let output = run_interactive(scratch.huterm("xterm-huterm").args([
        "ssh",
        "--forward-env",
        "--terminfo",
        "-At",
        "work",
        "-t",
    ]));
    assert_eq!(output.status.code(), Some(42));
    assert_eq!(
        text(&output.stderr),
        "huterm: installing the xterm-huterm terminfo entry on \
         alice@build.example.org\nsession-err\n"
    );
    assert_eq!(session_term(&scratch), "xterm-huterm");
    assert_eq!(
        scratch.recorded("install.stdin").unwrap(),
        SOURCE.as_bytes()
    );

    let install = scratch.arguments("install.args").unwrap();
    let socket = install[3]
        .to_str()
        .unwrap()
        .strip_prefix("ControlPath=")
        .unwrap()
        .to_owned();
    let directory = Path::new(&socket).parent().unwrap();
    assert_eq!(
        directory,
        scratch
            .0
            .join("run")
            .join(format!("huterm-ssh-{}", nix::unistd::geteuid())),
    );
    assert_eq!(
        std::fs::metadata(directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let (command, rest) = install.split_last().unwrap();
    assert_eq!(
        rest,
        args(&[
            "-o",
            "ControlMaster=yes",
            "-o",
            &format!("ControlPath={socket}"),
            "-o",
            "ControlPersist=10",
            "-o",
            "SendEnv=COLORTERM",
            "-o",
            "SendEnv=TERM_PROGRAM",
            "-o",
            "SendEnv=TERM_PROGRAM_VERSION",
            "-T",
            "-o",
            "ClearAllForwardings=yes",
            "-A",
            "work",
        ])
    );
    assert!(command.as_bytes().starts_with(b"sh -c '"), "{command:?}");

    assert_eq!(
        scratch.arguments("session.args").unwrap(),
        args(&[
            "-o",
            "SendEnv=COLORTERM",
            "-o",
            "SendEnv=TERM_PROGRAM",
            "-o",
            "SendEnv=TERM_PROGRAM_VERSION",
            "-o",
            &format!("ControlPath={socket}"),
            "-o",
            "ControlMaster=no",
            "-At",
            "work",
            "-t",
        ])
    );
    let cache = scratch.cache().unwrap();
    assert!(cache.starts_with("alice@build.example.org\t"), "{cache}");
}

#[test]
fn ssh_install_failures_warn_and_downgrade() {
    for mode in ["wrong", "echo", "fail"] {
        let scratch = Scratch::new();
        scratch.write("bin/install-mode", mode);
        let output = run_interactive(scratch.huterm("xterm-huterm").args([
            "ssh",
            "--terminfo",
            "work",
        ]));
        assert_eq!(output.status.code(), Some(42), "{mode}");
        let stderr = text(&output.stderr);
        assert!(
            stderr.contains(
                "huterm: could not install the xterm-huterm terminfo entry on \
                 alice@build.example.org; using xterm-256color\n"
            ),
            "{mode}: {stderr}"
        );
        assert!(scratch.recorded("install.args").is_some(), "{mode}");
        assert_eq!(session_term(&scratch), "xterm-256color", "{mode}");
        assert_eq!(
            scratch.arguments("session.args").unwrap(),
            args(&["work"]),
            "{mode}: no control options"
        );
        assert_eq!(scratch.cache(), None, "{mode}");
    }
}

#[test]
fn ssh_installs_only_before_an_interactive_session() {
    let scratch = Scratch::new();
    run_interactive(scratch.huterm("xterm-huterm").args([
        "ssh",
        "--terminfo",
        "work",
        "uptime",
    ]));
    assert!(
        scratch.recorded("install.args").is_none(),
        "a remote command"
    );
    scratch.write(
        "bin/resolved",
        &RESOLVED.replace("stdinnull no", "stdinnull yes"),
    );
    run_interactive(scratch.huterm("xterm-huterm").args([
        "ssh",
        "--terminfo",
        "work",
    ]));
    assert!(
        scratch.recorded("install.args").is_none(),
        "an effective -n"
    );
    assert_eq!(session_term(&scratch), "xterm-256color");
}

#[test]
fn ssh_installs_nothing_when_the_session_requests_no_terminal() {
    // OpenSSH prints `requesttty false` for both `-T` and `RequestTTY=no`.
    for arguments in [&["-T", "work"][..], &["-o", "RequestTTY=no", "work"]] {
        let scratch = Scratch::new();
        scratch.write(
            "bin/resolved",
            &RESOLVED.replace("requesttty auto", "requesttty false"),
        );
        let output = run_interactive(
            scratch
                .huterm("xterm-huterm")
                .args(["ssh", "--terminfo"])
                .args(arguments),
        );
        assert_eq!(output.status.code(), Some(42), "{arguments:?}");
        assert!(
            scratch.recorded("install.args").is_none(),
            "{arguments:?}: {:?}",
            text(&output.stderr)
        );
        assert_eq!(session_term(&scratch), "xterm-256color", "{arguments:?}");
        assert_eq!(
            scratch.arguments("session.args").unwrap(),
            args(arguments),
            "{arguments:?}"
        );
    }
}

#[test]
fn ssh_passes_piped_input_to_the_session_without_an_install() {
    let scratch = Scratch::new();
    scratch.write("bin/read-stdin", "");
    let input = b"echo one\n\0\xff two\n";
    let mut child = scratch
        .huterm("xterm-huterm")
        .args(["ssh", "--terminfo", "work"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(42));
    assert_eq!(text(&output.stderr), "session-err\n", "no install");
    assert!(scratch.recorded("install.args").is_none());
    assert_eq!(scratch.recorded("session.stdin").unwrap(), input);
    assert_eq!(session_term(&scratch), "xterm-256color");
}

#[test]
fn ssh_terminfo_without_openssh_8_7_settings_says_so_and_downgrades() {
    for missing in [
        "sessiontype default\n",
        "stdinnull no\n",
        "forkafterauthentication no\n",
    ] {
        let scratch = Scratch::new();
        scratch.write("bin/resolved", &RESOLVED.replace(missing, ""));
        let output = run_interactive(scratch.huterm("xterm-huterm").args([
            "ssh",
            "--terminfo",
            "work",
        ]));
        assert_eq!(output.status.code(), Some(42), "{missing:?}");
        assert_eq!(
            text(&output.stderr),
            "huterm: ssh_terminfo needs OpenSSH 8.7 or later; using \
             xterm-256color\nsession-err\n",
            "{missing:?}"
        );
        assert!(scratch.recorded("install.args").is_none(), "{missing:?}");
        assert_eq!(session_term(&scratch), "xterm-256color", "{missing:?}");
    }
}

#[test]
fn ssh_install_reads_past_large_output_without_hanging() {
    let scratch = Scratch::new();
    scratch.write("bin/install-mode", "flood");
    let mut command = scratch.huterm("xterm-huterm");
    command
        .args(["ssh", "--terminfo", "work"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    let _terminal = terminal_stdin(&mut command);
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let group =
                nix::unistd::Pid::from_raw(i32::try_from(child.id()).unwrap());
            let _ = nix::sys::signal::killpg(
                group,
                nix::sys::signal::Signal::SIGKILL,
            );
            let _ = child.wait();
            panic!(
                "the install hung on its output; session started: {}",
                scratch.recorded("session.args").is_some()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(42));
    assert_eq!(session_term(&scratch), "xterm-huterm");
    let cache = scratch.cache().unwrap();
    assert!(cache.starts_with("alice@build.example.org\t"), "{cache}");
}

#[test]
fn ssh_kills_a_hanging_resolution_and_downgrades() {
    let scratch = Scratch::new();
    scratch.write("bin/hang", "");
    let started = Instant::now();
    let output = run_interactive(scratch.huterm("xterm-huterm").args([
        "ssh",
        "--terminfo",
        "work",
    ]));
    assert_eq!(output.status.code(), Some(42));
    assert!(
        started.elapsed() >= Duration::from_secs(10),
        "waited for the deadline"
    );
    assert_eq!(session_term(&scratch), "xterm-256color");
    let pid: i32 = text(&scratch.recorded("resolve.pid").unwrap())
        .trim()
        .parse()
        .unwrap();
    assert!(!alive(pid), "the resolution was killed and reaped");
}

#[test]
fn ssh_interrupted_during_resolution_stops_everything() {
    let scratch = Scratch::new();
    scratch.write("bin/hang", "");
    let mut child = scratch
        .huterm("xterm-huterm")
        .args(["ssh", "--terminfo", "work"])
        .process_group(0)
        .spawn()
        .unwrap();
    eventually("the resolution", Duration::from_secs(10), || {
        scratch
            .recorded("resolve.pid")
            .is_some_and(|pid| pid.ends_with(b"\n"))
    });
    let pid: i32 = text(&scratch.recorded("resolve.pid").unwrap())
        .trim()
        .parse()
        .unwrap();
    let group = nix::unistd::Pid::from_raw(i32::try_from(child.id()).unwrap());
    nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGINT).unwrap();
    let status = child.wait().unwrap();
    assert_eq!(
        status.signal(),
        Some(nix::sys::signal::Signal::SIGINT as i32),
        "{status:?}"
    );
    eventually("the resolution to stop", Duration::from_secs(10), || {
        !alive(pid)
    });
    assert!(
        scratch.recorded("session.args").is_none(),
        "no session started"
    );
}

#[test]
fn ssh_cache_operations_through_the_binary() {
    let scratch = Scratch::new();
    let cache = |arguments: &[&str]| {
        run(scratch
            .huterm("xterm-256color")
            .arg("ssh-cache")
            .args(arguments))
    };
    let output = cache(&["list"]);
    assert!(output.status.success());
    assert_eq!(text(&output.stdout), "");

    assert!(cache(&["add", "b@h", "a@h"]).status.success());
    let output = cache(&["list"]);
    let listed = text(&output.stdout);
    let lines: Vec<_> = listed.lines().collect();
    assert_eq!(lines.len(), 2, "{listed}");
    assert!(
        lines[0].starts_with("a@h\t") && lines[1].starts_with("b@h\t"),
        "{listed}"
    );
    assert!(
        lines[0].ends_with('Z') && lines[0].len() == "a@h\t".len() + 20,
        "{listed}"
    );

    let output = cache(&["remove", "a@h", "x@h"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "huterm: x@h is not in the SSH cache\n"
    );
    assert!(scratch.cache().unwrap().starts_with("b@h\t"));

    scratch.write("state/ssh-cache", "old@h\t0\nb@h\t99999999999\n");
    assert!(cache(&["prune", "1w"]).status.success());
    assert_eq!(scratch.cache().unwrap(), "b@h\t99999999999\n");

    assert!(cache(&["clear"]).status.success());
    assert_eq!(scratch.cache().unwrap(), "");

    for bad in [
        &["add", "nohost"][..],
        &["remove", "a b@h"],
        &["prune", "5"],
    ] {
        let output = cache(bad);
        assert_eq!(output.status.code(), Some(2), "{bad:?}");
        assert!(text(&output.stderr).starts_with("error: "), "{bad:?}");
    }
}

#[test]
fn sudo_passes_arguments_through_and_downgrades_only_xterm_huterm() {
    // The host's own terminfo decides whether xterm-huterm keeps its TERM;
    // no development host or CI runner installs it system-wide.
    let raw = [
        OsString::from("--help"),
        OsString::from("--"),
        OsString::from_vec(b"\xff".to_vec()),
    ];
    for (term, expected) in [
        ("xterm-huterm", "xterm-256color"),
        ("xterm-256color", "xterm-256color"),
        ("screen", "screen"),
    ] {
        let scratch = Scratch::new();
        let child =
            scratch.huterm(term).arg("sudo").args(&raw).spawn().unwrap();
        let pid = child.id();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(3), "{term}");
        assert_eq!(scratch.arguments("sudo.args").unwrap(), raw, "{term}");
        assert_eq!(
            text(&scratch.recorded("sudo.term").unwrap()),
            expected,
            "{term}"
        );
        assert_eq!(
            text(&scratch.recorded("sudo.pid").unwrap()).trim(),
            pid.to_string(),
            "{term}: sudo replaces huterm's process"
        );
    }
}

#[test]
fn a_missing_program_is_a_failure() {
    let scratch = Scratch::new();
    std::fs::remove_file(scratch.0.join("bin/ssh")).unwrap();
    std::fs::remove_file(scratch.0.join("bin/sudo")).unwrap();
    for program in ["ssh", "sudo"] {
        let output = run(scratch
            .huterm("xterm-256color")
            .env("PATH", scratch.0.join("bin"))
            .arg(program));
        assert_eq!(output.status.code(), Some(1), "{program}");
        let stderr = text(&output.stderr);
        assert!(
            stderr.starts_with(&format!("huterm: cannot run {program}: ")),
            "{stderr}"
        );
    }
}
