use std::io::{self, Write};
use std::thread;
use std::time::Duration;

const INITIAL_LINES: usize = 10_064;

fn main() -> io::Result<()> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    for line in 0..INITIAL_LINES {
        writeln!(
            stdout,
            "huterm scroll benchmark row {line:05} — unique nonblank content"
        )?;
    }
    stdout.flush()?;

    thread::sleep(Duration::from_secs(4));
    let mut line = INITIAL_LINES;
    loop {
        if let Err(error) =
            writeln!(stdout, "huterm scroll benchmark flowing row {line:08}")
                .and_then(|()| stdout.flush())
        {
            return if error.kind() == io::ErrorKind::BrokenPipe {
                Ok(())
            } else {
                Err(error)
            };
        }
        line = line.saturating_add(1);
        thread::sleep(Duration::from_millis(8));
    }
}
