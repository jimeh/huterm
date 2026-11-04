//! Runs the #394 scenarios and measurements against a real server process.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::sys;

pub fn main(args: &[String]) -> i32 {
    let flag = |name: &str| {
        args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
    };
    let approach = flag("--approach").unwrap_or_else(|| "a".into());
    match args.first().map(String::as_str) {
        Some("scenario") => {
            let name = args.get(1).cloned().unwrap_or_else(|| "all".into());
            let names: Vec<&str> = if name == "all" {
                vec!["jobcontrol", "tui", "heavy", "exit", "close", "failure"]
            } else {
                vec![name.as_str()]
            };
            let mut ok = true;
            for n in names {
                let r = run_scenario(n, &approach);
                ok &= r["pass"].as_bool().unwrap_or(false);
                println!("{r}");
                summarize(&r);
            }
            if ok { 0 } else { 1 }
        }
        Some("bench") => {
            let counts: Vec<usize> = flag("--terminals")
                .unwrap_or_else(|| "1,16,128".into())
                .split(',')
                .filter_map(|s| s.parse().ok())
                .collect();
            let repeat =
                flag("--repeat").and_then(|s| s.parse().ok()).unwrap_or(10);
            let precheck = args.iter().any(|a| a == "--precheck");
            for n in counts {
                let r = bench(&approach, n, repeat, precheck);
                println!("{r}");
            }
            0
        }
        Some("rss") => {
            let count =
                flag("--count").and_then(|s| s.parse().ok()).unwrap_or(16);
            println!(
                "{}",
                rss(count, flag("--huterm-exe"), flag("--shim-exe"))
            );
            0
        }
        _ => {
            eprintln!(
                "usage: driver scenario <name|all> --approach a|b | bench | rss"
            );
            2
        }
    }
}

fn summarize(r: &Value) {
    eprintln!(
        "== {} [{}] {}: {}",
        r["scenario"].as_str().unwrap_or(""),
        r["approach"].as_str().unwrap_or(""),
        r["platform"].as_str().unwrap_or(""),
        if r["pass"].as_bool().unwrap_or(false) {
            "PASS"
        } else {
            "FAIL"
        }
    );
    for c in r["checks"].as_array().into_iter().flatten() {
        eprintln!(
            "   {} {}: {}",
            if c["pass"].as_bool().unwrap_or(false) {
                "ok  "
            } else {
                "FAIL"
            },
            c["name"].as_str().unwrap_or(""),
            c["detail"]
        );
    }
}

pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

// ---------------------------------------------------------------- harness

pub struct Env {
    pub exe: PathBuf,
    sessions: BTreeSet<i32>,
    pub approach: String,
    pub run_dir: PathBuf,
    server: Child,
    pids: BTreeSet<i32>,
    shim_exe: Option<String>,
}

