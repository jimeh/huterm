use std::io::{self, Read, Write};
use std::process::Command;
use std::thread;
use std::time::Duration;

const COLUMNS: usize = 100;
const ROWS: usize = 32;

/// Writes a few bytes at keystroke pace, so each write is an isolated update
/// whose latency to the screen is not hidden behind an earlier snapshot.
fn echo(stdout: &mut impl Write) -> io::Result<()> {
    for index in 0_u64.. {
        let written =
            write!(stdout, "\r{index:>8}").and_then(|()| stdout.flush());
        if let Err(error) = written {
            return if error.kind() == io::ErrorKind::BrokenPipe {
                Ok(())
            } else {
                Err(error)
            };
        }
        // An uneven period keeps writes from locking phase with a frame timer.
        thread::sleep(Duration::from_millis(90 + index * 7 % 23));
    }
    Ok(())
}

/// Echoes each typed byte as a shell would, from raw mode, so a keystroke's
/// latency to the screen includes the child's read and write.
fn keys(stdout: &mut impl Write) -> io::Result<()> {
    // stty configures the terminal on its inherited standard input.
    if !Command::new("stty")
        .args(["raw", "-echo"])
        .status()?
        .success()
    {
        return Err(io::Error::other("stty could not enter raw mode"));
    }
    let mut stdin = io::stdin().lock();
    let mut byte = [0_u8; 1];
    for index in 0_u64.. {
        if stdin.read(&mut byte)? == 0 {
            break;
        }
        let written =
            write!(stdout, "\r{index:>8}").and_then(|()| stdout.flush());
        if let Err(error) = written {
            return if error.kind() == io::ErrorKind::BrokenPipe {
                Ok(())
            } else {
                Err(error)
            };
        }
    }
    Ok(())
}

/// Lines that saturate a 100,000-row history at 120 columns.
const HISTORY_LINES: u64 = 121_000;

/// Writes enough history to saturate the maximum, then echoes keys. With
/// `HUTERM_RENDER_HISTORY=styles` every cell has its own truecolor pair,
/// which grows pages to several times their usual size; otherwise the
/// history is a colored build log.
fn compression(stdout: &mut impl Write) -> io::Result<()> {
    let styles =
        std::env::var("HUTERM_RENDER_HISTORY").as_deref() == Ok("styles");
    let mut seed = 0x5eed_u64;
    let mut next = move || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        seed >> 33
    };
    let mut batch = Vec::with_capacity(1 << 20);
    for line in 0..HISTORY_LINES {
        if styles {
            for _ in 0..118 {
                write!(
                    batch,
                    "\x1b[38;2;{};{};{};48;2;{};{};{}mx",
                    next() % 256,
                    next() % 256,
                    next() % 256,
                    next() % 256,
                    next() % 256,
                    next() % 256
                )?;
            }
            batch.extend_from_slice(b"\x1b[0m\r\n");
        } else {
            write!(
                batch,
                "\x1b[1;32m   Compiling\x1b[0m crate-{} v0.{}.{} (/src/crates/crate-{})\r\n",
                next() % 400,
                next() % 30,
                next() % 10,
                line % 97
            )?;
        }
        if batch.len() >= 1 << 20 || line + 1 == HISTORY_LINES {
            stdout.write_all(&batch)?;
            batch.clear();
        }
    }
    stdout.flush()?;
    keys(stdout)
}

fn main() -> io::Result<()> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    match std::env::var("HUTERM_RENDER_WORKLOAD").as_deref() {
        Ok("echo") => return echo(&mut stdout),
        Ok("keys") => return keys(&mut stdout),
        Ok("compression") => return compression(&mut stdout),
        _ => {}
    }
    let mut frame = 0_u64;
    let mut output = Vec::with_capacity(COLUMNS * ROWS * 16);
    stdout.write_all(b"\x1b[?1049h\x1b[?25l\x1b[?7l")?;

    loop {
        output.clear();
        output.extend_from_slice(b"\x1b[H");
        for row in 0..ROWS {
            for column in 0..COLUMNS {
                let phase = frame
                    .wrapping_add(u64::try_from(row * 3).unwrap_or(u64::MAX))
                    .wrapping_add(u64::try_from(column).unwrap_or(u64::MAX));
                let color = 16 + (phase % 216);
                write!(output, "\x1b[38;5;{color}m▀")?;
            }
            if row + 1 < ROWS {
                output.extend_from_slice(b"\r\n");
            }
        }
        if let Err(error) =
            stdout.write_all(&output).and_then(|()| stdout.flush())
        {
            return if error.kind() == io::ErrorKind::BrokenPipe {
                Ok(())
            } else {
                Err(error)
            };
        }
        frame = frame.wrapping_add(1);
        thread::sleep(Duration::from_millis(5));
    }
}
