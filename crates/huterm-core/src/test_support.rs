//! Test-only clients that pair a full-capability viewer with the runtime's
//! control client.

use std::sync::Arc;

use huterm_protocol::{
    CellSize, GridSize, TerminalSnapshot, TerminalStatus, ViewerCapabilities,
};

use crate::terminal::RuntimeClient;
use crate::{RuntimeError, TerminalRuntime, TerminalViewer, ViewerOptions};

/// A full-capability viewer paired with the runtime's control client,
/// which tests use for job, presentation, and fault-injection requests.
#[derive(Debug)]
pub(crate) struct TestClient {
    pub(crate) viewer: TerminalViewer,
    pub(crate) control: RuntimeClient,
}

impl std::ops::Deref for TestClient {
    type Target = TerminalViewer;

    fn deref(&self) -> &TerminalViewer {
        &self.viewer
    }
}

impl TestClient {
    pub(crate) fn new(runtime: &TerminalRuntime) -> Self {
        Self {
            viewer: runtime.subscribe(viewer_options()).unwrap(),
            control: runtime.client(),
        }
    }

    pub(crate) fn job_context(&self) -> Option<crate::jobs::JobContext> {
        self.control.job_context()
    }

    pub(crate) async fn has_foreground_job(
        &self,
    ) -> Result<bool, RuntimeError> {
        self.control.has_foreground_job().await
    }

    pub(crate) fn resize(
        &self,
        grid: GridSize,
        cell: CellSize,
    ) -> Result<(), RuntimeError> {
        self.viewer.report_geometry(grid, cell)
    }

    pub(crate) fn status(&self) -> Arc<TerminalStatus> {
        self.control.registry().status()
    }
}

/// The text of every cell in `snapshot`, row by row, without line breaks.
pub(crate) fn snapshot_text(snapshot: &TerminalSnapshot) -> String {
    snapshot.cells().map(|cell| cell.text.as_str()).collect()
}

/// A spawning viewer with every capability and no initial state.
pub(crate) fn viewer_options() -> ViewerOptions {
    ViewerOptions {
        spawned: true,
        ..ViewerOptions::new(ViewerCapabilities::ALL)
    }
}

/// Selects the job helper's mode when this test binary runs inside a
/// terminal as [`foreground_job`] or [`orphaned_sleep`].
const JOB_MODE: &str = "HUTERM_TEST_JOB";
/// The OSC payload a held job writes once it holds the foreground.
const JOB_REPORT: &str = "HUTERM_TEST_JOB_REPORT";
/// The marker a held job prints, followed by its PID and `;`.
pub(crate) const HELD: &str = "HELD:";

/// Shell text that runs this test binary as a plain foreground command
/// that moves itself into a new process group and takes the terminal.
/// Once it holds the foreground it writes `ESC ] report BEL` when given,
/// then prints [`HELD`], its PID, and `;`. It hands the terminal back to
/// the shell's group after reading one line, and then exits. A hangup does
/// not end it: like `sleep 30`, it then exits after 30 seconds.
///
/// The job owns its group instead of relying on the shell's job control:
/// macOS `/bin/sh` (bash 3.2) can fail its `setpgid` under load, or leave
/// the job in the shell's group while the terminal names another. A
/// non-interactive shell may exec its last command, which would make the
/// job a session leader, so follow it with another command.
#[cfg(unix)]
pub(crate) fn foreground_job(report: Option<&str>) -> String {
    let report = report
        .map(|report| format!("{JOB_REPORT}={} ", shell_quote(report)))
        .unwrap_or_default();
    format!("{report}{}", job_command("hold"))
}

/// Shell text that starts `sleep 30` in a new process group and exits
/// without waiting for it, leaving an orphan that holds the terminal from
/// outside both the shell's tree and the foreground group.
#[cfg(unix)]
pub(crate) fn orphaned_sleep() -> String {
    job_command("orphan")
}

/// The name a runtime publishes while [`foreground_job`] holds its
/// foreground.
#[cfg(unix)]
pub(crate) fn job_name() -> String {
    let exe = std::env::current_exe().unwrap();
    huterm_procinfo::display_name(&[exe.to_string_lossy().into_owned()])
        .unwrap()
}

/// The PID a held [`foreground_job`] printed in `text`.
#[cfg(unix)]
pub(crate) fn held_pid(text: &str) -> Option<i32> {
    let (_, rest) = text.split_once(HELD)?;
    rest.split_once(';')?.0.parse().ok()
}

#[cfg(unix)]
fn job_command(mode: &str) -> String {
    let exe = std::env::current_exe().unwrap();
    // libtest's own output goes nowhere; the job writes to `/dev/tty`.
    format!(
        "{JOB_MODE}={mode} {} --exact test_support::job_helper --nocapture >/dev/null",
        shell_quote(&exe.to_string_lossy())
    )
}

#[cfg(unix)]
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(unix)]
fn hold(report: Option<String>) {
    use std::io::{Read, Write};

    use nix::sys::signal::{SigSet, Signal};
    use nix::unistd::{Pid, getpgrp, getpid, setpgid, tcsetpgrp};

    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .unwrap();
    let shell = getpgrp();
    setpgid(Pid::from_raw(0), Pid::from_raw(0))
        .expect("the job leads its session; was it the shell's last command?");
    // A background group that changes the terminal's group gets SIGTTOU
    // unless the calling thread blocks or ignores it.
    let mut ttou = SigSet::empty();
    ttou.add(Signal::SIGTTOU);
    ttou.thread_block().unwrap();
    tcsetpgrp(&tty, getpid()).unwrap();
    let report = report
        .map(|report| format!("\x1b]{report}\x07"))
        .unwrap_or_default();
    write!(tty, "{report}{HELD}{};", getpid()).unwrap();
    // A canonical-mode read returns at most one line. After a hangup the
    // job lives on, as `sleep 30` would, so only a signal ends it early.
    if !matches!(tty.read(&mut [0; 256]), Ok(1..)) {
        std::thread::sleep(std::time::Duration::from_secs(30));
        return;
    }
    tcsetpgrp(&tty, shell).unwrap();
}

#[cfg(unix)]
fn orphan() {
    use nix::unistd::{Pid, setpgid};
    use std::process::{Command, Stdio};

    setpgid(Pid::from_raw(0), Pid::from_raw(0))
        .expect("the job leads its session; was it the shell's last command?");
    #[expect(
        clippy::zombie_processes,
        reason = "the helper exits at once and orphans `sleep` on purpose"
    )]
    Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
}

/// Runs the job helper when [`JOB_MODE`] selects it, and otherwise passes
/// at once. It exits the process when done, so libtest reports nothing.
#[cfg(unix)]
#[test]
fn job_helper() {
    let Ok(mode) = std::env::var(JOB_MODE) else {
        return;
    };
    match mode.as_str() {
        "hold" => hold(std::env::var(JOB_REPORT).ok()),
        "orphan" => orphan(),
        other => panic!("unknown job helper mode {other:?}"),
    }
    std::process::exit(0);
}
