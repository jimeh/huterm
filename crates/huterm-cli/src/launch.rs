//! Opening the desktop app installed beside this executable.
use std::path::{Path, PathBuf};

use anyhow::Context as _;

/// The desktop executable's file name, beside `huterm` in every layout.
pub(crate) const GUI_EXECUTABLE: &str = "huterm-gui";

/// Replaces this process with the desktop app and returns only on failure.
///
/// Replacing keeps the process ID, standard streams, and exit status, so a
/// shell, desktop entry, or `AppRun` that starts `huterm` sees the app.
pub(crate) fn desktop_app() -> anyhow::Error {
    match gui_executable() {
        Ok(gui) => exec(&gui),
        Err(error) => error,
    }
}

/// Resolves symlinks first, so `/usr/local/bin/huterm` pointing into an app
/// bundle finds the bundle's `huterm-gui`.
fn gui_executable() -> anyhow::Result<PathBuf> {
    let executable = std::env::current_exe()
        .context("cannot locate the huterm executable")?;
    let executable = std::fs::canonicalize(&executable)
        .with_context(|| format!("cannot resolve {}", executable.display()))?;
    let gui = sibling(&executable);
    if !gui.is_file() {
        anyhow::bail!(
            "the Huterm desktop app is not installed next to this command \
             (expected {}); run `huterm --help` for commands",
            gui.display()
        );
    }
    Ok(gui)
}

fn sibling(executable: &Path) -> PathBuf {
    executable.with_file_name(GUI_EXECUTABLE)
}

#[cfg(unix)]
fn exec(gui: &Path) -> anyhow::Error {
    use std::os::unix::process::CommandExt as _;

    let error = std::process::Command::new(gui).exec();
    anyhow::Error::new(error).context(format!("cannot start {}", gui.display()))
}

#[cfg(not(unix))]
fn exec(_gui: &Path) -> anyhow::Error {
    anyhow::anyhow!("opening the desktop app requires macOS or Linux")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gui_is_a_sibling_in_every_layout() {
        for (cli, gui) in [
            (
                "/Applications/Huterm.app/Contents/MacOS/huterm",
                "/Applications/Huterm.app/Contents/MacOS/huterm-gui",
            ),
            ("/opt/huterm/bin/huterm", "/opt/huterm/bin/huterm-gui"),
            ("/repo/target/debug/huterm", "/repo/target/debug/huterm-gui"),
        ] {
            assert_eq!(sibling(Path::new(cli)), PathBuf::from(gui));
        }
    }
}
