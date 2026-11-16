//! `huterm sudo`: runs the system `sudo` with a `TERM` the system knows.
//!
//! sudo removes `TERMINFO` and `TERMINFO_DIRS` from the environment, so a
//! program it starts finds only the system's terminfo directories. When
//! those lack `xterm-huterm`, sudo runs with `TERM=xterm-256color`, which
//! loses only the entry's `Tc` and larger `pairs`.
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use super::{COMPATIBILITY_TERM, HUTERM_TERM, exec, term_is_huterm};

/// The terminfo directories ncurses searches without `TERMINFO` or
/// `TERMINFO_DIRS`.
const SYSTEM_DIRECTORIES: [&str; 4] = [
    "/etc/terminfo",
    "/lib/terminfo",
    "/usr/share/terminfo",
    "/usr/lib/terminfo",
];

/// `huterm sudo`'s help. Its arguments bypass clap, so every one of them,
/// `--help` included, reaches sudo unchanged.
#[derive(Debug, clap::Args)]
pub(crate) struct Help {
    /// Arguments for sudo, passed through unchanged
    #[arg(
        value_name = "SUDO_ARGUMENTS",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    _arguments: Vec<OsString>,
}

/// Replaces this process with `sudo` and returns only on failure.
pub(crate) fn run(arguments: &[OsString]) -> anyhow::Error {
    let mut command = Command::new("sudo");
    command.args(arguments);
    if term_is_huterm() && !has_entry(&SYSTEM_DIRECTORIES.map(Path::new)) {
        command.env("TERM", COMPATIBILITY_TERM);
    }
    exec(&mut command)
}

/// Whether any of `directories` holds `xterm-huterm`, in the letter or the
/// hexadecimal layout.
fn has_entry(directories: &[&Path]) -> bool {
    directories.iter().any(|directory| {
        ["x", "78"]
            .iter()
            .any(|layout| directory.join(layout).join(HUTERM_TERM).is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_in_either_layout_of_any_directory_counts() {
        let root = std::env::temp_dir()
            .join(format!("huterm-cli-sudo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let [empty, letter, hex, wrong] =
            ["empty", "letter", "hex", "wrong"].map(|name| root.join(name));
        for directory in [&empty, &letter, &hex, &wrong] {
            std::fs::create_dir_all(directory).unwrap();
        }
        for (directory, layout) in [(&letter, "x"), (&hex, "78")] {
            std::fs::create_dir_all(directory.join(layout)).unwrap();
            std::fs::write(directory.join(layout).join(HUTERM_TERM), "")
                .unwrap();
        }
        // A directory in the entry's place is not an entry.
        std::fs::create_dir_all(wrong.join("x").join(HUTERM_TERM)).unwrap();
        let missing = root.join("missing");
        assert!(!has_entry(&[&empty, &missing, &wrong]));
        assert!(has_entry(&[&empty, &letter]));
        assert!(has_entry(&[&missing, &hex]));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