impl Env {
    pub fn start(approach: &str, server_args: &[&str]) -> Env {
        // HS_SERVER_EXE runs the server from another path, such as a FUSE mount.
        let exe = std::env::var_os("HS_SERVER_EXE")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_exe().unwrap());
        let mut rnd = [0u8; 4];
        let _ = std::fs::File::open("/dev/urandom")
            .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut rnd));
        let tag: String = rnd.iter().map(|b| format!("{b:02x}")).collect();
        let run_dir =
            std::env::temp_dir().join(format!("hs394-{approach}-{tag}"));
        std::fs::create_dir_all(&run_dir).unwrap();
        let log = std::fs::File::create(run_dir.join("server.log")).unwrap();
        let shim_exe = server_args
            .windows(2)
            .find(|w| w[0] == "--shim-exe")
            .map(|w| w[1].to_string());
        let server = Command::new(&exe)
            .args(["server", "--approach", approach, "--run-dir"])
            .arg(&run_dir)
            .args(["--build-id", "v1"])
            .args(server_args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .process_group(0)
            .spawn()
            .unwrap();
        let mut env = Env {
            exe,
            sessions: BTreeSet::new(),
            approach: approach.into(),
            run_dir,
            server,
            pids: BTreeSet::new(),
            shim_exe,
        };
        env.pids.insert(env.server.id() as i32);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline
            && UnixStream::connect(env.sock()).is_err()
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        env
    }

    fn sock(&self) -> PathBuf {
        self.run_dir.join("control.sock")
    }

    /// One request per connection; retries while the server is replacing
    /// itself.
    pub fn req(&mut self, v: Value) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(r) = self.try_req(&v) {
                return r;
            }
            if Instant::now() > deadline {
                return json!({"error": "server unreachable"});
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn try_req(&mut self, v: &Value) -> Option<Value> {
        let mut s = UnixStream::connect(self.sock()).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
        writeln!(s, "{v}").ok()?;
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).ok()?;
        if line.is_empty() {
            return None;
        }
        let r: Value = serde_json::from_str(&line).ok()?;
        if v["op"] == "status" {
            if let Some(p) = r["pid"].as_i64() {
                self.pids.insert(p as i32);
            }
            for t in r["terminals"].as_array().into_iter().flatten() {
                for k in ["pid", "shim_pid"] {
                    if let Some(p) = t[k].as_i64() {
                        self.pids.insert(p as i32);
                    }
                }
            }
        }
        Some(r)
    }

    pub fn status(&mut self) -> Value {
        self.req(json!({"op": "status"}))
    }

    pub fn term(&mut self, id: u64) -> Value {
        let st = self.status();
        st["terminals"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| t["id"] == id)
            .cloned()
            .unwrap_or(Value::Null)
    }

    pub fn shell_env(&self) -> Value {
        json!({
            "PS1": "$ ",
            "HOME": self.run_dir.display().to_string(),
            "HISTFILE": "/dev/null",
            "BASH_SILENCE_DEPRECATION_WARNING": "1",
            "TERM": "xterm-256color",
        })
    }

    pub fn spawn(&mut self, argv: &[&str]) -> (u64, i64) {
        let env = self.shell_env();
        let r = self.req(json!({"op": "spawn", "argv": argv, "env": env, "cwd": self.run_dir.display().to_string()}));
        if let Some(p) = r["pid"].as_i64() {
            self.pids.insert(p as i32);
            // Every terminal program leads its own session.
            self.sessions.insert(p as i32);
        }
        if let Some(p) = r["shim_pid"].as_i64() {
            self.pids.insert(p as i32);
        }
        (
            r["id"]
                .as_u64()
                .unwrap_or_else(|| panic!("spawn failed: {r}")),
            r["pid"].as_i64().unwrap_or(0),
        )
    }

    pub fn bash(&mut self) -> (u64, i64) {
        let r = self.spawn(&["/bin/bash", "--noprofile", "--norc", "-i"]);
        let _ = self.roundtrip(r.0, "READY");
        r
    }

    pub fn write(&mut self, id: u64, data: &str) {
        self.req(json!({"op": "write", "id": id, "data": data}));
    }

    pub fn capture(&self, id: u64) -> Vec<u8> {
        std::fs::read(self.run_dir.join(format!("term-{id}.capture")))
            .unwrap_or_default()
    }

    /// Waits until `needle` appears at or after `from`; returns the offset
    /// just past it.
    pub fn expect(
        &self,
        id: u64,
        needle: &str,
        from: usize,
        timeout: Duration,
    ) -> Result<usize, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let cap = self.capture(id);
            if let Some(pos) =
                find(&cap[from.min(cap.len())..], needle.as_bytes())
            {
                return Ok(from + pos + needle.len());
            }
            if Instant::now() > deadline {
                let tail = &cap[cap.len().saturating_sub(300)..];
                return Err(format!(
                    "timed out waiting for {needle:?}; tail {:?}",
                    String::from_utf8_lossy(tail)
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Types a command whose output differs from its echo and waits for it.
    pub fn roundtrip(&mut self, id: u64, tag: &str) -> Result<usize, String> {
        let from = self.capture(id).len();
        self.write(id, &format!("echo {tag}-$((40+2))\n"));
        self.expect(id, &format!("{tag}-42"), from, Duration::from_secs(5))
    }

    pub fn wait_term(
        &mut self,
        id: u64,
        timeout: Duration,
        pred: impl Fn(&Value) -> bool,
    ) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let t = self.term(id);
            if pred(&t) {
                return Some(t);
            }
            if Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Requests a handover to `args` and waits for its outcome record.
    pub fn handover(&mut self, args: &[&str], extra: Value) -> Value {
        let before = self.status()["history"].as_array().map_or(0, Vec::len);
        let mut args: Vec<String> =
            args.iter().map(|s| s.to_string()).collect();
        if let Some(shim) = &self.shim_exe {
            args.extend(["--shim-exe".into(), shim.clone()]);
        }
        let mut req = json!({"op": "handover", "exe": self.exe.display().to_string(), "args": args, "nudge": "shrink"});
        if let (Some(obj), Some(more)) =
            (req.as_object_mut(), extra.as_object())
        {
            for (k, v) in more {
                obj.insert(k.clone(), v.clone());
            }
        }
        self.req(req);
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Some(r) = self.try_req(&json!({"op": "status"})) {
                if let Some(h) = r["history"].as_array() {
                    if h.len() > before {
                        return h.last().cloned().unwrap();
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        json!({"outcome": "no_record", "reason": "server unreachable or never recorded the handover"})
    }

    pub fn cleanup(mut self) -> Value {
        let _ = self.try_req(&json!({"op": "status"}));
        let _ = self.try_req(&json!({"op": "shutdown"}));
        std::thread::sleep(Duration::from_millis(200));
        for p in &self.pids {
            if sys::alive(*p) {
                let _ = sys::kill(-*p, libc::SIGKILL);
                let _ = sys::kill(*p, libc::SIGKILL);
            }
        }
        let _ = self.server.kill();
        let _ = self.server.wait();
        // Background jobs live in the shells' sessions but not their groups.
        for p in session_members(&self.sessions) {
            let _ = sys::kill(p, libc::SIGKILL);
        }
        std::thread::sleep(Duration::from_millis(200));
        let mut left: Vec<i32> = self
            .pids
            .iter()
            .copied()
            .filter(|p| sys::alive(*p) && !is_zombie(*p))
            .collect();
        left.extend(
            session_members(&self.sessions)
                .into_iter()
                .filter(|p| !is_zombie(*p)),
        );
        left.sort();
        left.dedup();
        if std::env::var_os("HS_KEEP").is_none() {
            let _ = std::fs::remove_dir_all(&self.run_dir);
        }
        json!({"tracked": self.pids.len(), "left_alive": left})
    }
}

/// Every process whose session is one of `sessions`; neither macOS `pgrep`
/// nor its `ps` can select by session ID, so ask `getsid` for each process.
fn session_members(sessions: &BTreeSet<i32>) -> Vec<i32> {
    let out = Command::new("ps").args(["-A", "-o", "pid="]).output();
    let text = out
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    text.split_whitespace()
        .filter_map(|s| s.parse::<i32>().ok())
        .filter(|p| sessions.contains(&sys::getsid(*p)))
        .collect()
}

fn is_zombie(pid: i32) -> bool {
    Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim_start()
                .starts_with('Z')
        })
        .unwrap_or(false)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

fn ppid(pid: i64) -> i64 {
    Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(-1)
}

struct Checks(Vec<Value>);

impl Checks {
    fn add(
        &mut self,
        name: &str,
        pass: bool,
        detail: impl Into<Value>,
    ) -> bool {
        self.0
            .push(json!({"name": name, "pass": pass, "detail": detail.into()}));
        pass
    }
    fn res(&mut self, name: &str, r: Result<usize, String>) -> usize {
        match r {
            Ok(p) => {
                self.add(name, true, "seen");
                p
            }
            Err(e) => {
                self.add(name, false, e);
                0
            }
        }
    }
    fn outcome(&mut self, name: &str, rec: &Value, want: &str) -> bool {
        let got = rec["outcome"].as_str().unwrap_or("");
        self.add(name, got == want, json!({"outcome": got, "reason": rec["reason"], "where": rec["where"], "cutover_ms": rec["cutover_ms"]}))
    }
}

fn run_scenario(name: &str, approach: &str) -> Value {
    let mut env = Env::start(approach, &[]);
    let mut c = Checks(Vec::new());
    let mut metrics = json!({});
    let started = Instant::now();
    match name {
        "jobcontrol" => jobcontrol(&mut env, &mut c, &mut metrics),
        "tui" => tui(&mut env, &mut c, &mut metrics),
        "heavy" => heavy(&mut env, &mut c, &mut metrics),
        "exit" => exit(&mut env, &mut c, &mut metrics),
        "close" => close(&mut env, &mut c, &mut metrics),
        "failure" => failure(&mut env, &mut c, &mut metrics),
        "appimage" => appimage(&mut env, &mut c, &mut metrics),
        other => {
            c.add("known scenario", false, other);
        }
    }
    let history = env.status()["history"].clone();
    let cleanup = env.cleanup();
    let pass = c.0.iter().all(|x| x["pass"].as_bool().unwrap_or(false))
        && cleanup["left_alive"].as_array().is_some_and(Vec::is_empty);
    json!({
        "scenario": name,
        "approach": approach,
        "platform": platform(),
        "pass": pass,
        "checks": c.0,
        "metrics": metrics,
        "history": history,
        "cleanup": cleanup,
        "elapsed_s": started.elapsed().as_secs_f64(),
    })
}

const T5: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------- scenarios

fn jobcontrol(env: &mut Env, c: &mut Checks, m: &mut Value) {
    let (id, pid) = env.bash();
    let server_before = env.status()["pid"].as_i64();
    let ppid_before = ppid(pid);
    env.write(id, "sleep 300 &\n");
    let r = env.roundtrip(id, "BG");
    c.res("background job started", r);
    let rec = env.handover(&["--build-id", "v2"], json!({}));
    c.outcome("handover", &rec, "ok");
    let t = env.term(id);
    c.add(
        "shell still session leader",
        t["sid"] == pid,
        json!({"sid": t["sid"], "pid": pid}),
    );
    c.add("shell alive", t["alive"] == true, t["alive"].clone());
    let ppid_after = ppid(pid);
    m["parent"] = json!({"before": ppid_before, "after": ppid_after, "server_before": server_before, "server_after": env.status()["pid"]});

    let from = env.capture(id).len();
    env.write(id, "jobs; echo J-$((2+3))\n");
    let end = c.res("jobs after handover", env.expect(id, "J-5", from, T5));
    let slice = String::from_utf8_lossy(&env.capture(id)[from..end.max(from)])
        .to_string();
    c.add(
        "background job still running",
        slice.contains("Running"),
        slice.clone(),
    );

    env.write(id, "fg %1\n");
    let fg = env.wait_term(id, T5, |t| {
        t["fg_pgrp"].as_i64().is_some_and(|g| g > 0 && g != pid)
    });
    c.add(
        "fg gives the job the terminal",
        fg.is_some(),
        env.term(id)["fg_pgrp"].clone(),
    );
    let from = env.capture(id).len();
    env.write(id, "\x1a");
    c.res("ctrl-z stops the job", env.expect(id, "Stopped", from, T5));
    let back = env.wait_term(id, T5, |t| t["fg_pgrp"].as_i64() == Some(pid));
    c.add(
        "shell regains the terminal",
        back.is_some(),
        env.term(id)["fg_pgrp"].clone(),
    );
    let from = env.capture(id).len();
    env.write(id, "bg %1; jobs; echo BGAGAIN-$((3+4))\n");
    let end =
        c.res("bg resumes the job", env.expect(id, "BGAGAIN-7", from, T5));
    let slice = String::from_utf8_lossy(&env.capture(id)[from..end.max(from)])
        .to_string();
    c.add("job running again", slice.contains("Running"), slice);

    env.write(id, "fg %1\n");
    let fg = env.wait_term(id, T5, |t| {
        t["fg_pgrp"].as_i64().is_some_and(|g| g > 0 && g != pid)
    });
    c.add("fg again", fg.is_some(), "");
    env.write(id, "\x03");
    let back = env.wait_term(id, T5, |t| t["fg_pgrp"].as_i64() == Some(pid));
    c.add(
        "ctrl-c interrupts the foreground job",
        back.is_some(),
        env.term(id)["fg_pgrp"].clone(),
    );
    let from = env.capture(id).len();
    env.write(id, "jobs; echo J3-$((5+5))\n");
    let end = c.res("prompt after ctrl-c", env.expect(id, "J3-10", from, T5));
    let slice = String::from_utf8_lossy(&env.capture(id)[from..end.max(from)])
        .to_string();
    c.add(
        "no jobs left",
        !slice.contains("Running") && !slice.contains("Stopped"),
        slice,
    );
}

fn tui(env: &mut Env, c: &mut Checks, m: &mut Value) {
    let (id, _pid) = env.bash();
    let exe = env.exe.display().to_string();
    env.write(id, &format!("{exe} tui\n"));
    let pos = c.res(
        "tui drew its first frame",
        env.expect(id, "REDRAW 0 24x80", 0, T5),
    );
    let _ = pos;
    let mut modes = json!({});
    for mode in ["none", "same", "sigwinch", "shrink"] {
        let from = env.capture(id).len();
        let rec = env.handover(&["--build-id", "v2"], json!({"nudge": mode}));
        let t0 = Instant::now();
        let seen = env
            .expect(id, "REDRAW", from, Duration::from_millis(1500))
            .is_ok();
        let latency = t0.elapsed().as_secs_f64() * 1000.0;
        std::thread::sleep(Duration::from_millis(100));
        let cap = env.capture(id);
        let tail = &cap[from.min(cap.len())..];
        let last = rfind(tail, b"REDRAW").map(|p| {
            String::from_utf8_lossy(&tail[p..(p + 20).min(tail.len())])
                .to_string()
        });
        modes[mode] = json!({"handover": rec["outcome"], "redrew": seen, "detect_ms": if seen { json!(latency) } else { Value::Null }, "last_frame": last});
        if mode == "shrink" {
            c.add("shrink nudge redraws", seen, modes[mode].clone());
            c.add(
                "final frame has the restored size",
                last.as_deref().is_some_and(|l| l.contains("24x80")),
                last.clone(),
            );
        }
    }
    m["nudge_modes"] = modes;
    env.write(id, "q");
    let _ = env.roundtrip(id, "TUIQUIT");

    // Real programs, when installed.
    for (prog, cmd, quit, sentinel) in [
        (
            "vim",
            "vim -u NONE -N -n VIMFILE\n",
            "\x1b:q!\r",
            Some("VIMSENTINEL"),
        ),
        ("htop", "htop\n", "q", None),
    ] {
        if !Command::new("sh")
            .args(["-c", &format!("command -v {prog}")])
            .output()
            .is_ok_and(|o| o.status.success())
        {
            m[prog] = json!("not installed");
            continue;
        }
        let file = env.run_dir.join("vim.txt");
        std::fs::write(&file, "VIMSENTINEL line one\n").unwrap();
        let from = env.capture(id).len();
        env.write(id, &cmd.replace("VIMFILE", &file.display().to_string()));
        std::thread::sleep(Duration::from_millis(800));
        let _ = env.expect(id, sentinel.unwrap_or("\x1b["), from, T5);
        let from = env.capture(id).len();
        let rec =
            env.handover(&["--build-id", "v2"], json!({"nudge": "shrink"}));
        let ok = match sentinel {
            Some(s) => {
                env.expect(id, s, from, Duration::from_millis(2000)).is_ok()
            }
            None => {
                std::thread::sleep(Duration::from_millis(1500));
                env.capture(id).len() - from > 200
            }
        };
        let bytes = env.capture(id).len() - from;
        m[prog] = json!({"handover": rec["outcome"], "redrew": ok, "bytes_after": bytes});
        c.add(&format!("{prog} redraws after nudge"), ok, m[prog].clone());
        env.write(id, quit);
        let _ = env.roundtrip(id, "QUIT");
    }
}

fn heavy(env: &mut Env, c: &mut Checks, m: &mut Value) {
    const TOTAL: u64 = 50 * 1024 * 1024;
    let exe = env.exe.display().to_string();
    let (id, _pid) = env.spawn(&[&exe, "gen", "--bytes", &TOTAL.to_string()]);
    let start = Instant::now();
    let mut handovers = Vec::new();
    for threshold in [10u64 << 20, 30 << 20] {
        env.wait_term(id, Duration::from_secs(60), |t| {
            t["bytes"].as_u64().unwrap_or(0) >= threshold
        });
        let before = env.term(id)["bytes"].as_u64();
        let rec = env.handover(&["--build-id", "v2"], json!({}));
        let t = env.wait_term(id, Duration::from_secs(5), |t| {
            !t["stall_ns"].is_null()
        });
        handovers.push(json!({
            "outcome": rec["outcome"],
            "bytes_at_request": before,
            "cutover_ms": rec["cutover_ms"],
            "blackout_ms": rec["blackout_ms"],
            "reader_stall_ms": t.as_ref().and_then(|t| t["stall_ns"].as_f64()).map(|n| n / 1e6),
        }));
        c.outcome("handover mid-stream", &rec, "ok");
    }
    let done = env.wait_term(id, Duration::from_secs(120), |t| {
        !t["exit"].is_null() && t["eof"] == true
    });
    let elapsed = start.elapsed().as_secs_f64();
    let t = env.term(id);
    c.add(
        "generator exited 0",
        t["exit"]["code"] == 0,
        t["exit"].clone(),
    );
    c.add("reader saw end of output", done.is_some(), t["eof"].clone());
    let cap = env.capture(id);
    c.add(
        "exact byte count",
        cap.len() as u64 == TOTAL,
        json!({"got": cap.len(), "want": TOTAL}),
    );
    let mut bad = None;
    for (i, chunk) in cap.chunks(8).enumerate() {
        let mut want = (i as u64).to_le_bytes().to_vec();
        want.truncate(chunk.len());
        if chunk != want.as_slice() {
            bad = Some(i * 8);
            break;
        }
    }
    c.add(
        "no lost or duplicated bytes",
        bad.is_none(),
        json!({"first_bad_offset": bad}),
    );
    m["handovers"] = json!(handovers);
    m["elapsed_s"] = json!(elapsed);
    m["throughput_mib_s"] = json!(TOTAL as f64 / 1048576.0 / elapsed);
}

fn exit(env: &mut Env, c: &mut Checks, m: &mut Value) {
    let (a, _) = env.bash();
    let (b, _) = env.bash();
    let (d, dpid) = env.spawn(&[
        "/bin/sh",
        "-c",
        "trap 'exit 7' USR1; while :; do sleep 0.05; done",
    ]);
    std::thread::sleep(Duration::from_millis(200));
    let rec = env.handover(
        &["--build-id", "v2"],
        json!({"signal_during_pause": {"id": d, "signal": libc::SIGUSR1}}),
    );
    c.outcome("handover", &rec, "ok");
    let t = env
        .wait_term(d, T5, |t| !t["exit"].is_null())
        .unwrap_or(Value::Null);
    c.add(
        "exit during the cutover keeps status 7",
        t["exit"]["code"] == 7,
        json!({"exit": t["exit"], "pid": dpid}),
    );

    env.write(a, "exit 42\n");
    let t = env
        .wait_term(a, T5, |t| !t["exit"].is_null() && t["eof"] == true)
        .unwrap_or(Value::Null);
    c.add(
        "exit 42 after handover",
        t["exit"]["code"] == 42,
        json!({"exit": t["exit"], "eof": t["eof"]}),
    );

    env.write(b, "kill -KILL $$\n");
    let t = env
        .wait_term(b, T5, |t| !t["exit"].is_null())
        .unwrap_or(Value::Null);
    c.add(
        "SIGKILL after handover",
        t["exit"]["signal"] == 9,
        t["exit"].clone(),
    );

    let (n, _) = env.bash();
    env.write(n, "exit 42\n");
    let t = env
        .wait_term(n, T5, |t| !t["exit"].is_null())
        .unwrap_or(Value::Null);
    c.add(
        "control: shell spawned by the new server",
        t["exit"]["code"] == 42,
        t["exit"].clone(),
    );
    m["note"] = json!("status read via waitpid in A, via the shim record in B");
}

/// A process's descriptors as (number, target), read from outside it.
fn proc_fd_pairs(pid: i64) -> Vec<(i64, String)> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_dir(format!("/proc/{pid}/fd"))
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let fd =
                            e.file_name().to_string_lossy().parse().ok()?;
                        let link = std::fs::read_link(e.path())
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();
                        Some((fd, link))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let out = Command::new("lsof")
            .args(["-a", "-p", &pid.to_string(), "-d", "0-9999", "-F", "fn"])
            .output();
        let text = out
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        let mut pairs = Vec::new();
        let mut fd = None;
        for line in text.lines() {
            if let Some(v) = line.strip_prefix('f') {
                fd = v.parse().ok();
            } else if let Some(v) = line.strip_prefix('n') {
                if let Some(f) = fd.take() {
                    pairs.push((f, v.to_string()));
                }
            }
        }
        pairs
    }
}

/// A shell may hold stdio and bash's own saved terminal descriptor (255);
/// anything else, above all a PTY master, leaked from the server.
fn leaked_fds(pid: i64) -> (bool, Value) {
    let pairs = proc_fd_pairs(pid);
    let leaked: Vec<String> = pairs
        .iter()
        .filter(|(fd, name)| (*fd > 2 && *fd != 255) || name.contains("ptmx"))
        .map(|(fd, name)| format!("{fd}={name}"))
        .collect();
    let all: Vec<String> = pairs
        .iter()
        .map(|(fd, name)| format!("{fd}={name}"))
        .collect();
    (
        !pairs.is_empty() && leaked.is_empty(),
        json!({"fds": all, "leaked": leaked}),
    )
}

fn close(env: &mut Env, c: &mut Checks, m: &mut Value) {
    let mark = env.run_dir.join("hup-1");
    let script = format!(
        "trap 'echo hup > {}; exit 129' HUP; while :; do sleep 0.05; done",
        mark.display()
    );
    let (h, hpid) = env.spawn(&["/bin/sh", "-c", &script]);
    let (b, bpid) = env.bash();
    let server_before = env.status()["pid"].as_i64().unwrap_or(0);
    let rec = env.handover(&["--build-id", "v2"], json!({}));
    c.outcome("handover", &rec, "ok");
    let (n, npid) = env.bash();

    let (ok, d) = leaked_fds(bpid);
    c.add("pre-handover shell holds no stray descriptors", ok, d);
    let (ok, d) = leaked_fds(npid);
    c.add("post-handover shell inherits no masters", ok, d);
    let st = env.status();
    if env.approach == "b" {
        let shims: Vec<Value> = st["terminals"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| json!({"id": t["id"], "shim_pid": t["shim_pid"], "fds": proc_fd_pairs(t["shim_pid"].as_i64().unwrap_or(0)).iter().map(|(f, n)| format!("{f}={n}")).collect::<Vec<_>>()}))
            .collect();
        m["shim_fds"] = json!(shims);
        c.add(
            "source server exited",
            !sys::alive(server_before as i32)
                || is_zombie(server_before as i32),
            server_before,
        );
    } else {
        c.add(
            "same server PID",
            st["pid"].as_i64() == Some(server_before),
            json!({"before": server_before, "after": st["pid"]}),
        );
    }

    for (id, pid, label) in [
        (h, hpid, "trap script"),
        (b, bpid, "pre-handover bash"),
        (n, npid, "post-handover bash"),
    ] {
        env.req(json!({"op": "close", "id": id}));
        let t = env
            .wait_term(id, T5, |t| !t["exit"].is_null())
            .unwrap_or(Value::Null);
        let gone = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(20));
            !sys::alive(pid as i32) || is_zombie(pid as i32)
        });
        c.add(
            &format!("close hangs up the {label}"),
            !t["exit"].is_null() && gone,
            json!({"exit": t["exit"], "gone": gone}),
        );
        if id == h {
            c.add("trap saw SIGHUP", mark.exists(), mark.exists());
            c.add(
                "trap exit status 129",
                t["exit"]["code"] == 129,
                t["exit"].clone(),
            );
        }
    }
    if env.approach == "b" {
        let shims_alive: Vec<i64> = st["terminals"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t["shim_pid"].as_i64())
            .filter(|p| sys::alive(*p as i32) && !is_zombie(*p as i32))
            .collect();
        c.add(
            "shims exit with their shells",
            shims_alive.is_empty(),
            json!(shims_alive),
        );
    }
}

