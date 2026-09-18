use std::io::{self, Write};
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

fn main() -> io::Result<()> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    if std::env::var("HUTERM_RENDER_WORKLOAD").is_ok_and(|mode| mode == "echo")
    {
        return echo(&mut stdout);
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
