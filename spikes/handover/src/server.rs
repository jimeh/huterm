//! Shared server core: terminals, reader pause and drain, the control
//! socket, the redraw nudge, and restore from a manifest. The capture file
//! per terminal stands in for the emulator: it receives every byte read.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{Value, json};

use crate::manifest::{Exit, Manifest, TermState, Timing};
use crate::sys;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Approach {
    A,
    B,
}

impl Approach {
    pub fn name(self) -> &'static str {
        match self {
            Approach::A => "a",
            Approach::B => "b",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub approach: Approach,
    pub run_dir: PathBuf,
    pub build_id: String,
    pub supported_formats: Vec<u32>,
    pub crash_on_import: bool,
    /// Approach B: the shim executable and whether it is the main binary.
    pub shim_exe: Option<PathBuf>,
    pub resume_fd: Option<RawFd>,
    pub rollback_reason: Option<String>,
    pub handoff_ns: Option<u64>,
    pub import_socket: Option<PathBuf>,
    pub token: Option<String>,
    /// A: where to roll back to and as which build, carried in argv so a
    /// manifest the new build cannot parse still rolls back.
    pub rollback_exe: Option<String>,
    pub source_build: Option<String>,
    /// Test hook: fail to parse any manifest lacking this field, as a
    /// changed schema would.
    pub require_field: Option<String>,
    /// Exit, closing every terminal, once this process is gone.
    pub driver_pid: Option<i32>,
}

impl Options {
    pub fn parse(args: &[String]) -> Options {
        let mut o = Options {
            approach: Approach::A,
            run_dir: PathBuf::from("/tmp"),
            build_id: "v1".into(),
            supported_formats: vec![crate::manifest::FORMAT],
            crash_on_import: false,
            shim_exe: None,
            resume_fd: None,
            rollback_reason: None,
            handoff_ns: None,
            import_socket: None,
            token: None,
            rollback_exe: None,
            source_build: None,
            require_field: None,
            driver_pid: None,
        };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut val = || it.next().cloned().unwrap_or_default();
            match a.as_str() {
                "--approach" => {
                    o.approach = if val() == "b" {
                        Approach::B
                    } else {
                        Approach::A
                    }
                }
                "--run-dir" => o.run_dir = val().into(),
                "--build-id" => o.build_id = val(),
                "--supported-formats" => {
                    o.supported_formats = val()
                        .split(',')
                        .filter_map(|s| s.parse().ok())
                        .collect()
                }
                "--crash-on-import" => o.crash_on_import = true,
                "--shim-exe" => o.shim_exe = Some(val().into()),
                "--resume-fd" => o.resume_fd = val().parse().ok(),
                "--rollback-reason" => o.rollback_reason = Some(val()),
                "--handoff-ns" => o.handoff_ns = val().parse().ok(),
                "--import-socket" => o.import_socket = Some(val().into()),
                "--token" => o.token = Some(val()),
                "--rollback-exe" => o.rollback_exe = Some(val()),
                "--source-build" => o.source_build = Some(val()),
                "--require-manifest-field" => o.require_field = Some(val()),
                "--driver-pid" => o.driver_pid = val().parse().ok(),
                other => eprintln!("server: ignoring argument {other}"),
            }
        }
        o
    }
}

pub struct Worker {
    stop_w: RawFd,
    handle: JoinHandle<()>,
}

impl Worker {
    pub fn spawn(
        name: String,
        f: impl FnOnce(RawFd) + Send + 'static,
    ) -> Worker {
        let (stop_r, stop_w) = sys::pipe_cloexec().expect("stop pipe");
        let handle = std::thread::Builder::new()
            .name(name)
            .spawn(move || {
                f(stop_r);
                sys::close(stop_r);
            })
            .expect("spawn worker");
        Worker { stop_w, handle }
    }

    /// Stops the worker and waits until it has finished its current step.
    pub fn stop(self) {
        let _ = sys::write(self.stop_w, b"x");
        let _ = self.handle.join();
        sys::close(self.stop_w);
    }
}