fn failure(env: &mut Env, c: &mut Checks, m: &mut Value) {
    let mut terms = Vec::new();
    for _ in 0..3 {
        let (id, pid) = env.bash();
        env.write(id, "sleep 300 &\n");
        terms.push((id, pid));
    }
    let server_pid = env.status()["pid"].as_i64();
    let mut steps = Vec::new();
    let survive = |env: &mut Env, tag: &str| -> usize {
        let mut n = 0;
        for i in 0..3 {
            let id = (i + 1) as u64;
            if env.roundtrip(id, tag).is_ok() {
                n += 1;
            }
        }
        n
    };

    let rec = env
        .handover(&["--build-id", "v2", "--supported-formats", "2"], json!({}));
    let ok = survive(env, "FONE");
    steps.push(json!({"step": "new build rejects the state", "record": rec, "survivors": ok}));
    c.outcome("reject: rolled back", &rec, "rolled_back");
    c.add("reject: all terminals usable", ok == 3, ok);

    if env.approach == "a" {
        let rec = env.handover(
            &["--build-id", "v2", "--supported-formats", "2"],
            json!({"precheck": true}),
        );
        let ok = survive(env, "FTWO");
        steps.push(json!({"step": "check-handover dry run rejects", "record": rec, "survivors": ok}));
        c.outcome("precheck: nothing changed", &rec, "precheck_rejected");
        c.add("precheck: all terminals usable", ok == 3, ok);
    }

    let missing = env.run_dir.join("missing-binary").display().to_string();
    let saved = std::mem::replace(&mut env.exe, PathBuf::from(&missing));
    let rec = env.handover(&["--build-id", "v2"], json!({}));
    env.exe = saved;
    let ok = survive(env, "FTHREE");
    steps.push(
        json!({"step": "new binary missing", "record": rec, "survivors": ok}),
    );
    c.add("missing binary: terminals usable", ok == 3, json!({"outcome": rec["outcome"], "reason": rec["reason"], "survivors": ok}));

    if env.approach == "b" {
        let st = env.status();
        let shim = st["terminals"][2]["shim_pid"].as_i64().unwrap_or(0);
        let _ = sys::kill(shim as i32, libc::SIGKILL);
        std::thread::sleep(Duration::from_millis(300));
        let shell_alive = sys::alive(terms[2].1 as i32);
        let usable = env.roundtrip(3, "FSHIM").is_ok();
        let t = env
            .wait_term(3, T5, |t| !t["exit"].is_null())
            .unwrap_or(Value::Null);
        steps.push(json!({"step": "shim killed", "shell_alive": shell_alive, "usable": usable, "exit": t["exit"]}));
        c.add(
            "shim killed: shell keeps running",
            shell_alive && usable,
            json!({"alive": shell_alive, "usable": usable}),
        );
        c.add(
            "shim killed: exit reported as unknown",
            t["exit"] == "unknown",
            t["exit"].clone(),
        );
    }

    let rec = env.handover(&["--build-id", "v2"], json!({}));
    let ok = survive(env, "FFOUR");
    steps.push(json!({"step": "a later handover succeeds", "record": rec, "survivors": ok}));
    c.outcome("later handover", &rec, "ok");

    let rec =
        env.handover(&["--build-id", "v3", "--crash-on-import"], json!({}));
    std::thread::sleep(Duration::from_millis(500));
    let alive = terms
        .iter()
        .filter(|(_, p)| sys::alive(*p as i32) && !is_zombie(*p as i32))
        .count();
    let ok = if env.approach == "b" {
        survive(env, "FFIVE")
    } else {
        0
    };
    steps.push(json!({"step": "new build crashes on import", "record": rec, "shells_alive": alive, "usable": ok}));
    if env.approach == "a" {
        c.add(
            "crash on import (A): not covered, expect losses",
            true,
            json!({"shells_alive": alive, "outcome": rec["outcome"]}),
        );
    } else {
        c.outcome("crash on import (B): rolled back", &rec, "rolled_back");
        c.add("crash on import (B): terminals usable", ok >= 2, ok);
    }
    m["steps"] = json!(steps);
    m["server_pid_start"] = json!(server_pid);
}

