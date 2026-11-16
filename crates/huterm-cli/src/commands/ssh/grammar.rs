//! OpenSSH's command-line grammar, as `ssh.c` parses it.
//!
//! The option letters follow ssh(1) in OpenSSH 10.2 (`getopt` string
//! `1246ab:c:e:fgi:kl:m:no:p:qstvxAB:CD:E:F:GI:J:KL:MNO:P:Q:R:S:TVw:W:XYy`).
//! OpenSSH uses BSD `getopt`, which stops at the first non-option, and then
//! parses options again after the destination unless `--` came before it.
//! The first non-option after the destination, or whatever follows a `--`
//! there, starts the remote command.
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};

/// Options that take no value.
const FLAGS: &[u8] = b"1246AaCfGgKkMNnqsTtVvXxYy";

/// Options that take a value: the rest of the cluster or the next argument.
const VALUES: &[u8] = b"BbcDEeFIiJLlmOoPpQRSWw";

/// Options that rule out an install: they forward or query instead of
/// starting a session (`-W`, `-O`, `-G`, `-V`, `-Q`), or they would
/// override Huterm's control options (`-S`, `-M`).
const NOT_INSTALLABLE: &[u8] = b"WOGVQSM";

/// A command line that OpenSSH would accept.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Parsed {
    /// Every option letter, in order.
    options: Vec<u8>,
    /// Whether a remote command follows the destination.
    command: bool,
    /// The arguments without any `-t`.
    without_tty: Vec<OsString>,
}

impl Parsed {
    /// Whether the command line, before any configuration applies, starts
    /// a session an install may precede.
    pub(crate) fn installable(&self) -> bool {
        !self.command
            && !self
                .options
                .iter()
                .any(|option| NOT_INSTALLABLE.contains(option))
    }

    /// The arguments with every `-t` removed, from clusters too.
    pub(crate) fn without_tty(&self) -> &[OsString] {
        &self.without_tty
    }
}

/// Parses `arguments` as ssh would, or returns `None` for an unknown
/// option, a missing value, or a missing destination.
pub(crate) fn parse(arguments: &[OsString]) -> Option<Parsed> {
    let mut parsed = Parsed {
        options: Vec::new(),
        command: false,
        without_tty: Vec::new(),
    };
    let mut index = 0;
    let mut destination = false;
    loop {
        let mut terminated = false;
        while let Some(argument) = arguments.get(index) {
            let bytes = argument.as_bytes();
            if bytes == b"--" {
                parsed.without_tty.push(argument.clone());
                index += 1;
                terminated = true;
                break;
            }
            if bytes.len() < 2 || bytes[0] != b'-' {
                break;
            }
            let mut kept = vec![b'-'];
            let mut position = 1;
            let mut detached = false;
            while let Some(&letter) = bytes.get(position) {
                parsed.options.push(letter);
                if FLAGS.contains(&letter) {
                    if letter != b't' {
                        kept.push(letter);
                    }
                    position += 1;
                } else if VALUES.contains(&letter) {
                    kept.extend_from_slice(&bytes[position..]);
                    detached = position + 1 == bytes.len();
                    break;
                } else {
                    return None;
                }
            }
            if kept.len() > 1 {
                parsed.without_tty.push(OsString::from_vec(kept));
            }
            index += 1;
            if detached {
                parsed.without_tty.push(arguments.get(index)?.clone());
                index += 1;
            }
        }
        if destination {
            break;
        }
        parsed.without_tty.push(arguments.get(index)?.clone());
        index += 1;
        destination = true;
        if terminated {
            break;
        }
    }
    parsed.command = index < arguments.len();
    parsed
        .without_tty
        .extend_from_slice(&arguments[index.min(arguments.len())..]);
    Some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn parsed(values: &[&str]) -> Parsed {
        parse(&args(values)).unwrap_or_else(|| panic!("{values:?}"))
    }

    #[test]
    fn clusters_and_values_attached_or_detached() {
        let line = parsed(&["-4p22", "-vvv", "-o", "User=me", "-iid", "host"]);
        assert_eq!(line.options, b"4pvvvoi");
        assert!(!line.command);
        // A value never counts as options, wherever it is.
        let line = parsed(&["-l", "-t", "-pt", "host"]);
        assert_eq!(line.options, b"lp");
        assert_eq!(line.without_tty, args(&["-l", "-t", "-pt", "host"]));
    }

    #[test]
    fn options_after_the_destination_and_the_command_start() {
        let line = parsed(&["host", "-p", "22", "-N"]);
        assert_eq!(line.options, b"pN");
        assert!(!line.command);
        let line = parsed(&["-A", "host", "-v", "uptime", "-W", "x"]);
        assert_eq!(
            line.options, b"Av",
            "options after the command are its own"
        );
        assert!(line.command);
        assert!(parsed(&["host", "ls"]).command);
        assert!(parsed(&["host", "-"]).command, "a lone - is the command");
    }

    #[test]
    fn double_dashes_end_options() {
        // Before the destination, options stop for good.
        let line = parsed(&["-v", "--", "host", "-p", "22"]);
        assert_eq!(line.options, b"v");
        assert!(line.command);
        // After it, what follows is the command.
        let line = parsed(&["host", "--", "-p"]);
        assert_eq!(line.options, b"");
        assert!(line.command);
        assert!(!parsed(&["host", "--"]).command, "an empty command");
        assert!(!parsed(&["--", "-host"]).command, "a destination after --");
    }

    #[test]
    fn unknown_options_missing_values_and_destinations_are_unparsed() {
        for values in [
            &["-Z", "host"][..],
            &["-vZ", "host"],
            &["--help"],
            &["host", "-p"],
            &["-p"],
            &[],
            &["-v"],
            &["--"],
        ] {
            assert!(parse(&args(values)).is_none(), "{values:?}");
        }
    }

    #[test]
    fn interactive_lines_are_installable_and_the_rest_are_not() {
        for values in [&["host"][..], &["-tt", "host", "-A"], &["-p22", "host"]]
        {
            assert!(parsed(values).installable(), "{values:?}");
        }
        for values in [
            &["host", "true"][..],
            &["-W", "h:22", "host"],
            &["-O", "check", "host"],
            &["-G", "host"],
            &["-V", "host"],
            &["-Q", "cipher", "host"],
            &["-S", "/sock", "host"],
            &["-AM", "host"],
            &["host", "-M"],
        ] {
            assert!(!parsed(values).installable(), "{values:?}");
        }
    }

    #[test]
    fn tty_requests_are_removed_from_options_only() {
        for (values, expected) in [
            (&["-t", "host"][..], &["host"][..]),
            (&["-tt", "-At", "host"], &["-A", "host"]),
            (&["-tp22", "host", "-t"], &["-p22", "host"]),
            (&["-tp", "22", "host"], &["-p", "22", "host"]),
            (
                &["-o", "-t", "host", "--", "-t"],
                &["-o", "-t", "host", "--", "-t"],
            ),
            (&["--", "host"], &["--", "host"]),
        ] {
            assert_eq!(
                parsed(values).without_tty,
                args(expected),
                "{values:?}"
            );
        }
    }

    #[test]
    fn non_utf8_arguments_survive() {
        let destination = OsString::from_vec(b"h\xffst".to_vec());
        let value = OsString::from_vec(b"\xfe".to_vec());
        let line = parse(&[
            OsString::from("-t"),
            OsString::from("-l"),
            value.clone(),
            destination.clone(),
        ])
        .unwrap();
        assert_eq!(
            line.without_tty,
            [OsString::from("-l"), value, destination]
        );
    }
}