pub struct Term {
    pub st: TermState,
    /// Whether the process this server waits on (the shell in A, the shim in
    /// B) is this process's child.
    pub own_child: bool,
    pub reader: Option<Worker>,
    pub status: Option<Worker>,
    pub resumed_ns: Option<u64>,
    pub stall_ns: Option<u64>,
}

pub struct State {
    pub server_id: String,
    pub generation: u64,
    pub listener_fd: RawFd,
    pub next_id: u32,
    pub terms: BTreeMap<u32, Term>,
    pub history: Vec<Value>,
    pub rollback_exe: Option<String>,
}

pub struct Server {
    pub opts: Options,
    pub state: Mutex<State>,
    pub handover_lock: Mutex<()>,
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Server {
    pub fn manifest(&self, st: &State, timing: Timing) -> Manifest {
        Manifest {
            format: crate::manifest::FORMAT,
            approach: self.opts.approach.name().into(),
            server_id: st.server_id.clone(),
            source_build: self.opts.build_id.clone(),
            generation: st.generation,
            run_dir: self.opts.run_dir.display().to_string(),
            listener_fd: st.listener_fd,
            rollback_exe: st.rollback_exe.clone(),
            next_id: st.next_id,
            terminals: st.terms.values().map(|t| t.st.clone()).collect(),
            handover: timing,
            history: st.history.clone(),
        }
    }
}

/// Reads every available byte into the capture until stopped or hung up.
/// Each chunk reaches the capture before the stop is observed, so a stopped
/// reader has delivered everything it took from the PTY.
pub fn start_reader(
    srv: &Arc<Server>,
    id: u32,
    master: RawFd,
    capture: &str,
) -> Worker {
    let srv = Arc::clone(srv);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(capture)
        .expect("open capture");
    Worker::spawn(format!("reader-{id}"), move |stop_r| {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut fds = [
                sys::pollfd(master, libc::POLLIN),
                sys::pollfd(stop_r, libc::POLLIN),
            ];
            if sys::poll(&mut fds, -1).is_err() || fds[1].revents != 0 {
                return;
            }
            loop {
                match sys::read(master, &mut buf) {
                    Ok(0) => return mark_eof(&srv, id),
                    Ok(n) => {
                        file.write_all(&buf[..n]).expect("capture write");
                        let now = sys::now_ns();
                        let mut st = lock(&srv.state);
                        if let Some(t) = st.terms.get_mut(&id) {
                            t.st.bytes += n as u64;
                            if let (Some(resumed), None) =
                                (t.resumed_ns, t.stall_ns)
                            {
                                let _ = resumed;
                                t.stall_ns =
                                    Some(now.saturating_sub(t.st.last_read_ns));
                            }
                            t.st.last_read_ns = now;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    // EIO: the slave side has no open descriptors left.
                    Err(_) => return mark_eof(&srv, id),
                }
            }
        }
    })
}

fn mark_eof(srv: &Arc<Server>, id: u32) {
    if let Some(t) = lock(&srv.state).terms.get_mut(&id) {
        t.st.eof = true;
    }
}

pub fn record_exit(srv: &Arc<Server>, id: u32, exit: Exit) {
    if let Some(t) = lock(&srv.state).terms.get_mut(&id) {
        if t.st.exit.is_none() {
            t.st.exit = Some(exit);
        }
    }
}

/// Takes the readers, and the exit observers too when `observers`, out of
/// every terminal. Stop them only after releasing the state lock: a reader
/// mid-chunk needs that lock to record its bytes, so stopping it under the
/// lock deadlocks.
pub fn take_workers(st: &mut State, observers: bool) -> Vec<Worker> {
    let mut out = Vec::new();
    for t in st.terms.values_mut() {
        out.extend(t.reader.take());
        if observers {
            out.extend(t.status.take());
        }
    }
    out
}

/// Stops workers. Bytes they have not read stay in the kernel's PTY and
/// status-pipe buffers for the next owner.
pub fn stop_all(workers: Vec<Worker>) {
    for w in workers {
        w.stop();
    }
}

pub fn start_workers(srv: &Arc<Server>, st: &mut State) {
    let now = sys::now_ns();
    for (id, t) in st.terms.iter_mut() {
        if !t.st.closed && !t.st.eof {
            t.reader =
                Some(start_reader(srv, *id, t.st.master_fd, &t.st.capture));
        }
        if t.st.exit.is_none() {
            t.status = Some(crate::exit_watch(srv, t));
        }
        t.resumed_ns = Some(now);
        t.stall_ns = None;
    }
}

pub fn nudge(master: RawFd, rows: u16, cols: u16, mode: &str) {
    match mode {
        "shrink" => {
            let (r, c) = if rows > 2 {
                (rows - 1, cols)
            } else {
                (rows, cols.saturating_sub(1).max(4))
            };
            let _ = sys::set_winsize(master, r, c);
            std::thread::sleep(Duration::from_millis(30));
            let _ = sys::set_winsize(master, rows, cols);
        }
        "same" => {
            let _ = sys::set_winsize(master, rows, cols);
        }
        "sigwinch" => {
            let pgrp = sys::tcgetpgrp(master);
            if pgrp > 0 {
                let _ = sys::kill(-pgrp, libc::SIGWINCH);
            }
        }
        _ => {}
    }
}

/// Rebuilds terminals from a manifest whose descriptors are already open in
/// this process. Nothing reads until `start_workers`.
pub fn restore_state(
    srv_opts: &Options,
    m: &Manifest,
    own_child: bool,
) -> State {
    let _ = srv_opts;
    let mut terms = BTreeMap::new();
    for t in &m.terminals {
        terms.insert(
            t.id,
            Term {
                st: t.clone(),
                own_child,
                reader: None,
                status: None,
                resumed_ns: None,
                stall_ns: None,
            },
        );
    }
    State {
        server_id: m.server_id.clone(),
        generation: m.generation,
        listener_fd: m.listener_fd,
        next_id: m.next_id,
        terms,
        history: m.history.clone(),
        rollback_exe: m.rollback_exe.clone(),
    }
}

pub fn fresh_state(opts: &Options) -> State {
    let sock = opts.run_dir.join("control.sock");
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).expect("bind control socket");
    let mut id = [0u8; 8];
    let _ = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut id));
    State {
        server_id: id.iter().map(|b| format!("{b:02x}")).collect(),
        generation: 1,
        listener_fd: listener.into_raw_fd(),
        next_id: 1,
        terms: BTreeMap::new(),
        history: Vec::new(),
        rollback_exe: None,
    }
}

