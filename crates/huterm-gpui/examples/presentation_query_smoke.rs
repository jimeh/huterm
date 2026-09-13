//! Drives production presentation queries through a real PTY and native window.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

const QUERIES: &[u8] =
    b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[14t\x1b[16t\x1b[18t";

fn main() -> anyhow::Result<()> {
    if std::env::var_os("HUTERM_QUERY_CHILD").is_some() {
        return child();
    }
    huterm_gpui::run_presentation_query_smoke()
}

fn child() -> anyhow::Result<()> {
    let directory =
        PathBuf::from(std::env::var("HUTERM_PRESENTATION_QUERY_SMOKE")?);
    let pid = std::process::id();
    std::fs::write(directory.join(format!("child-{pid}")), b"ready")?;
    let status = std::process::Command::new("stty")
        .args(["raw", "-echo"])
        .status()?;
    anyhow::ensure!(status.success(), "stty raw -echo failed: {status}");
    for phase in ["initial", "reload"] {
        if !wait_for(
            &directory.join(format!("query-{phase}-{pid}")),
            &directory,
        )? {
            publish(&directory, &format!("stopped-{pid}"), b"stopped")?;
            return Ok(());
        }
        let replies = capture_replies()?;
        publish(&directory, &format!("replies-{phase}-{pid}"), &replies)?;
    }
    let _ = wait_for(&directory.join(format!("stop-{pid}")), &directory)?;
    publish(&directory, &format!("stopped-{pid}"), b"stopped")?;
    Ok(())
}

fn wait_for(trigger: &Path, directory: &Path) -> anyhow::Result<bool> {
    loop {
        if trigger.is_file() {
            return Ok(true);
        }
        if directory.join("stop-all").is_file() {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn capture_replies() -> anyhow::Result<Vec<u8>> {
    std::io::stdout().write_all(QUERIES)?;
    std::io::stdout().flush()?;
    let mut input = std::io::stdin().lock();
    let mut replies = Vec::new();
    while !complete(&replies) {
        let mut byte = [0];
        input.read_exact(&mut byte)?;
        replies.push(byte[0]);
        anyhow::ensure!(replies.len() <= 4096, "query replies exceeded 4 KiB");
    }
    Ok(replies)
}

fn complete(bytes: &[u8]) -> bool {
    bytes.windows(2).filter(|pair| *pair == b"\x1b\\").count() >= 2
        && bytes.windows(2).filter(|pair| *pair == b"\x1b[").count() >= 3
        && bytes.ends_with(b"t")
}

fn publish(directory: &Path, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let temporary = directory.join(format!("{name}.tmp"));
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, directory.join(name))?;
    Ok(())
}