/// The server, and in B its shims, run from a FUSE mount that goes away
/// (`HS_BREAK_CMD`) before a handover to `HS_TARGET_EXE` on another mount.
fn appimage(env: &mut Env, c: &mut Checks, m: &mut Value) {
    let target = std::env::var("HS_TARGET_EXE").unwrap_or_default();
    let brk = std::env::var("HS_BREAK_CMD").unwrap_or_default();
    let (a, _) = env.bash();
    let (b, bpid) = env.bash();
    let out = Command::new("sh").args(["-c", &brk]).output();
    m["break"] = json!(
        out.map(|o| String::from_utf8_lossy(&o.stderr).to_string())
            .unwrap_or_default()
    );
    std::thread::sleep(Duration::from_millis(300));
    c.add(
        "server survives its mount going away",
        env.roundtrip(a, "MNT").is_ok(),
        "",
    );
    env.exe = PathBuf::from(&target);
    if env.approach == "a" {
        // The new build rejects; the rollback copy taken at startup lives
        // outside the dead mount.
        let rec = env.handover(
            &["--build-id", "v2", "--supported-formats", "2"],
            json!({}),
        );
        c.outcome("rollback after the old mount vanished", &rec, "rolled_back");
    }
    let rec = env.handover(&["--build-id", "v2"], json!({}));
    c.outcome("handover to the new mount", &rec, "ok");
    c.add(
        "terminal usable after handover",
        env.roundtrip(a, "NEWMNT").is_ok(),
        "",
    );
    env.write(a, "exit 42\n");
    let t = env
        .wait_term(a, T5, |t| !t["exit"].is_null())
        .unwrap_or(Value::Null);
    c.add(
        "exit status after the mount vanished",
        t["exit"]["code"] == 42,
        t["exit"].clone(),
    );
    env.req(json!({"op": "close", "id": b}));
    let t = env
        .wait_term(b, T5, |t| !t["exit"].is_null())
        .unwrap_or(Value::Null);
    c.add(
        "close still hangs up",
        !t["exit"].is_null() && !sys::alive(bpid as i32)
            || is_zombie(bpid as i32),
        t["exit"].clone(),
    );
    let (n, _) = env.bash();
    env.write(n, "exit 3\n");
    let t = env
        .wait_term(n, T5, |t| !t["exit"].is_null())
        .unwrap_or(Value::Null);
    c.add(
        "new terminal on the new mount",
        t["exit"]["code"] == 3,
        t["exit"].clone(),
    );
}

