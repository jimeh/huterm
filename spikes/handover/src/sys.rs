//! Thin libc wrappers shared by both approaches.

use std::io;
use std::os::fd::RawFd;
use std::time::Duration;

pub fn cvt(ret: libc::c_int) -> io::Result<libc::c_int> {
    if ret == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret)
    }
}

pub fn now_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

pub fn set_cloexec(fd: RawFd, on: bool) -> io::Result<()> {
    let flags = cvt(unsafe { libc::fcntl(fd, libc::F_GETFD) })?;
    let flags = if on {
        flags | libc::FD_CLOEXEC
    } else {
        flags & !libc::FD_CLOEXEC
    };
    cvt(unsafe { libc::fcntl(fd, libc::F_SETFD, flags) }).map(drop)
}

pub fn is_cloexec(fd: RawFd) -> io::Result<bool> {
    Ok(cvt(unsafe { libc::fcntl(fd, libc::F_GETFD) })? & libc::FD_CLOEXEC != 0)
}

pub fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = cvt(unsafe { libc::fcntl(fd, libc::F_GETFL) })?;
    cvt(unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) })
        .map(drop)
}

pub fn dup_cloexec(fd: RawFd) -> io::Result<RawFd> {
    cvt(unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) })
}

pub fn close(fd: RawFd) {
    unsafe { libc::close(fd) };
}

/// A pipe whose both ends are close-on-exec. macOS has no pipe2, so the flag
/// is set after creation; callers spawn under a lock, which is enough here.
pub fn pipe_cloexec() -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0; 2];
    cvt(unsafe { libc::pipe(fds.as_mut_ptr()) })?;
    set_cloexec(fds[0], true)?;
    set_cloexec(fds[1], true)?;
    Ok((fds[0], fds[1]))
}

pub fn read(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

pub fn write(fd: RawFd, buf: &[u8]) -> io::Result<usize> {
    let n = unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

/// Writes everything to a possibly nonblocking descriptor.
pub fn write_all(
    fd: RawFd,
    mut buf: &[u8],
    timeout: Duration,
) -> io::Result<()> {
    let deadline = now_ns() + timeout.as_nanos() as u64;
    while !buf.is_empty() {
        match write(fd, buf) {
            Ok(n) => buf = &buf[n..],
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if now_ns() > deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "write timed out",
                    ));
                }
                poll(&mut [pollfd(fd, libc::POLLOUT)], 50)?;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

pub fn pollfd(fd: RawFd, events: libc::c_short) -> libc::pollfd {
    libc::pollfd {
        fd,
        events,
        revents: 0,
    }
}

pub fn poll(fds: &mut [libc::pollfd], timeout_ms: i32) -> io::Result<usize> {
    loop {
        let n =
            unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout_ms) };
        if n >= 0 {
            return Ok(n as usize);
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

pub fn set_winsize(fd: RawFd, rows: u16, cols: u16) -> io::Result<()> {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    cvt(unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &ws) }).map(drop)
}

pub fn get_winsize(fd: RawFd) -> io::Result<(u16, u16)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    cvt(unsafe { libc::ioctl(fd, libc::TIOCGWINSZ as _, &mut ws) })?;
    Ok((ws.ws_row, ws.ws_col))
}

pub fn tcgetpgrp(fd: RawFd) -> i32 {
    unsafe { libc::tcgetpgrp(fd) }
}

pub fn getsid(pid: i32) -> i32 {
    unsafe { libc::getsid(pid) }
}

pub fn kill(pid: i32, sig: i32) -> io::Result<()> {
    cvt(unsafe { libc::kill(pid, sig) }).map(drop)
}

pub fn alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Blocks until `pid` has exited without reaping it.
pub fn wait_exit_nowait(pid: i32) -> io::Result<()> {
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let r = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if r == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// Whether `pid`, a child of this process, has exited and awaits reaping.
pub fn is_zombie_child(pid: i32) -> bool {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let r = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
        )
    };
    r == 0 && siginfo_pid(&info) == pid
}

#[cfg(target_os = "linux")]
fn siginfo_pid(info: &libc::siginfo_t) -> i32 {
    unsafe { info.si_pid() }
}

#[cfg(target_os = "macos")]
fn siginfo_pid(info: &libc::siginfo_t) -> i32 {
    info.si_pid
}

/// Reaps `pid` if it has exited; returns the raw wait status.
pub fn try_reap(pid: i32) -> Option<i32> {
    let mut status = 0;
    let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    (r == pid).then_some(status)
}

pub fn reap_blocking(pid: i32) -> Option<i32> {
    loop {
        let mut status = 0;
        let r = unsafe { libc::waitpid(pid, &mut status, 0) };
        if r == pid {
            return Some(status);
        }
        if r == -1
            && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted
        {
            continue;
        }
        return None;
    }
}

pub fn openpty(rows: u16, cols: u16) -> io::Result<(RawFd, RawFd)> {
    let mut master = -1;
    let mut slave = -1;
    let mut ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    cvt(unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut ws,
        )
    })?;
    set_cloexec(master, true)?;
    set_cloexec(slave, true)?;
    Ok((master, slave))
}

pub fn rss_kib(pid: i32) -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Physical footprint in KiB: Linux `Pss` from smaps_rollup, macOS
/// `ri_phys_footprint`.
#[cfg(target_os = "linux")]
pub fn footprint_kib(pid: i32) -> Option<u64> {
    let text =
        std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
    text.lines()
        .find(|l| l.starts_with("Pss:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
}

#[cfg(target_os = "macos")]
pub fn footprint_kib(pid: i32) -> Option<u64> {
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let r = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V2,
            (&mut info as *mut libc::rusage_info_v2).cast(),
        )
    };
    (r == 0).then_some(info.ri_phys_footprint / 1024)
}

/// Linux `Private_Clean + Private_Dirty`; macOS has no cheap equivalent.
pub fn private_kib(_pid: i32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text =
            std::fs::read_to_string(format!("/proc/{_pid}/smaps_rollup"))
                .ok()?;
        let mut total = 0;
        for l in text.lines() {
            if l.starts_with("Private_Clean:")
                || l.starts_with("Private_Dirty:")
            {
                total += l.split_whitespace().nth(1)?.parse::<u64>().ok()?;
            }
        }
        Some(total)
    }
    #[cfg(not(target_os = "linux"))]
    None
}

pub fn decode_status(raw: i32) -> serde_json::Value {
    if libc::WIFEXITED(raw) {
        serde_json::json!({"raw": raw, "code": libc::WEXITSTATUS(raw)})
    } else if libc::WIFSIGNALED(raw) {
        serde_json::json!({"raw": raw, "signal": libc::WTERMSIG(raw)})
    } else {
        serde_json::json!({"raw": raw})
    }
}