/// Runs the control socket until shutdown. Never returns.
pub fn serve(srv: Arc<Server>) -> ! {
    if let Some(pid) = srv.opts.driver_pid {
        let srv = Arc::clone(&srv);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(500));
                if !sys::alive(pid) {
                    eprintln!("driver {pid} is gone; shutting down");
                    shutdown(&srv);
                }
            }
        });
    }
    let fd = lock(&srv.state).listener_fd;
    let _ = std::fs::write(
        srv.opts.run_dir.join("server.pid"),
        std::process::id().to_string(),
    );
    let listener =
        std::mem::ManuallyDrop::new(unsafe { UnixListener::from_raw_fd(fd) });
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let srv = Arc::clone(&srv);
                std::thread::spawn(move || handle_conn(srv, stream));
            }
            Err(e) => {
                eprintln!("accept: {e}");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn handle_conn(srv: Arc<Server>, stream: UnixStream) {
    let mut out = stream.try_clone().expect("clone stream");
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => json!({"op": "invalid", "error": e.to_string()}),
        };
        let resp = handle(&srv, &req);
        if writeln!(out, "{resp}").is_err() {
            return;
        }
        if req["op"] == "shutdown" {
            shutdown(&srv);
        }
    }
}

fn handle(srv: &Arc<Server>, req: &Value) -> Value {
    let id = req["id"].as_u64().unwrap_or(0) as u32;
    match req["op"].as_str().unwrap_or("") {
        "status" => status(srv),
        "spawn" => match crate::spawn(srv, req) {
            Ok(v) => v,
            Err(e) => json!({"error": e}),
        },
        "write" => {
            let st = lock(&srv.state);
            let Some(t) = st.terms.get(&id) else {
                return json!({"error": "no such terminal"});
            };
            let data = req["data"].as_str().unwrap_or("").as_bytes();
            match sys::write_all(t.st.master_fd, data, Duration::from_secs(5)) {
                Ok(()) => json!({"ok": true}),
                Err(e) => json!({"error": e.to_string()}),
            }
        }
        "resize" => {
            let mut st = lock(&srv.state);
            let Some(t) = st.terms.get_mut(&id) else {
                return json!({"error": "no such terminal"});
            };
            t.st.rows = req["rows"].as_u64().unwrap_or(24) as u16;
            t.st.cols = req["cols"].as_u64().unwrap_or(80) as u16;
            let _ = sys::set_winsize(t.st.master_fd, t.st.rows, t.st.cols);
            json!({"ok": true})
        }
        "close" => {
            let reader = lock(&srv.state)
                .terms
                .get_mut(&id)
                .and_then(|t| t.reader.take());
            if let Some(w) = reader {
                w.stop();
            }
            let mut st = lock(&srv.state);
            let Some(t) = st.terms.get_mut(&id) else {
                return json!({"error": "no such terminal"});
            };
            if !t.st.closed {
                sys::close(t.st.master_fd);
                t.st.closed = true;
            }
            json!({"ok": true})
        }
        "handover" => {
            let srv = Arc::clone(srv);
            let req = req.clone();
            std::thread::spawn(move || crate::handover(&srv, &req));
            json!({"started": true})
        }
        "shutdown" => json!({"ok": true}),
        other => json!({"error": format!("unknown op {other}")}),
    }
}

