use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use huterm_protocol::{CellSize, GridSize, TerminalCommand};
use nix::sys::stat::Mode;
use nix::unistd::{Pid, mkfifo};

pub(super) struct HolderFixture {
    pub(super) directory: PathBuf,
}

impl HolderFixture {
    pub(super) fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "huterm-holder-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        mkfifo(&directory.join("release"), Mode::S_IRUSR | Mode::S_IWUSR)
            .unwrap();
        Self { directory }
    }

    pub(super) fn command(&self) -> TerminalCommand {
        TerminalCommand {
            engine: huterm_protocol::TerminalEngineKind::default(),
            program: "/bin/sh".into(),
            arguments: vec![
                "-c".into(),
                r#"
trap '' HUP
/bin/sh -c '
    trap "" HUP
    test -t 1 || exit 41
    printf "%s\n" "$$" > "$HUTERM_HOLDER_DIR/ready"
    IFS= read -r release < "$HUTERM_HOLDER_DIR/release"
    test "$release" = release || exit 42
    : > "$HUTERM_HOLDER_DIR/done"
' &
while [ ! -f "$HUTERM_HOLDER_DIR/ready" ]; do sleep 0.01; done
kill -KILL $$
"#
                .into(),
            ],
            working_directory: std::env::current_dir().unwrap(),
            environment: vec![(
                "HUTERM_HOLDER_DIR".into(),
                self.directory.to_string_lossy().into_owned(),
            )],
            grid_size: GridSize::clamped(80, 24),
            cell_size: CellSize {
                width: 8,
                height: 16,
            },
        }
    }

    pub(super) fn wait_ready(&self) -> Pid {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(text) =
                std::fs::read_to_string(self.directory.join("ready"))
                && let Ok(pid) = text.trim().parse()
            {
                return Pid::from_raw(pid);
            }
            assert!(
                Instant::now() < deadline,
                "helper did not publish PID in {:?}",
                self.directory
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub(super) fn release(&self) -> std::io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(nix::fcntl::OFlag::O_NONBLOCK.bits())
                .open(self.directory.join("release"))
            {
                Ok(mut fifo) => return fifo.write_all(b"release\n"),
                Err(error) if Instant::now() >= deadline => return Err(error),
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }
}

impl Drop for HolderFixture {
    fn drop(&mut self) {
        if !self.directory.join("done").exists() {
            let _ = self.release();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.directory.join("ready").exists()
            && !self.directory.join("done").exists()
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
