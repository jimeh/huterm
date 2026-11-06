//! HUTERM_METAL_DIAG: temporary CI diagnostic for CAMetalDisplayLink frame
//! delivery. Writes to `$HUTERM_METAL_DIAG_DIR`, `$HUTERM_SMOKE_EVIDENCE_DIR`,
//! or the GitHub runner's smoke evidence directory; does nothing otherwise.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

static START: OnceLock<Instant> = OnceLock::new();
static FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();
static LINES: AtomicUsize = AtomicUsize::new(0);
const MAX_LINES: usize = 50_000;

fn open() -> Option<Mutex<File>> {
    let directory = std::env::var_os("HUTERM_METAL_DIAG_DIR")
        .or_else(|| std::env::var_os("HUTERM_SMOKE_EVIDENCE_DIR"))
        .map(PathBuf::from)
        .or_else(|| {
            let runner = PathBuf::from("/Users/runner/work/_temp/huterm-smoke-evidence");
            runner.is_dir().then_some(runner)
        })?
        .join("metal-diag");
    std::fs::create_dir_all(&directory).ok()?;
    let executable = std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "unknown".into());
    let path = directory.join(format!("{executable}-{}.log", std::process::id()));
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
        .map(Mutex::new)
}

pub fn log(arguments: std::fmt::Arguments) {
    let start = *START.get_or_init(Instant::now);
    let Some(file) = FILE.get_or_init(open) else {
        return;
    };
    let line = LINES.fetch_add(1, Ordering::Relaxed);
    if line >= MAX_LINES {
        return;
    }
    if let Ok(mut file) = file.lock() {
        let _ = writeln!(
            file,
            "{:>10.3} {}",
            start.elapsed().as_secs_f64() * 1000.0,
            arguments
        );
    }
}

macro_rules! metal_diag {
    ($($argument:tt)*) => {
        $crate::metal_diag::log(format_args!($($argument)*))
    };
}
