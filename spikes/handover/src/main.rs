//! Throwaway harness for #394: hand live PTYs from one server image to a new
//! one, by exec in place (A) or by descriptor passing with a shim (B).

mod approach_a;
mod approach_b;
mod driver;
mod manifest;
mod server;
mod shim_core;
mod sys;
mod tools;

use std::sync::{Arc, Mutex};

use serde_json::Value;

use server::{Approach, Server, State, Term, Worker, lock};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rest = args.get(2..).unwrap_or_default().to_vec();
    match args.get(1).map(String::as_str) {
        Some("server") => run_server(server::Options::parse(&rest)),
        Some("check-handover") => {
            std::process::exit(approach_a::check_handover(&rest))
        }
        Some("pty-shim") => {
            shim_core::run(std::env::args_os().skip(2).collect())
        }
        Some("gen") => std::process::exit(tools::gen_output(&rest)),
        Some("tui") => std::process::exit(tools::tui()),
        Some("driver") => std::process::exit(driver::main(&rest)),
        _ => {
            eprintln!(
                "usage: handover-spike server|check-handover|pty-shim|gen|tui|driver ..."
            );
            std::process::exit(2);
        }
    }
}

fn run_server(opts: server::Options) -> ! {
    if opts.resume_fd.is_some() {
        approach_a::resume(opts);
    }
    if opts.import_socket.is_some() {
        approach_b::import(opts);
    }
    let _ = std::fs::create_dir_all(&opts.run_dir);
    let mut st = server::fresh_state(&opts);
    if opts.approach == Approach::A {
        st.rollback_exe = approach_a::prepare_rollback(&opts).0;
    }
    let srv = Arc::new(Server {
        opts,
        state: Mutex::new(st),
        handover_lock: Mutex::new(()),
    });
    server::serve(srv)
}

pub fn spawn(srv: &Arc<Server>, req: &Value) -> Result<Value, String> {
    let mut st = lock(&srv.state);
    match srv.opts.approach {
        Approach::A => approach_a::spawn(srv, &mut st, req),
        Approach::B => approach_b::spawn(srv, &mut st, req),
    }
}

pub fn exit_watch(srv: &Arc<Server>, t: &Term) -> Worker {
    match srv.opts.approach {
        Approach::A => approach_a::exit_watch(srv, t),
        Approach::B => approach_b::exit_watch(srv, t),
    }
}

pub fn handover(srv: &Arc<Server>, req: &Value) {
    match srv.opts.approach {
        Approach::A => approach_a::handover(srv, req),
        Approach::B => approach_b::handover(srv, req),
    }
}

/// Test hook: make one terminal's program exit while readers are paused,
/// then wait until its status is waiting to be collected, unread.
pub fn signal_during_pause(st: &State, req: &Value, approach: Approach) {
    let Some(id) = req["signal_during_pause"]["id"].as_u64() else {
        return;
    };
    let sig = req["signal_during_pause"]["signal"]
        .as_i64()
        .unwrap_or(libc::SIGUSR1 as i64) as i32;
    let Some(t) = st.terms.get(&(id as u32)) else {
        return;
    };
    let _ = sys::kill(t.st.pid, sig);
    let deadline = sys::now_ns() + 3_000_000_000;
    while sys::now_ns() < deadline {
        let done = match approach {
            Approach::A => sys::is_zombie_child(t.st.pid),
            Approach::B => t.st.status_fd.is_some_and(|fd| {
                sys::poll(&mut [sys::pollfd(fd, libc::POLLIN)], 0).unwrap_or(0)
                    > 0
            }),
        };
        if done {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    eprintln!("signal_during_pause: terminal {id} did not exit in time");
}
