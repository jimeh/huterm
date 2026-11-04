//! Approach B: SCM_RIGHTS descriptor passing with a two-phase commit, and a
//! per-terminal shim that is the shell's real parent.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::manifest::{self, Exit, Manifest, TermState, Timing};
use crate::server::{
    self, Approach, Options, Server, State, Term, Worker, lock,
};
use crate::shim_core::{
    self, KIND_EXEC_FAILED, KIND_EXITED, KIND_SPAWNED, RECORD_LEN,
};
use crate::sys;

const FDS_PER_MESSAGE: usize = 64;
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------- spawning

/// Opens the PTY here and hands the slave to a shim, whose child becomes the
/// session leader with the PTY as its controlling terminal.
pub fn spawn(
    srv: &Arc<Server>,
    st: &mut State,
    req: &Value,
) -> Result<Value, String> {
    let rows = req["rows"].as_u64().unwrap_or(24) as u16;
    let cols = req["cols"].as_u64().unwrap_or(80) as u16;
    let argv: Vec<String> = serde_json::from_value(req["argv"].clone())
        .map_err(|e| e.to_string())?;
    let (master, slave) =
        sys::openpty(rows, cols).map_err(|e| e.to_string())?;
    sys::set_nonblocking(master).map_err(|e| e.to_string())?;
    let (status_r, status_w) =
        sys::pipe_cloexec().map_err(|e| e.to_string())?;

    let (exe, main_mode) = match &srv.opts.shim_exe {
        Some(p) => (
            p.clone(),
            p.file_name().is_some_and(|n| n == "handover-spike"),
        ),
        None => (std::env::current_exe().map_err(|e| e.to_string())?, true),
    };
    let mut cmd = std::process::Command::new(&exe);
    if main_mode {
        cmd.arg("pty-shim");
    }
    let (soft, hard) = nofile();
    cmd.args(["--nofile", &soft.to_string(), &hard.to_string()]);
    if let Some(cwd) = req["cwd"].as_str() {
        cmd.args(["--cwd", cwd]);
    }
    cmd.arg("--").args(&argv);
    if let Some(env) = req["env"].as_object() {
        for (k, v) in env {
            cmd.env(k, v.as_str().unwrap_or(""));
        }
    }
    let slave_dup = sys::dup_cloexec(slave).map_err(|e| e.to_string())?;
    cmd.stdin(std::process::Stdio::from(unsafe {
        std::os::fd::OwnedFd::from_raw_fd(slave_dup)
    }))
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());
    unsafe {
        cmd.pre_exec(move || {
            // dup2 onto fd 3 clears close-on-exec; if the pipe already is
            // fd 3, clear it explicitly.
            if status_w == shim_core::STATUS_FD {
                let flags = libc::fcntl(status_w, libc::F_GETFD);
                libc::fcntl(status_w, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            } else if libc::dup2(status_w, shim_core::STATUS_FD) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn().map_err(|e| e.to_string());
    sys::close(slave);
    sys::close(status_w);
    let child = child?;
    let shim_pid = child.id() as i32;
    drop(child);

    // The first record names the shell, or the reason it did not start.
    let rec = read_record(status_r, 5000).ok_or("shim sent no spawn record")?;
    let pid = match rec {
        (KIND_SPAWNED, _, pid) => pid,
        (KIND_EXEC_FAILED, errno, _) => {
            let _ = sys::reap_blocking(shim_pid);
            sys::close(status_r);
            sys::close(master);
            return Err(format!(
                "exec failed: {}",
                std::io::Error::from_raw_os_error(errno)
            ));
        }
        other => return Err(format!("unexpected shim record {other:?}")),
    };
    sys::set_nonblocking(status_r).map_err(|e| e.to_string())?;

    let id = st.next_id;
    st.next_id += 1;
    let capture = srv
        .opts
        .run_dir
        .join(format!("term-{id}.capture"))
        .display()
        .to_string();
    let mut t = Term {
        st: TermState {
            id,
            pid,
            shim_pid: Some(shim_pid),
            master_fd: master,
            status_fd: Some(status_r),
            rows,
            cols,
            capture: capture.clone(),
            bytes: 0,
            exit: None,
            eof: false,
            closed: false,
            last_read_ns: sys::now_ns(),
        },
        own_child: true,
        reader: None,
        status: None,
        resumed_ns: None,
        stall_ns: None,
    };
    t.reader = Some(server::start_reader(srv, id, master, &capture));
    t.status = Some(exit_watch(srv, &t));
    st.terms.insert(id, t);
    Ok(json!({"id": id, "pid": pid, "shim_pid": shim_pid}))
}

fn nofile() -> (u64, u64) {
    let mut l = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut l) };
    (l.rlim_cur as u64, l.rlim_max as u64)
}

fn read_record(fd: RawFd, timeout_ms: i32) -> Option<(u16, i32, i32)> {
    let mut buf = [0u8; RECORD_LEN];
    let mut got = 0;
    let deadline = sys::now_ns() + timeout_ms as u64 * 1_000_000;
    while got < RECORD_LEN {
        let left = deadline.saturating_sub(sys::now_ns()) / 1_000_000;
        if left == 0
            || sys::poll(&mut [sys::pollfd(fd, libc::POLLIN)], left as i32)
                .ok()?
                == 0
        {
            return None;
        }
        match sys::read(fd, &mut buf[got..]) {
            Ok(0) => return None,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    shim_core::decode(&buf)
}

/// Reads the shim's exit record. A stop leaves unread records in the pipe,
/// which is handed over with the master.
pub fn exit_watch(srv: &Arc<Server>, t: &Term) -> Worker {
    let srv = Arc::clone(srv);
    let (id, fd, shim_pid, own) =
        (t.st.id, t.st.status_fd.unwrap(), t.st.shim_pid, t.own_child);
    Worker::spawn(format!("status-{id}"), move |stop_r| {
        let mut buf = Vec::with_capacity(RECORD_LEN);
        loop {
            let mut fds = [
                sys::pollfd(fd, libc::POLLIN),
                sys::pollfd(stop_r, libc::POLLIN),
            ];
            if sys::poll(&mut fds, -1).is_err() || fds[1].revents != 0 {
                return;
            }
            let mut chunk = [0u8; RECORD_LEN];
            let want = RECORD_LEN - buf.len();
            match sys::read(fd, &mut chunk[..want]) {
                Ok(0) => {
                    server::record_exit(&srv, id, Exit::Unknown);
                    break;
                }
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() == RECORD_LEN {
                        if let Some((KIND_EXITED, raw, _)) =
                            shim_core::decode(&buf)
                        {
                            server::record_exit(&srv, id, Exit::Status(raw));
                            break;
                        }
                        buf.clear();
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => {
                    server::record_exit(&srv, id, Exit::Unknown);
                    break;
                }
            }
        }
        if own {
            if let Some(shim) = shim_pid {
                let _ = sys::reap_blocking(shim);
            }
        }
    })
}

// ---------------------------------------------------------------- handover

pub fn handover(srv: &Arc<Server>, req: &Value) {
    let _h = lock(&srv.handover_lock);
    let begin = sys::now_ns();
    let exe = req["exe"].as_str().unwrap_or("").to_string();
    let extra: Vec<String> =
        serde_json::from_value(req["args"].clone()).unwrap_or_default();
    let mut timing = Timing {
        begin_ns: begin,
        target_build: extra
            .windows(2)
            .filter(|w| w[0] == "--build-id")
            .map(|w| w[1].clone())
            .last()
            .unwrap_or_default(),
        nudge: req["nudge"].as_str().unwrap_or("shrink").into(),
        source_pid: std::process::id() as i32,
        ..Timing::default()
    };

    let mut tok = [0u8; 16];
    let _ = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut tok));
    let token: String = tok.iter().map(|b| format!("{b:02x}")).collect();
    let sock = srv
        .opts
        .run_dir
        .join(format!("import-{}.sock", &token[..8]));
    let listener = match UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(e) => {
            return abort(
                srv,
                None,
                None,
                &timing,
                &format!("bind import socket: {e}"),
            );
        }
    };
    use std::os::unix::fs::PermissionsExt;
    let _ =
        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600));

    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(srv.opts.run_dir.join("server-import.log"))
        .ok();
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(["server", "--approach", "b", "--run-dir"])
        .arg(&srv.opts.run_dir)
        .args(&extra)
        .args(["--import-socket"])
        .arg(&sock)
        .args(["--token", &token])
        .args(
            srv.opts
                .driver_pid
                .map(|p| vec!["--driver-pid".to_string(), p.to_string()])
                .unwrap_or_default(),
        )
        .stdin(std::process::Stdio::null())
        .process_group(0);
    if let Some(log) = log {
        cmd.stdout(std::process::Stdio::null()).stderr(log);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&sock);
            return abort(
                srv,
                None,
                None,
                &timing,
                &format!("spawn {exe}: {e}"),
            );
        }
    };

    let result = (|| -> Result<UnixStream, String> {
        let stream = accept_timeout(&listener, STEP_TIMEOUT)?;
        stream
            .set_read_timeout(Some(STEP_TIMEOUT))
            .map_err(|e| e.to_string())?;
        let mut rd =
            BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        if read_line(&mut rd)? != token {
            return Err("token mismatch".into());
        }
        drop(rd);
        Ok(stream)
    })();
    let _ = std::fs::remove_file(&sock);
    let stream = match result {
        Ok(s) => s,
        Err(e) => return abort(srv, Some(&mut child), None, &timing, &e),
    };

    timing.pause_start_ns = sys::now_ns();
    let workers = server::take_workers(&mut lock(&srv.state), true);
    server::stop_all(workers);
    let mut st = lock(&srv.state);
    crate::signal_during_pause(&st, req, Approach::B);
    timing.paused_ns = sys::now_ns();
    let manifest = srv.manifest(&st, timing.clone());

    let result = (|| -> Result<(), String> {
        let mut w = stream.try_clone().map_err(|e| e.to_string())?;
        let mut rd =
            BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let mut body =
            serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
        body.push(b'\n');
        w.write_all(&body).map_err(|e| e.to_string())?;
        let line = read_line(&mut rd)?;
        if line != "validated" {
            return Err(format!("target refused: {line}"));
        }
        send_fds(&stream, &fds_of(&manifest))
            .map_err(|e| format!("sendmsg: {e}"))?;
        let line = read_line(&mut rd)?;
        if line != "ready" {
            return Err(format!("target not ready: {line}"));
        }
        writeln!(w, "committed {}", sys::now_ns())
            .map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_millis(2000)))
            .ok();
        // The target acks before it reads anything, so anything but the ack
        // or a read timeout means it died unowned and the source can resume.
        // EOF and Linux's ECONNRESET (a close with unread data) both qualify.
        // Only a timeout is ambiguous; then assume the target owns everything.
        let mut line = String::new();
        match rd.read_line(&mut line) {
            Ok(_) if line.trim_end() == "owned" => Ok(()),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) =>
            {
                eprintln!("no ownership ack after commit: {e}");
                Ok(())
            }
            Ok(_) => Err(format!(
                "target died after commit, before owning: {:?}",
                line.trim_end()
            )),
            Err(e) => {
                Err(format!("target died after commit, before owning: {e}"))
            }
        }
    })();
    match result {
        Ok(()) => {
            // The target owns every terminal. Exit without running
            // destructors; the target holds its own copies of every
            // descriptor, so closing ours hangs nothing up.
            unsafe { libc::_exit(0) };
        }
        Err(e) => abort(srv, Some(&mut child), Some(st), &timing, &e),
    }
}