fn status(srv: &Arc<Server>) -> Value {
    let st = lock(&srv.state);
    let terms: Vec<Value> = st
        .terms
        .values()
        .map(|t| {
            let exit = match t.st.exit {
                Some(Exit::Status(raw)) => sys::decode_status(raw),
                Some(Exit::Unknown) => json!("unknown"),
                None => Value::Null,
            };
            json!({
                "id": t.st.id,
                "pid": t.st.pid,
                "shim_pid": t.st.shim_pid,
                "bytes": t.st.bytes,
                "exit": exit,
                "eof": t.st.eof,
                "closed": t.st.closed,
                "rows": t.st.rows,
                "cols": t.st.cols,
                "fg_pgrp": if t.st.closed { -1 } else { sys::tcgetpgrp(t.st.master_fd) },
                "sid": sys::getsid(t.st.pid),
                "alive": sys::alive(t.st.pid),
                "stall_ns": t.stall_ns,
                "master_cloexec": if t.st.closed { Value::Null } else { json!(sys::is_cloexec(t.st.master_fd).unwrap_or(false)) },
                "capture": t.st.capture,
            })
        })
        .collect();
    json!({
        "server_id": st.server_id,
        "build_id": srv.opts.build_id,
        "approach": srv.opts.approach.name(),
        "pid": std::process::id(),
        "generation": st.generation,
        "listener_cloexec": sys::is_cloexec(st.listener_fd).unwrap_or(false),
        "history": st.history,
        "terminals": terms,
    })
}

fn shutdown(srv: &Arc<Server>) -> ! {
    let mut st = lock(&srv.state);
    for t in st.terms.values_mut() {
        if !t.st.closed {
            sys::close(t.st.master_fd);
            t.st.closed = true;
        }
    }
    // Give shells time to forward the hangup to their jobs before SIGKILL.
    std::thread::sleep(Duration::from_millis(500));
    for t in st.terms.values() {
        if sys::alive(t.st.pid) {
            let _ = sys::kill(-t.st.pid, libc::SIGKILL);
            let _ = sys::kill(t.st.pid, libc::SIGKILL);
        }
        if t.own_child {
            let _ = sys::reap_blocking(t.st.shim_pid.unwrap_or(t.st.pid));
        }
    }
    std::process::exit(0)
}

/// Closes a listener descriptor's Rust owner without closing the descriptor.
pub fn raw(listener: &UnixListener) -> RawFd {
    listener.as_raw_fd()
}

/// Appends a handover record, keeping the most recent outcome last.
pub fn push_history(st: &mut State, record: Value) {
    st.history.push(record);
}

pub fn ns_to_ms(ns: u64) -> f64 {
    ns as f64 / 1e6
}
