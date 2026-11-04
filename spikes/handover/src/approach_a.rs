//! Approach A: exec in place. Shells stay this PID's children across exec.

use std::ffi::CString;
use std::io::{Read, Seek, Write};
use std::os::fd::{FromRawFd, IntoRawFd, RawFd};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use portable_pty::{CommandBuilder, PtySize};
use serde_json::{Value, json};

use crate::manifest::{self, Exit, Manifest, TermState, Timing};
use crate::server::{self, Options, Server, State, Term, Worker, lock};
use crate::sys;

/// Spawns through the vendored portable-pty, exactly as Huterm does.
pub fn spawn(
    srv: &Arc<Server>,
    st: &mut State,
    req: &Value,
) -> Result<Value, String> {
    let rows = req["rows"].as_u64().unwrap_or(24) as u16;
    let cols = req["cols"].as_u64().unwrap_or(80) as u16;
    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;
    let master_raw = pair.master.as_raw_fd().ok_or("no master fd")?;
    sys::set_nonblocking(master_raw).map_err(|e| e.to_string())?;
    let argv: Vec<String> = serde_json::from_value(req["argv"].clone())
        .map_err(|e| e.to_string())?;
    let mut builder = CommandBuilder::new(&argv[0]);
    builder.args(&argv[1..]);
    if let Some(cwd) = req["cwd"].as_str() {
        builder.cwd(cwd);
    }
    if let Some(env) = req["env"].as_object() {
        for (k, v) in env {
            builder.env(k, v.as_str().unwrap_or(""));
        }
    }
    let child = pair
        .slave
        .spawn_command(builder)
        .map_err(|e| e.to_string())?;
    let pid = child.process_id().ok_or("no pid")? as i32;
    // Own a plain duplicate of the master. Dropping portable-pty's master
    // closes its descriptor without the writer's newline-and-EOF injection.
    let master = sys::dup_cloexec(master_raw).map_err(|e| e.to_string())?;
    drop(pair);
    drop(child);
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
            shim_pid: None,
            master_fd: master,
            status_fd: None,
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
    Ok(json!({"id": id, "pid": pid}))
}

/// Observes the shell's exit with `WNOWAIT` and reaps only under the state
/// lock, so a handover that holds the lock never loses a reaped status: an
/// exit during the cutover stays a zombie for the next image to reap.
pub fn exit_watch(srv: &Arc<Server>, t: &Term) -> Worker {
    let srv = Arc::clone(srv);
    let (id, pid) = (t.st.id, t.st.pid);
    Worker::spawn(format!("wait-{pid}"), move |_stop| {
        if sys::wait_exit_nowait(pid).is_err() {
            return;
        }
        let mut st = lock(&srv.state);
        if let Some(t) = st.terms.get_mut(&id) {
            if t.st.exit.is_none() {
                if let Some(raw) = sys::try_reap(pid) {
                    t.st.exit = Some(Exit::Status(raw));
                }
            }
        }
    })
}

/// The waiter thread blocks in `waitid` and cannot be woken by its stop pipe,
/// so pausing detaches it instead: it parks on the state lock the handover
/// holds and dies with the old image at `execve`.
pub fn detach_waiters(st: &mut State) {
    for t in st.terms.values_mut() {
        if let Some(w) = t.status.take() {
            std::mem::forget(w);
        }
    }
}

/// Copies the running executable so a rejected handover can exec back into
/// this build. Done at startup: after an upgrade replaces the file, macOS
/// offers no path to the running image.
pub fn prepare_rollback(opts: &Options) -> (Option<String>, f64) {
    let start = sys::now_ns();
    let Ok(exe) = std::env::current_exe() else {
        return (None, 0.0);
    };
    let dir = opts.run_dir.join("rollback");
    let _ = std::fs::create_dir_all(&dir);
    let dest = dir.join(format!("handover-spike-{}", opts.build_id));
    #[cfg(target_os = "linux")]
    let src = PathBuf::from("/proc/self/exe");
    #[cfg(not(target_os = "linux"))]
    let src = exe;
    // Copy then rename: a rolled-back image runs from `dest` itself, and
    // copying onto it would truncate the running executable (macOS) or fail
    // with ETXTBSY (Linux).
    let tmp = dir.join(format!(
        ".handover-spike-{}.{}",
        opts.build_id,
        std::process::id()
    ));
    match std::fs::copy(&src, &tmp).and_then(|_| std::fs::rename(&tmp, &dest)) {
        Ok(_) => (
            Some(dest.display().to_string()),
            server::ns_to_ms(sys::now_ns() - start),
        ),
        Err(e) => {
            eprintln!("rollback copy failed: {e}");
            (None, 0.0)
        }
    }
}