// ---------------------------------------------------------------- measurements

fn stats(mut v: Vec<f64>) -> Value {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() {
        return Value::Null;
    }
    let pct = |p: f64| {
        v[((p * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1]
    };
    json!({"n": v.len(), "min": v[0], "median": pct(0.5), "p95": pct(0.95), "max": v[v.len() - 1]})
}

fn bench(approach: &str, n: usize, repeat: usize, precheck: bool) -> Value {
    let mut env = Env::start(approach, &[]);
    for _ in 0..n {
        env.spawn(&["/bin/cat"]);
    }
    let (mut cut, mut black, mut fails) = (Vec::new(), Vec::new(), Vec::new());
    let mut phases: std::collections::BTreeMap<&str, Vec<f64>> =
        Default::default();
    for i in 0..repeat {
        let rec = env.handover(
            &["--build-id", &format!("v{}", i + 2)],
            json!({"precheck": precheck}),
        );
        if rec["outcome"] == "ok" {
            cut.push(rec["cutover_ms"].as_f64().unwrap_or(0.0));
            black.push(rec["blackout_ms"].as_f64().unwrap_or(0.0));
            let ns = |k: &str| {
                rec[k]
                    .as_f64()
                    .or_else(|| rec["timing"][k].as_f64())
                    .unwrap_or(0.0)
            };
            let mut add = |name, a: &str, b: &str| {
                phases.entry(name).or_default().push((ns(b) - ns(a)) / 1e6)
            };
            add("pre_pause", "begin_ns", "paused_ns");
            if approach == "a" {
                add("write_state", "paused_ns", "handoff_ns");
                add("execve_to_main", "handoff_ns", "import_start_ns");
                add("parse_state", "import_start_ns", "restored_ns");
            } else {
                add("transfer", "paused_ns", "restored_ns");
                add("commit", "restored_ns", "committed_ns");
                add("commit_to_resume", "committed_ns", "resumed_ns");
            }
            add("start_workers", "restored_ns", "resumed_ns");
            add("serial_nudge", "resumed_ns", "nudged_ns");
        } else {
            fails.push(rec);
        }
    }
    if std::env::var_os("HS_DUMP_HISTORY").is_some() {
        eprintln!("{}", env.status()["history"]);
    }
    let alive = env.status()["terminals"]
        .as_array()
        .map_or(0, |t| t.iter().filter(|t| t["alive"] == true).count());
    let cleanup = env.cleanup();
    json!({
        "bench": "cutover", "approach": approach, "platform": platform(), "terminals": n, "precheck": precheck,
        "cutover_ms": stats(cut), "blackout_ms": stats(black), "failures": fails,
        "phases_ms": phases.into_iter().map(|(k, v)| (k.to_string(), stats(v))).collect::<serde_json::Map<_, _>>(), "alive_after": alive, "cleanup": cleanup,
    })
}

fn rss(count: usize, huterm: Option<String>, shim: Option<String>) -> Value {
    let exe = std::env::current_exe().unwrap();
    let dedicated = shim.unwrap_or_else(|| {
        exe.with_file_name("handover-shim").display().to_string()
    });
    let mut out =
        json!({"bench": "rss", "platform": platform(), "count": count});
    for (label, shim_exe) in [
        ("dedicated_shim", dedicated.clone()),
        ("main_binary_shim", exe.display().to_string()),
    ] {
        let mut env = Env::start("b", &["--shim-exe", &shim_exe]);
        for _ in 0..count {
            env.spawn(&["/bin/sleep", "1000"]);
        }
        std::thread::sleep(Duration::from_millis(500));
        let st = env.status();
        let (mut r, mut f, mut p) = (Vec::new(), Vec::new(), Vec::new());
        for t in st["terminals"].as_array().into_iter().flatten() {
            let pid = t["shim_pid"].as_i64().unwrap_or(0) as i32;
            r.extend(sys::rss_kib(pid).map(|v| v as f64));
            f.extend(sys::footprint_kib(pid).map(|v| v as f64));
            p.extend(sys::private_kib(pid).map(|v| v as f64));
        }
        let size = std::fs::metadata(&shim_exe).map(|m| m.len()).unwrap_or(0);
        out[label] = json!({"exe": shim_exe, "exe_bytes": size, "rss_kib": stats(r), "footprint_kib": stats(f), "private_kib": stats(p)});
        env.cleanup();
    }
    if let Some(h) = huterm {
        let mut kids: Vec<Child> = (0..count)
            .filter_map(|_| {
                Command::new(&h)
                    .arg("--pty-shim-idle")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .ok()
            })
            .collect();
        std::thread::sleep(Duration::from_millis(1000));
        let (mut r, mut f, mut p) = (Vec::new(), Vec::new(), Vec::new());
        for k in &kids {
            let pid = k.id() as i32;
            r.extend(sys::rss_kib(pid).map(|v| v as f64));
            f.extend(sys::footprint_kib(pid).map(|v| v as f64));
            p.extend(sys::private_kib(pid).map(|v| v as f64));
        }
        for k in &mut kids {
            let _ = k.kill();
            let _ = k.wait();
        }
        let size = std::fs::metadata(&h).map(|m| m.len()).unwrap_or(0);
        out["huterm_main_binary_idle"] = json!({"exe": h, "exe_bytes": size, "rss_kib": stats(r), "footprint_kib": stats(f), "private_kib": stats(p)});
    }
    // Reference: an idle interactive bash.
    let mut env = Env::start("a", &[]);
    let (_, pid) = env.bash();
    out["reference_bash"] = json!({"rss_kib": sys::rss_kib(pid as i32), "footprint_kib": sys::footprint_kib(pid as i32), "private_kib": sys::private_kib(pid as i32)});
    env.cleanup();
    out
}

use std::os::unix::process::CommandExt;
