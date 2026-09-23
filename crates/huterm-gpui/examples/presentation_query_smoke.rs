//! Drives production presentation queries through a real PTY and native window.

use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const QUERIES: &[u8] =
    b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[14t\x1b[16t\x1b[18t";
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

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
        .args(["raw", "-echo", "min", "0", "time", "1"])
        .status()?;
    anyhow::ensure!(status.success(), "stty configuration failed: {status}");
    for phase in ["initial", "padding", "reload"] {
        if !wait_for(
            &directory.join(format!("query-{phase}-{pid}")),
            &directory,
        )? {
            publish(&directory, &format!("stopped-{pid}"), b"stopped")?;
            return Ok(());
        }
        let replies = match capture_replies() {
            Ok(replies) => replies,
            Err(error) => {
                publish(
                    &directory,
                    &format!("error-{phase}-{pid}"),
                    error.to_string().as_bytes(),
                )?;
                publish(&directory, &format!("stopped-{pid}"), b"stopped")?;
                return Err(error);
            }
        };
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
    let started = Instant::now();
    read_replies(&mut input, || started.elapsed() >= REPLY_TIMEOUT)
}

fn read_replies(
    input: &mut impl Read,
    mut timed_out: impl FnMut() -> bool,
) -> anyhow::Result<Vec<u8>> {
    let mut replies = Vec::new();
    while !complete(&replies) {
        anyhow::ensure!(
            !timed_out(),
            "timed out waiting for terminal query replies"
        );
        let mut bytes = [0; 256];
        let count = input.read(&mut bytes)?;
        if count == 0 {
            continue;
        }
        anyhow::ensure!(
            replies.len().saturating_add(count) <= 4096,
            "query replies exceeded 4 KiB"
        );
        replies.extend_from_slice(&bytes[..count]);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_and_empty_replies_end_at_the_deadline() {
        for input in [b"".as_slice(), b"\x1b]10;rgb:ffff".as_slice()] {
            let mut input = input;
            let mut checks = 0;
            let error = read_replies(&mut input, || {
                checks += 1;
                checks > 1
            })
            .unwrap_err();
            assert_eq!(
                error.to_string(),
                "timed out waiting for terminal query replies"
            );
        }
    }
}
