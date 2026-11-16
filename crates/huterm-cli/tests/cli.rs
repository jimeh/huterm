//! Runs the built `huterm` executable as a shell would, without a display.
#![cfg(unix)]

use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CLI: &str = env!("CARGO_BIN_EXE_huterm");

/// A directory removed on drop, unique per test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir()
            .join(format!("huterm-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }

    /// Copies the CLI into `directory` as `huterm`.
    fn install_cli(&self, directory: &str) -> PathBuf {
        let directory = self.0.join(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let cli = directory.join("huterm");
        std::fs::copy(CLI, &cli).unwrap();
        cli
    }

    /// A stand-in desktop app that records its process ID and arguments,
    /// writes to both streams, and exits 7.
    fn install_gui(&self, directory: &str) -> PathBuf {
        let gui = self.0.join(directory).join("huterm-gui");
        std::fs::write(
            &gui,
            "#!/bin/sh\n\
             printf '%s\\n' \"$$\" \"$#\" \"$@\" > \"${0%/*}/launched\"\n\
             echo app-out\necho app-err >&2\nexit 7\n",
        )
        .unwrap();
        std::fs::set_permissions(&gui, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        gui
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(program: &Path, args: &[&str]) -> Output {
    Command::new(program)
        .args(args)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[test]
fn help_prints_usage_to_stdout_without_a_display() {
    let scratch = Scratch::new("help");
    let cli = scratch.install_cli("bin");
    scratch.install_gui("bin");
    for flag in ["--help", "-h"] {
        let output = run(&cli, &[flag]);
        assert_eq!(output.status.code(), Some(0), "{flag}");
        assert!(text(&output.stdout).contains("Usage: huterm"), "{flag}");
        assert_eq!(text(&output.stderr), "", "{flag}");
    }
    assert!(
        !scratch.0.join("bin/launched").exists(),
        "help must not start the desktop app"
    );
}

#[test]
fn version_prints_the_package_version() {
    let scratch = Scratch::new("version");
    let cli = scratch.install_cli("bin");
    scratch.install_gui("bin");
    let output = run(&cli, &["--version"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        text(&output.stdout),
        format!("huterm {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(text(&output.stderr), "");
    assert!(
        !scratch.0.join("bin/launched").exists(),
        "--version must not start the desktop app"
    );
}

#[test]
fn unknown_subcommands_and_arguments_fail_with_usage() {
    let scratch = Scratch::new("usage");
    let cli = scratch.install_cli("bin");
    scratch.install_gui("bin");
    for args in [&["frobnicate"][..], &["--frobnicate"], &["-psn_0_1"]] {
        let output = run(&cli, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(text(&output.stdout), "", "{args:?}");
        let stderr = text(&output.stderr);
        assert!(stderr.starts_with("error: "), "{args:?}: {stderr}");
        assert!(stderr.contains("Usage: huterm"), "{args:?}: {stderr}");
    }
    assert!(
        !scratch.0.join("bin/launched").exists(),
        "a usage error must not start the desktop app"
    );
}

#[test]
fn no_arguments_replaces_itself_with_the_sibling_desktop_app() {
    let scratch = Scratch::new("launch");
    let cli = scratch.install_cli("bin");
    scratch.install_gui("bin");
    let child = Command::new(&cli)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(7), "the app's exit status");
    assert_eq!(
        text(&output.stdout),
        "app-out\n",
        "the app's standard output"
    );
    assert_eq!(
        text(&output.stderr),
        "app-err\n",
        "the app's standard error"
    );
    assert_eq!(
        std::fs::read_to_string(scratch.0.join("bin/launched")).unwrap(),
        format!("{pid}\n0\n"),
        "the app replaces the command's process and starts without arguments"
    );
}

#[test]
fn a_symlinked_cli_finds_the_desktop_app_beside_its_target() {
    let scratch = Scratch::new("symlink");
    let cli = scratch.install_cli("app");
    scratch.install_gui("app");
    std::fs::create_dir_all(scratch.0.join("path")).unwrap();
    let link = scratch.0.join("path/huterm");
    symlink(&cli, &link).unwrap();
    assert_eq!(run(&link, &[]).status.code(), Some(7));
    assert!(scratch.0.join("app/launched").exists());
}

#[test]
fn a_desktop_app_that_cannot_start_is_a_failure() {
    let scratch = Scratch::new("unexecutable");
    let cli = scratch.install_cli("bin");
    let gui = scratch.install_gui("bin");
    std::fs::set_permissions(&gui, std::fs::Permissions::from_mode(0o644))
        .unwrap();
    let output = run(&cli, &[]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = text(&output.stderr);
    assert!(stderr.starts_with("huterm: cannot start "), "{stderr}");
    assert!(stderr.contains("huterm-gui"), "{stderr}");
}

#[test]
fn a_missing_desktop_app_is_a_failure() {
    let scratch = Scratch::new("missing");
    let cli = scratch.install_cli("bin");
    let output = run(&cli, &[]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stdout), "");
    let stderr = text(&output.stderr);
    assert!(stderr.starts_with("huterm: "), "{stderr}");
    assert!(stderr.contains("huterm-gui"), "{stderr}");
}
