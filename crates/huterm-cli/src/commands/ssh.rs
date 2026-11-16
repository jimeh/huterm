//! `huterm ssh`: runs the system `ssh` with a `TERM` the remote host knows.
//!
//! When `TERM` is `xterm-huterm` and the destination is not in the SSH
//! cache, the session uses `TERM=xterm-256color`, or, with `--terminfo`,
//! first installs the entry through a control master the session reuses.
//! `--forward-env` sends `COLORTERM`, `TERM_PROGRAM`, and
//! `TERM_PROGRAM_VERSION` with `SendEnv`, which accumulates with the user's
//! own `SendEnv` lines, where `-o SetEnv` would replace theirs.
use std::ffi::OsString;
use std::io::Read as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::ssh_cache::{Key, Store};
use super::{COMPATIBILITY_TERM, exec, term_is_huterm};

mod effective;
mod grammar;
mod install;

use effective::Effective;

/// How long `ssh -G` may take: `Match exec` and canonicalization can run
/// commands or block.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(10);

/// The most `ssh -G` output Huterm reads.
const RESOLVE_LIMIT: u64 = 1 << 20;

/// The variables `--forward-env` sends.
const FORWARDED: [&str; 3] =
    ["COLORTERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION"];

/// `huterm ssh`'s help. Its arguments bypass clap, so `--`, `--help`, and
/// every other argument after Huterm's own options reach ssh unchanged.
#[derive(Debug, clap::Args)]
pub(crate) struct Help {
    #[arg(
        long = "forward-env",
        help = "Send COLORTERM, TERM_PROGRAM, and TERM_PROGRAM_VERSION with \
                SendEnv; the remote sshd must accept them"
    )]
    _forward_env: bool,
    #[arg(
        long = "terminfo",
        help = "Install the xterm-huterm terminfo entry in ~/.terminfo on a \
                host that is not in the SSH cache, before an interactive \
                session"
    )]
    _terminfo: bool,
    /// Arguments for ssh, passed through unchanged
    #[arg(
        value_name = "SSH_ARGUMENTS",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    _arguments: Vec<OsString>,
}

/// Huterm's options, recognized only before the first other argument.
#[derive(Debug, Default, Eq, PartialEq)]
struct Options {
    forward_env: bool,
    terminfo: bool,
}

fn split(arguments: &[OsString]) -> (Options, &[OsString]) {
    let mut options = Options::default();
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        match argument.to_str() {
            Some("--forward-env") => options.forward_env = true,
            Some("--terminfo") => options.terminfo = true,
            _ => break,
        }
        index += 1;
    }
    (options, &arguments[index..])
}

/// What the session's `TERM` and connection become.
#[derive(Debug, Eq, PartialEq)]
enum Session {
    /// Keep `TERM`; reuse Huterm's master at the path when there is one.
    Keep(Option<PathBuf>),
    /// Use `TERM=xterm-256color`.
    Downgrade,
}

/// Replaces this process with `ssh` and returns only on failure.
pub(crate) fn run(arguments: &[OsString]) -> anyhow::Error {
    let (options, arguments) = split(arguments);
    let mut forwarding: Vec<OsString> = Vec::new();
    if options.forward_env {
        for name in FORWARDED {
            forwarding.push("-o".into());
            forwarding.push(format!("SendEnv={name}").into());
        }
    }
    let mut command = Command::new("ssh");
    command.args(&forwarding);
    if term_is_huterm() {
        match session(&options, &forwarding, arguments) {
            Session::Keep(Some(socket)) => {
                command.args(install::session_options(&socket));
            }
            Session::Keep(None) => {}
            Session::Downgrade => {
                command.env("TERM", COMPATIBILITY_TERM);
            }
        }
    }
    command.args(arguments);
    exec(&mut command)
}