fn abort(
    srv: &Arc<Server>,
    child: Option<&mut std::process::Child>,
    st: Option<std::sync::MutexGuard<'_, State>>,
    timing: &Timing,
    reason: &str,
) {
    if let Some(child) = child {
        let _ = child.kill();
        let _ = child.wait();
    }
    let mut st = st.unwrap_or_else(|| lock(&srv.state));
    server::start_workers(srv, &mut st);
    let rec = json!({
        "approach": "b",
        "outcome": "rolled_back",
        "reason": reason,
        "where": "source server kept every terminal; it never released them",
        "timing": timing,
        "source_pid": timing.source_pid,
        "target_pid": std::process::id(),
        "cutover_ms": server::ns_to_ms(sys::now_ns() - timing.begin_ns),
    });
    server::push_history(&mut st, rec);
}

fn fds_of(m: &Manifest) -> Vec<RawFd> {
    let mut fds = Vec::new();
    for t in &m.terminals {
        if !t.closed {
            fds.push(t.master_fd);
        }
        if let Some(s) = t.status_fd.filter(|_| t.exit.is_none()) {
            fds.push(s);
        }
    }
    fds.push(m.listener_fd);
    fds
}

fn accept_timeout(
    listener: &UnixListener,
    timeout: Duration,
) -> Result<UnixStream, String> {
    let n = sys::poll(
        &mut [sys::pollfd(listener.as_raw_fd(), libc::POLLIN)],
        timeout.as_millis() as i32,
    )
    .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("target never connected".into());
    }
    listener.accept().map(|(s, _)| s).map_err(|e| e.to_string())
}

