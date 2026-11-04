//! Programs the scenarios run inside terminals.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::sys;

/// Writes `bytes` of little-endian u64 counters to a raw-mode terminal, so a
/// checker can locate any lost or repeated byte exactly.
pub fn gen_output(args: &[String]) -> i32 {
    let total: u64 = args
        .windows(2)
        .find(|w| w[0] == "--bytes")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(50 * 1024 * 1024);
    raw_mode(1);
    let mut counter: u64 = 0;
    let mut written: u64 = 0;
    let mut buf = Vec::with_capacity(64 * 1024);
    while written < total {
        buf.clear();
        while buf.len() < 64 * 1024 && written + (buf.len() as u64) < total {
            buf.extend_from_slice(&counter.to_le_bytes());
            counter += 1;
        }
        let take = buf.len().min((total - written) as usize);
        if sys::write_all(1, &buf[..take], std::time::Duration::from_secs(120))
            .is_err()
        {
            return 1;
        }
        written += take as u64;
    }
    0
}

fn raw_mode(fd: i32) {
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut t) } == 0 {
        unsafe {
            libc::cfmakeraw(&mut t);
            libc::tcsetattr(fd, libc::TCSANOW, &t);
        }
    }
}

static WINCH: AtomicU32 = AtomicU32::new(0);

extern "C" fn on_winch(_: libc::c_int) {
    WINCH.fetch_add(1, Ordering::SeqCst);
}

/// A minimal full-screen program: it redraws on every SIGWINCH and exits on
/// `q`.
pub fn tui() -> i32 {
    unsafe {
        libc::signal(
            libc::SIGWINCH,
            on_winch as *const () as libc::sighandler_t,
        )
    };
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    unsafe { libc::tcgetattr(0, &mut saved) };
    raw_mode(0);
    let draw = |n: u32| {
        let (rows, cols) = sys::get_winsize(0).unwrap_or((0, 0));
        let frame =
            format!("\x1b[?1049h\x1b[H\x1b[2JREDRAW {n} {rows}x{cols}\r\n");
        let _ = sys::write_all(
            1,
            frame.as_bytes(),
            std::time::Duration::from_secs(5),
        );
    };
    let mut seen = 0;
    draw(0);
    loop {
        let now = WINCH.load(Ordering::SeqCst);
        if now != seen {
            seen = now;
            draw(seen);
        }
        let mut fds = [sys::pollfd(0, libc::POLLIN)];
        if sys::poll(&mut fds, 20).unwrap_or(0) > 0 {
            let mut b = [0u8; 16];
            if let Ok(n) = sys::read(0, &mut b) {
                if n == 0 || b[..n].contains(&b'q') {
                    break;
                }
            }
        }
    }
    let _ =
        sys::write_all(1, b"\x1b[?1049l", std::time::Duration::from_secs(1));
    unsafe { libc::tcsetattr(0, libc::TCSANOW, &saved) };
    0
}