pub fn handover(srv: &Arc<Server>, req: &Value) {
    let _h = lock(&srv.handover_lock);
    let begin = sys::now_ns();
    let exe = req["exe"].as_str().unwrap_or("").to_string();
    let extra: Vec<String> =
        serde_json::from_value(req["args"].clone()).unwrap_or_default();
    let mut timing = Timing {
        begin_ns: begin,
        target_build: target_build(&extra),
        nudge: req["nudge"].as_str().unwrap_or("shrink").into(),
        source_pid: std::process::id() as i32,
        ..Timing::default()
    };
    if req["precheck"].as_bool().unwrap_or(false) {
        let provisional = srv.manifest(&lock(&srv.state), timing.clone());
        let result = precheck(&exe, &extra, &provisional);
        timing.precheck_ns = Some(sys::now_ns());
        if let Err(reason) = result {
            let rec = record(
                &timing,
                "precheck_rejected",
                &reason,
                "nothing changed; source kept running",
            );
            server::push_history(&mut lock(&srv.state), rec);
            return;
        }
    }

    timing.pause_start_ns = sys::now_ns();
    let readers = server::take_workers(&mut lock(&srv.state), false);
    server::stop_all(readers);
    // From here the lock stays held until execve, so no waiter can reap.
    let mut st = lock(&srv.state);
    detach_waiters(&mut st);
    crate::signal_during_pause(&st, req, Approach::A);
    timing.paused_ns = sys::now_ns();

    let manifest = srv.manifest(&st, timing.clone());
    let state_fd = match write_state(&srv.opts, &manifest) {
        Ok(fd) => fd,
        Err(e) => {
            return fail(srv, &mut st, &timing, &format!("state write: {e}"));
        }
    };
    let mut inherited: Vec<RawFd> = st
        .terms
        .values()
        .filter(|t| !t.st.closed)
        .map(|t| t.st.master_fd)
        .collect();
    inherited.push(st.listener_fd);
    inherited.push(state_fd);
    for fd in &inherited {
        let _ = sys::set_cloexec(*fd, false);
    }

    let mut argv = vec![
        "handover-spike".to_string(),
        "server".into(),
        "--approach".into(),
        "a".into(),
    ];
    argv.extend(["--run-dir".into(), srv.opts.run_dir.display().to_string()]);
    argv.extend(extra.iter().cloned());
    if let Some(rb) = &st.rollback_exe {
        argv.extend(["--rollback-exe".into(), rb.clone()]);
    }
    argv.extend(["--source-build".into(), srv.opts.build_id.clone()]);
    if let Some(p) = srv.opts.driver_pid {
        argv.extend(["--driver-pid".into(), p.to_string()]);
    }
    argv.extend(["--resume-fd".into(), state_fd.to_string()]);
    argv.extend(["--handoff-ns".into(), sys::now_ns().to_string()]);
    let err = exec(&exe, &argv);

    for fd in &inherited {
        let _ = sys::set_cloexec(*fd, true);
    }
    sys::close(state_fd);
    fail(srv, &mut st, &timing, &format!("execve {exe}: {err}"));
}

fn fail(srv: &Arc<Server>, st: &mut State, timing: &Timing, reason: &str) {
    server::start_workers(srv, st);
    let rec =
        record(timing, "exec_failed", reason, "source image kept running");
    server::push_history(st, rec);
}

fn target_build(extra: &[String]) -> String {
    extra
        .windows(2)
        .filter(|w| w[0] == "--build-id")
        .map(|w| w[1].clone())
        .last()
        .unwrap_or_default()
}

fn record(timing: &Timing, outcome: &str, reason: &str, place: &str) -> Value {
    json!({
        "approach": "a",
        "outcome": outcome,
        "reason": reason,
        "where": place,
        "timing": timing,
        "source_pid": timing.source_pid,
        "target_pid": std::process::id(),
    })
}