fn read_line(rd: &mut impl BufRead) -> Result<String, String> {
    let mut line = String::new();
    match rd.read_line(&mut line) {
        Ok(0) => Err("peer closed the handover socket".into()),
        Ok(_) => Ok(line.trim_end().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

fn send_fds(stream: &UnixStream, fds: &[RawFd]) -> std::io::Result<()> {
    for batch in fds.chunks(FDS_PER_MESSAGE) {
        let byte = [b'F'];
        let iov = libc::iovec {
            iov_base: byte.as_ptr() as *mut _,
            iov_len: 1,
        };
        let len = std::mem::size_of_val(batch) as u32;
        let mut control = vec![0u8; unsafe { libc::CMSG_SPACE(len) } as usize];
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &iov as *const _ as *mut _;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = control.len() as _;
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(len) as _;
            std::ptr::copy_nonoverlapping(
                batch.as_ptr().cast::<u8>(),
                libc::CMSG_DATA(cmsg),
                len as usize,
            );
            if libc::sendmsg(stream.as_raw_fd(), &msg, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

fn recv_fds(
    stream: &UnixStream,
    expected: usize,
) -> std::io::Result<Vec<RawFd>> {
    let mut out = Vec::with_capacity(expected);
    while out.len() < expected {
        let wanted = (expected - out.len()).min(FDS_PER_MESSAGE);
        let mut byte = [0u8; 1];
        let mut iov = libc::iovec {
            iov_base: byte.as_mut_ptr().cast(),
            iov_len: 1,
        };
        let len = (wanted * std::mem::size_of::<RawFd>()) as u32;
        let mut control = vec![0u8; unsafe { libc::CMSG_SPACE(len) } as usize];
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = control.len() as _;
        #[cfg(target_os = "linux")]
        let flags = libc::MSG_CMSG_CLOEXEC;
        #[cfg(not(target_os = "linux"))]
        let flags = 0;
        let n = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, flags) };
        if n <= 0 {
            return Err(std::io::Error::other(
                "handover socket closed during descriptor transfer",
            ));
        }
        if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(std::io::Error::other("descriptor message truncated"));
        }
        unsafe {
            let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
            while !cmsg.is_null() {
                if (*cmsg).cmsg_level == libc::SOL_SOCKET
                    && (*cmsg).cmsg_type == libc::SCM_RIGHTS
                {
                    let count = ((*cmsg).cmsg_len as usize
                        - libc::CMSG_LEN(0) as usize)
                        / std::mem::size_of::<RawFd>();
                    let data = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                    for i in 0..count {
                        out.push(std::ptr::read_unaligned(data.add(i)));
                    }
                }
                cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
            }
        }
    }
    for fd in &out {
        let _ = sys::set_cloexec(*fd, true);
    }
    Ok(out)
}

/// Entry point of the target server.
pub fn import(opts: Options) -> ! {
    let import_start = sys::now_ns();
    let sock = opts.import_socket.clone().expect("import socket");
    let token = opts.token.clone().unwrap_or_default();
    let stream = UnixStream::connect(&sock).expect("connect import socket");
    stream.set_read_timeout(Some(STEP_TIMEOUT)).ok();
    let mut w = stream.try_clone().unwrap();
    let mut rd = BufReader::new(stream.try_clone().unwrap());
    writeln!(w, "{token}").unwrap();
    let line = read_line(&mut rd).expect("manifest");
    // A manifest this build cannot parse panics here, as an unhandled schema
    // change would; the source sees the socket close and keeps everything.
    let mut m: Manifest = manifest::parse(&line, opts.require_field.as_deref())
        .expect("parse manifest");
    if let Err(reason) = manifest::validate(&m, &opts.supported_formats, "b") {
        let _ = writeln!(w, "rejected: {reason}");
        std::process::exit(1);
    }
    writeln!(w, "validated").unwrap();
    let expected = fds_of(&m).len();
    let fds = recv_fds(&stream, expected).expect("receive descriptors");
    if opts.crash_on_import {
        std::process::abort();
    }
    let mut it = fds.into_iter();
    for t in &mut m.terminals {
        if !t.closed {
            t.master_fd = it.next().unwrap();
        }
        if t.exit.is_none() && t.status_fd.is_some() {
            t.status_fd = it.next();
        }
    }
    m.listener_fd = it.next().unwrap();
    let mut st = server::restore_state(&opts, &m, false);
    st.generation += 1;
    let restored = sys::now_ns();
    writeln!(w, "ready").unwrap();

    stream.set_read_timeout(Some(Duration::from_secs(15))).ok();
    let line = match read_line(&mut rd) {
        Ok(l) if l.starts_with("committed") => l,
        other => {
            // The source still owns everything; closing our copies is safe.
            eprintln!("no commit: {other:?}");
            std::process::exit(1);
        }
    };
    let committed: u64 = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    writeln!(w, "owned").unwrap();

    let srv = Arc::new(Server {
        opts: opts.clone(),
        state: Mutex::new(st),
        handover_lock: Mutex::new(()),
    });
    let mut st = lock(&srv.state);
    server::start_workers(&srv, &mut st);
    let resumed = sys::now_ns();
    for t in st.terms.values() {
        if !t.st.closed && !t.st.eof {
            server::nudge(
                t.st.master_fd,
                t.st.rows,
                t.st.cols,
                &m.handover.nudge,
            );
        }
    }
    let nudged = sys::now_ns();
    let rec = json!({
        "approach": "b",
        "outcome": "ok",
        "reason": "",
        "where": "new server process owns the terminals",
        "timing": m.handover,
        "source_pid": m.handover.source_pid,
        "target_pid": std::process::id(),
        "import_start_ns": import_start,
        "restored_ns": restored,
        "committed_ns": committed,
        "resumed_ns": resumed,
        "nudged_ns": nudged,
        "cutover_ms": server::ns_to_ms(resumed - m.handover.begin_ns),
        "blackout_ms": server::ns_to_ms(resumed - m.handover.pause_start_ns),
        "terminals": m.terminals.len(),
    });
    server::push_history(&mut st, rec);
    drop(st);
    server::serve(srv)
}
