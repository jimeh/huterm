//! Approach B's per-terminal shim. It runs outside the terminal, forks, and
//! its child performs the setup portable-pty's `pre_exec` does today: signal
//! and mask resets, `setsid`, `TIOCSCTTY`, the stdio `dup2`s, descriptor
//! closing, and the descriptor limit. The shim stays the shell's parent and
//! reports the raw wait status as one fixed record on fd 3.
//!
//! Invocation: `[--nofile SOFT HARD] [--cwd DIR] -- PROGRAM ARGS...`, with the
//! PTY slave on stdin and the status pipe on fd 3.

use std::ffi::{CString, OsString};
use std::os::unix::ffi::OsStrExt;

pub const STATUS_FD: i32 = 3;
pub const RECORD_LEN: usize = 16;
pub const MAGIC: [u8; 4] = *b"HSHM";
pub const VERSION: u16 = 1;
pub const KIND_SPAWNED: u16 = 1;
pub const KIND_EXEC_FAILED: u16 = 2;
pub const KIND_EXITED: u16 = 3;

pub fn encode(kind: u16, value: i32, pid: i32) -> [u8; RECORD_LEN] {
    let mut r = [0u8; RECORD_LEN];
    r[..4].copy_from_slice(&MAGIC);
    r[4..6].copy_from_slice(&VERSION.to_le_bytes());
    r[6..8].copy_from_slice(&kind.to_le_bytes());
    r[8..12].copy_from_slice(&value.to_le_bytes());
    r[12..16].copy_from_slice(&pid.to_le_bytes());
    r
}

#[allow(dead_code)]
pub fn decode(r: &[u8]) -> Option<(u16, i32, i32)> {
    if r.len() != RECORD_LEN
        || r[..4] != MAGIC
        || u16::from_le_bytes([r[4], r[5]]) != VERSION
    {
        return None;
    }
    Some((
        u16::from_le_bytes([r[6], r[7]]),
        i32::from_le_bytes(r[8..12].try_into().ok()?),
        i32::from_le_bytes(r[12..16].try_into().ok()?),
    ))
}

fn report(kind: u16, value: i32, pid: i32) {
    // One write below PIPE_BUF is atomic; a reader never sees half a record.
    let rec = encode(kind, value, pid);
    unsafe { libc::write(STATUS_FD, rec.as_ptr().cast(), rec.len()) };
}

pub fn run(args: Vec<OsString>) -> ! {
    let mut nofile: Option<(libc::rlim_t, libc::rlim_t)> = None;
    let mut cwd: Option<CString> = None;
    let mut it = args.into_iter();
    let mut argv: Vec<CString> = Vec::new();
    while let Some(a) = it.next() {
        match a.as_bytes() {
            b"--nofile" => {
                let soft = it.next().and_then(|s| s.to_str()?.parse().ok());
                let hard = it.next().and_then(|s| s.to_str()?.parse().ok());
                nofile = soft.zip(hard);
            }
            b"--cwd" => {
                cwd = it.next().map(|s| CString::new(s.as_bytes()).unwrap())
            }
            b"--" => {
                argv = it
                    .by_ref()
                    .map(|s| CString::new(s.as_bytes()).unwrap())
                    .collect();
            }
            _ => {}
        }
    }
    if argv.is_empty() {
        unsafe { libc::_exit(2) };
    }
    let mut ptrs: Vec<*const libc::c_char> =
        argv.iter().map(|c| c.as_ptr()).collect();
    ptrs.push(std::ptr::null());

    unsafe {
        // Writes to a pipe whose reader went away must fail, not kill us.
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        // Leave the server's process group so signals aimed at it miss us.
        libc::setsid();

        let mut err_pipe = [0; 2];
        if libc::pipe(err_pipe.as_mut_ptr()) == -1 {
            libc::_exit(3);
        }
        libc::fcntl(err_pipe[0], libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(err_pipe[1], libc::F_SETFD, libc::FD_CLOEXEC);

        let pid = libc::fork();
        if pid == 0 {
            child(&ptrs, cwd.as_ref(), nofile, err_pipe[1]);
        }
        libc::close(err_pipe[1]);
        if pid < 0 {
            report(
                KIND_EXEC_FAILED,
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
                0,
            );
            libc::_exit(1);
        }
        let mut errno = [0u8; 4];
        let n = loop {
            let n = libc::read(err_pipe[0], errno.as_mut_ptr().cast(), 4);
            if n >= 0
                || std::io::Error::last_os_error().kind()
                    != std::io::ErrorKind::Interrupted
            {
                break n;
            }
        };
        libc::close(err_pipe[0]);
        // Drop every handle on the terminal. A slave held here would keep the
        // master from seeing the hangup when the shell exits.
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        for fd in 0..3 {
            libc::dup2(null, fd);
        }
        if null > 2 {
            libc::close(null);
        }
        if n == 4 {
            let mut status = 0;
            libc::waitpid(pid, &mut status, 0);
            report(KIND_EXEC_FAILED, i32::from_ne_bytes(errno), pid);
            libc::_exit(1);
        }
        report(KIND_SPAWNED, 0, pid);
        let mut status = 0;
        loop {
            let r = libc::waitpid(pid, &mut status, 0);
            if r == pid {
                break;
            }
            if r == -1
                && std::io::Error::last_os_error().kind()
                    != std::io::ErrorKind::Interrupted
            {
                status = -1;
                break;
            }
        }
        report(KIND_EXITED, status, pid);
        libc::_exit(0);
    }
}

/// Runs in the forked child. Only async-signal-safe calls until `execvp`.
unsafe fn child(
    argv: &[*const libc::c_char],
    cwd: Option<&CString>,
    nofile: Option<(libc::rlim_t, libc::rlim_t)>,
    err_w: i32,
) -> ! {
    unsafe {
        for sig in [
            libc::SIGCHLD,
            libc::SIGHUP,
            libc::SIGINT,
            libc::SIGQUIT,
            libc::SIGTERM,
            libc::SIGALRM,
            libc::SIGPIPE,
        ] {
            libc::signal(sig, libc::SIG_DFL);
        }
        let empty: libc::sigset_t = std::mem::zeroed();
        libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
        let fail = |err_w: i32| -> ! {
            let e = std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or(0)
                .to_ne_bytes();
            libc::write(err_w, e.as_ptr().cast(), 4);
            libc::_exit(127);
        };
        if libc::setsid() == -1 {
            fail(err_w);
        }
        if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
            fail(err_w);
        }
        if libc::dup2(0, 1) == -1 || libc::dup2(0, 2) == -1 {
            fail(err_w);
        }
        // Close everything above stdio except the error pipe, which closes on
        // exec. The status pipe (fd 3) must not reach the shell.
        let max = libc::sysconf(libc::_SC_OPEN_MAX).clamp(256, 65536) as i32;
        for fd in 3..max {
            if fd != err_w {
                libc::close(fd);
            }
        }
        if let Some((soft, hard)) = nofile {
            let l = libc::rlimit {
                rlim_cur: soft,
                rlim_max: hard,
            };
            libc::setrlimit(libc::RLIMIT_NOFILE, &l);
        }
        if let Some(dir) = cwd {
            if libc::chdir(dir.as_ptr()) == -1 {
                fail(err_w);
            }
        }
        libc::execvp(argv[0], argv.as_ptr());
        fail(err_w);
    }
}