fn exec(exe: &str, argv: &[String]) -> std::io::Error {
    let Ok(path) = CString::new(exe) else {
        return std::io::Error::other("bad path");
    };
    let cargs: Vec<CString> = argv
        .iter()
        .map(|a| CString::new(a.as_str()).unwrap())
        .collect();
    let mut ptrs: Vec<*const libc::c_char> =
        cargs.iter().map(|c| c.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    unsafe { libc::execv(path.as_ptr(), ptrs.as_ptr()) };
    std::io::Error::last_os_error()
}

/// An unlinked temporary file: a pipe's buffer is too small for a large
/// manifest, and nothing is left on disk if either image dies.
fn write_state(opts: &Options, m: &Manifest) -> std::io::Result<RawFd> {
    let path = opts
        .run_dir
        .join(format!("state-{}.json", std::process::id()));
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    std::fs::remove_file(&path)?;
    // One write: serde_json's writer issues a syscall per token otherwise.
    f.write_all(&serde_json::to_vec(m)?)?;
    f.rewind()?;
    Ok(f.into_raw_fd())
}

fn precheck(exe: &str, extra: &[String], m: &Manifest) -> Result<(), String> {
    let mut child = std::process::Command::new(exe)
        .arg("check-handover")
        .args(extra)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn {exe}: {e}"))?;
    let body = serde_json::to_vec(m).unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&body)
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

/// `check-handover`: validate a manifest on stdin without touching anything.
pub fn check_handover(args: &[String]) -> i32 {
    let opts = Options::parse(args);
    let mut body = String::new();
    let _ = std::io::stdin().read_to_string(&mut body);
    match manifest::parse(&body, opts.require_field.as_deref())
        .and_then(|m| manifest::validate(&m, &opts.supported_formats, "a"))
    {
        Ok(()) => 0,
        Err(reason) => {
            println!("{reason}");
            3
        }
    }
}

/// Entry point of the new image after `execve`.
pub fn resume(opts: Options) -> ! {
    let import_start = sys::now_ns();
    let fd = opts.resume_fd.expect("resume fd");
    let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
    let _ = f.rewind();
    let mut body = String::new();
    let _ = f.read_to_string(&mut body);
    let parsed = manifest::parse(&body, opts.require_field.as_deref());
    let verdict = parsed
        .as_ref()
        .map_err(|e| format!("parse state: {e}"))
        .and_then(|m| manifest::validate(m, &opts.supported_formats, "a"));

    if let Err(reason) = verdict {
        // Leave every descriptor inheritable and exec the source build back.
        // The rollback target comes from argv, not the manifest, so this
        // works even when the manifest did not parse.
        let Some(rollback) = opts
            .rollback_exe
            .clone()
            .filter(|_| opts.rollback_reason.is_none())
        else {
            eprintln!("rejected state and no rollback available: {reason}");
            std::process::abort();
        };
        let mut argv = vec![
            "handover-spike".to_string(),
            "server".into(),
            "--approach".into(),
            "a".into(),
        ];
        argv.extend(["--run-dir".into(), opts.run_dir.display().to_string()]);
        argv.extend([
            "--build-id".into(),
            opts.source_build.clone().unwrap_or_default(),
        ]);
        argv.extend(["--rollback-exe".into(), rollback.clone()]);
        if let Some(p) = opts.driver_pid {
            argv.extend(["--driver-pid".into(), p.to_string()]);
        }
        argv.extend([
            "--resume-fd".into(),
            fd.to_string(),
            "--rollback-reason".into(),
            reason,
        ]);
        argv.extend([
            "--handoff-ns".into(),
            opts.handoff_ns.unwrap_or_default().to_string(),
        ]);
        let _ = f.into_raw_fd();
        let err = exec(&rollback, &argv);
        eprintln!("rollback exec failed: {err}");
        std::process::abort();
    }
    let m = parsed.expect("validated above");
    if opts.crash_on_import {
        std::process::abort();
    }

    for t in &m.terminals {
        if !t.closed {
            let _ = sys::set_cloexec(t.master_fd, true);
        }
    }
    let _ = sys::set_cloexec(m.listener_fd, true);
    drop(f);

    let rolled_back = opts.rollback_reason.is_some();
    let mut st = server::restore_state(&opts, &m, true);
    if !rolled_back {
        st.generation += 1;
    }
    let restored = sys::now_ns();
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
    let (outcome, reason, place) = match &opts.rollback_reason {
        Some(r) => (
            "rolled_back",
            r.clone(),
            "source build re-executed from its rollback copy; same PID",
        ),
        None => ("ok", String::new(), "new image, same PID"),
    };
    let mut rec = record(&m.handover, outcome, &reason, place);
    rec["handoff_ns"] = json!(opts.handoff_ns);
    rec["import_start_ns"] = json!(import_start);
    rec["restored_ns"] = json!(restored);
    rec["resumed_ns"] = json!(resumed);
    rec["nudged_ns"] = json!(nudged);
    rec["cutover_ms"] = json!(server::ns_to_ms(resumed - m.handover.begin_ns));
    rec["blackout_ms"] =
        json!(server::ns_to_ms(resumed - m.handover.pause_start_ns));
    rec["terminals"] = json!(m.terminals.len());
    server::push_history(&mut st, rec);
    drop(st);

    let (rollback, copy_ms) = prepare_rollback(&opts);
    {
        let mut st = lock(&srv.state);
        st.rollback_exe = rollback;
        if let Some(last) = st.history.last_mut() {
            last["rollback_copy_ms"] = json!(copy_ms);
        }
    }
    server::serve(srv)
}

use crate::server::Approach;