/// Decides the session for a `TERM=xterm-huterm` shell, installing first
/// when `--terminfo` allows it.
fn session(
    options: &Options,
    forwarding: &[OsString],
    arguments: &[OsString],
) -> Session {
    let Some(parsed) = grammar::parse(arguments) else {
        return Session::Downgrade;
    };
    let Some(effective) = resolve(arguments) else {
        return Session::Downgrade;
    };
    let Some(key) = effective.key() else {
        return Session::Downgrade;
    };
    // An unsafe cache counts as empty and is never written.
    let store = Store::locate().ok();
    let entries = store.as_ref().and_then(|store| store.load().ok());
    if entries
        .as_ref()
        .is_some_and(|entries| entries.contains_key(&key))
    {
        return Session::Keep(None);
    }
    if !options.terminfo || !parsed.installable() || !effective.interactive() {
        return Session::Downgrade;
    }
    let uid = huterm_socket::effective_uid();
    let candidates = install::candidates(
        |name| std::env::var_os(name),
        std::env::temp_dir(),
        uid,
    );
    let name = format!(
        "{}-{:08x}",
        std::process::id(),
        super::random() & 0xffff_ffff
    );
    let socket = match install::choose(&candidates, &name, install::prepare) {
        Ok(socket) => socket,
        Err(install::NoSocket::Ineligible) => return Session::Downgrade,
        Err(install::NoSocket::Refused(problem)) => {
            eprintln!("huterm: {problem}");
            failed(&key);
            return Session::Downgrade;
        }
    };
    eprintln!("huterm: installing the xterm-huterm terminfo entry on {key}");
    let nonce = u32::try_from(super::random() >> 33).unwrap_or(0);
    if !install::run(&socket, forwarding, parsed.without_tty(), nonce) {
        failed(&key);
        return Session::Downgrade;
    }
    if let (Some(store), Some(mut entries)) = (store, entries) {
        entries.insert(key.clone(), super::ssh_cache::now());
        if let Err(error) = store.save(&entries) {
            eprintln!(
                "huterm: could not record {key} in the SSH cache: {error:#}"
            );
        }
    }
    Session::Keep(Some(socket))
}

fn failed(key: &Key) {
    eprintln!(
        "huterm: could not install the xterm-huterm terminfo entry on {key}; \
         using xterm-256color"
    );
}

/// Runs `ssh -G` with the user's arguments, without a terminal or error
/// output, and reads the configuration it prints. Returns `None` when it
/// fails, prints too much, or runs past [`RESOLVE_TIMEOUT`]; then it is
/// killed and reaped.
fn resolve(arguments: &[OsString]) -> Option<Effective> {
    let deadline = Instant::now() + RESOLVE_TIMEOUT;
    let mut child = Command::new("ssh")
        .arg("-G")
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let (sender, receiver) = mpsc::channel();
    // A descendant can hold the pipe open after the child exits, so the
    // reader is never joined; exec ends it.
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let result = stdout.take(RESOLVE_LIMIT + 1).read_to_end(&mut output);
        let _ = sender.send(result.map(|_| output));
    });
    let output = receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()
        .and_then(Result::ok);
    let status = output.as_ref().and_then(|_| wait(&mut child, deadline));
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    let output = output.filter(|output| {
        status.is_some_and(|status| status.success())
            && u64::try_from(output.len())
                .is_ok_and(|length| length <= RESOLVE_LIMIT)
    })?;
    Some(Effective::parse(&String::from_utf8_lossy(&output)))
}

/// Waits for `child` until `deadline`.
fn wait(
    child: &mut std::process::Child,
    deadline: Instant,
) -> Option<std::process::ExitStatus> {
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn huterm_options_lead_and_everything_after_passes_through() {
        for (values, forward_env, terminfo, rest) in [
            (&["host"][..], false, false, &["host"][..]),
            (
                &["--forward-env", "--terminfo", "-v", "host"],
                true,
                true,
                &["-v", "host"],
            ),
            (
                &["--terminfo", "--", "--forward-env"],
                false,
                true,
                &["--", "--forward-env"],
            ),
            (
                &["host", "--terminfo"],
                false,
                false,
                &["host", "--terminfo"],
            ),
            (
                &["--forward-env=yes", "host"],
                false,
                false,
                &["--forward-env=yes", "host"],
            ),
            (&["--help"], false, false, &["--help"]),
        ] {
            let values = args(values);
            let (options, remaining) = split(&values);
            assert_eq!(
                options,
                Options {
                    forward_env,
                    terminfo
                },
                "{values:?}"
            );
            assert_eq!(remaining, args(rest), "{values:?}");
        }
    }
}
