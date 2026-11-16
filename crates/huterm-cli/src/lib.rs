//! The `huterm` command line.
//!
//! Without a subcommand, `huterm` replaces itself with the `huterm-gui`
//! desktop executable installed beside it. With arguments, it parses them and
//! runs a subcommand without loading GPUI or needing a display, so the same
//! binary works inside the app bundle, in the Linux packages, and as a
//! standalone download.
//!
//! # Adding a subcommand
//!
//! - Add a variant to [`Command`]. Its doc comment is the help summary.
//! - Put its arguments and its `run(args) -> anyhow::Result<()>` function in
//!   a module under `commands/`. A group such as `huterm server <action>` is
//!   a variant holding its own `#[derive(Subcommand)]` enum.
//! - Dispatch it in [`run`].
//!
//! # Output and exit codes
//!
//! - Help and `--version` print to standard output and exit 0.
//! - Usage errors print clap's message and usage to standard error and exit 2.
//! - A subcommand that fails prints `huterm: <message>` to standard error and
//!   exits 1. Results go to standard output, diagnostics to standard error.
//!
//! This crate must not depend on GPUI; `mise run architecture` checks that.
use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod launch;

/// Exit status for a subcommand that parsed but failed.
const FAILURE: u8 = 1;

#[derive(Debug, Parser)]
#[command(
    name = "huterm",
    version,
    about = "Huterm terminal emulator and multiplexer",
    long_about = "Huterm terminal emulator and multiplexer.\n\n\
                  Run without a command to open the desktop app."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// Every `huterm` subcommand.
#[derive(Debug, Subcommand)]
enum Command {}

/// Runs the command line for `args`, whose first item is the program name.
#[must_use]
pub fn run<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => return usage(&error),
    };
    let result = match cli.command {
        None => Err(launch::desktop_app()),
        Some(command) => match command {},
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("huterm: {error:#}");
            ExitCode::from(FAILURE)
        }
    }
}

/// Prints help, the version, or a usage error, each to its stream.
fn usage(error: &clap::Error) -> ExitCode {
    // A closed stream must not turn help into a failure or a panic.
    let _ = error.print();
    ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(FAILURE))
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use clap::error::ErrorKind;

    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(
            std::iter::once("huterm").chain(args.iter().copied()),
        )
    }

    #[test]
    fn command_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_arguments_opens_the_desktop_app() {
        assert!(parse(&[]).unwrap().command.is_none());
    }

    #[test]
    fn help_and_version_exit_successfully() {
        for (args, kind) in [
            (["--help"], ErrorKind::DisplayHelp),
            (["-h"], ErrorKind::DisplayHelp),
            (["--version"], ErrorKind::DisplayVersion),
            (["-V"], ErrorKind::DisplayVersion),
        ] {
            let error = parse(&args).unwrap_err();
            assert_eq!(error.kind(), kind, "{args:?}");
            assert_eq!(error.exit_code(), 0, "{args:?}");
        }
    }

    #[test]
    fn unknown_subcommands_and_arguments_are_usage_errors() {
        // macOS launch arguments reach `huterm-gui`, never this parser, so
        // they are usage errors here too.
        for args in [
            &["frobnicate"][..],
            &["--frobnicate"],
            &["-psn_0_12345"],
            &["--", "anything"],
        ] {
            let error = parse(args).unwrap_err();
            assert_eq!(error.exit_code(), 2, "{args:?}");
            assert!(error.use_stderr(), "{args:?}");
        }
    }
}
