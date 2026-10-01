use super::*;

fn at(start: Instant, milliseconds: u64) -> Instant {
    start + Duration::from_millis(milliseconds)
}

#[test]
fn nothing_is_armed_until_an_event_and_idle_shells_rearm_nothing() {
    // A probe at spawn could see the child before it execs its program.
    let start = Instant::now();
    let mut schedule = ProbeSchedule::default();
    assert_eq!(schedule.deadline(), None);
    schedule.output(start);
    schedule.probed(at(start, 50), false);
    assert_eq!(schedule.deadline(), None, "an idle shell arms nothing");
}

#[test]
fn command_input_probes_after_settling_and_again_later() {
    let start = Instant::now();
    let mut schedule = ProbeSchedule::default();
    schedule.input(b"ls", start);
    assert_eq!(schedule.deadline(), None, "typing alone arms nothing");
    schedule.input(b"\r", start);
    assert_eq!(schedule.deadline(), Some(at(start, 50)));
    schedule.probed(at(start, 50), false);
    assert_eq!(schedule.deadline(), Some(at(start, 500)));
    schedule.probed(at(start, 500), false);
    assert_eq!(schedule.deadline(), None);
    for control in [0x03, 0x04, 0x1a, 0x1c, b'\n'] {
        schedule.input(&[control], start);
        assert!(schedule.deadline().is_some(), "{control:#x}");
        schedule.stop();
    }
}

#[test]
fn output_probes_only_after_silence() {
    let start = Instant::now();
    let mut schedule = ProbeSchedule::default();
    schedule.output(start);
    assert_eq!(schedule.deadline(), Some(at(start, 50)));
    schedule.probed(at(start, 50), false);
    for milliseconds in (60..1_000).step_by(10) {
        schedule.output(at(start, milliseconds));
    }
    assert_eq!(schedule.deadline(), None, "a flood arms one probe");
    schedule.output(at(start, 1_300));
    assert_eq!(schedule.deadline(), Some(at(start, 1_350)));
}

#[test]
fn titles_probe_and_jobs_keep_a_slow_poll() {
    let start = Instant::now();
    let mut schedule = ProbeSchedule::default();
    schedule.title(start);
    assert_eq!(schedule.deadline(), Some(at(start, 50)));
    schedule.probed(at(start, 50), true);
    assert_eq!(schedule.deadline(), Some(at(start, 1_050)));
    schedule.probed(at(start, 1_050), true);
    assert_eq!(schedule.deadline(), Some(at(start, 2_050)));
    schedule.probed(at(start, 2_050), false);
    assert_eq!(
        schedule.deadline(),
        None,
        "returning to the shell stops polling"
    );
}

#[test]
fn probes_are_rate_limited_and_deadlines_bounded() {
    let start = Instant::now();
    let mut schedule = ProbeSchedule::default();
    schedule.probed(start, false);
    schedule.title(start);
    assert_eq!(schedule.deadline(), Some(at(start, 100)));
    assert!(!schedule.due(at(start, 99)));
    assert!(schedule.due(at(start, 100)));
    for milliseconds in 0..20 {
        schedule.title(at(start, milliseconds));
    }
    assert!(schedule.triggers.len() <= MAX_TRIGGERS);
    assert_eq!(schedule.triggers.first(), Some(&at(start, 50)));
}

fn directory(path: &str, local: bool) -> TerminalDirectory {
    TerminalDirectory::new(None, path.into(), local)
}

#[test]
fn reports_apply_while_their_reporter_holds_the_foreground() {
    let (shell, job) = (Some(10), Some(20));
    let mut directories = Reports::default();
    directories.probed(shell, Some(directory("/home/me", true)));
    assert_eq!(directories.directory(), Some(directory("/home/me", true)));
    directories.report_directory(Some(directory("/home/me/src", true)), shell);
    assert_eq!(
        directories.directory(),
        Some(directory("/home/me/src", true)),
        "the shell's report wins at its prompt"
    );
    directories.probed(job, Some(directory("/tmp", true)));
    assert_eq!(
        directories.directory(),
        Some(directory("/tmp", true)),
        "a running job shows its own directory"
    );
    directories.probed(shell, Some(directory("/home/me", true)));
    assert_eq!(
        directories.directory(),
        Some(directory("/home/me/src", true))
    );
}

#[test]
fn a_jobs_report_expires_with_the_job_and_clearing_falls_back() {
    let (shell, ssh) = (Some(10), Some(30));
    let mut directories = Reports::default();
    directories.probed(ssh, Some(directory("/home/me", true)));
    directories.report_directory(Some(directory("/srv/remote", false)), ssh);
    assert_eq!(
        directories.directory(),
        Some(directory("/srv/remote", false))
    );
    directories.probed(shell, Some(directory("/home/me", true)));
    assert_eq!(
        directories.directory(),
        Some(directory("/home/me", true)),
        "a remote report stops applying after ssh exits"
    );
    directories.report_directory(Some(directory("/home/me/src", true)), shell);
    directories.report_directory(None, shell);
    assert_eq!(directories.directory(), Some(directory("/home/me", true)));
    assert_eq!(Reports::default().directory(), None);
}

#[test]
fn titles_apply_only_while_their_setter_holds_the_foreground() {
    let start = Instant::now();
    let (shell, job) = (Some(10), Some(20));
    let mut reports = Reports::default();
    reports.probed(shell, None);
    assert!(reports.title_needs_group("~/src", start));
    reports.report_title("~/src".into(), shell, start);
    assert_eq!(reports.title().as_deref(), Some("~/src"));
    reports.probed(job, None);
    assert_eq!(reports.title(), None, "a job has not titled itself yet");
    reports.report_title("notes.txt - VIM".into(), job, start);
    assert_eq!(reports.title().as_deref(), Some("notes.txt - VIM"));
    reports.probed(shell, None);
    assert_eq!(reports.title(), None, "the job's title leaves with it");
    reports.report_title(String::new(), shell, start);
    assert_eq!(reports.title(), None);
    assert!(!reports.title_needs_group("", start));
}

#[test]
fn repeated_titles_are_reattributed_when_stale_or_inapplicable() {
    let start = Instant::now();
    let (first, second) = (Some(20), Some(30));
    let mut reports = Reports::default();
    reports.report_title("vim".into(), first, start);
    assert!(!reports.title_text_changes("vim"));
    assert!(
        !reports.title_needs_group("vim", at(start, 100)),
        "a program re-sending its title reads the group at most every 250 ms"
    );
    assert!(reports.title_needs_group("vim", at(start, 250)));
    // A second vim starts after the first exits and repeats its title.
    reports.probed(second, None);
    assert!(
        reports.title_needs_group("vim", at(start, 120)),
        "the recorded attribution no longer applies"
    );
    reports.report_title("vim".into(), second, at(start, 120));
    assert_eq!(reports.title().as_deref(), Some("vim"));
}

#[test]
fn repeated_titles_after_job_control_input_read_the_group() {
    let start = Instant::now();
    let (shell, job) = (Some(20), Some(30));
    let mut reports = Reports::default();
    reports.probed(shell, None);
    reports.report_title("vim".into(), shell, start);
    // Enter starts a job that repeats the title before any probe.
    reports.job_control_input();
    assert!(reports.title_needs_group("vim", at(start, 10)));
    reports.report_title("vim".into(), job, at(start, 10));
    reports.probed(job, None);
    assert_eq!(reports.title().as_deref(), Some("vim"));
    assert!(
        !reports.title_needs_group("vim", at(start, 60)),
        "the probe settles the attribution"
    );
}

#[test]
fn a_running_job_keeps_one_poll_despite_other_triggers() {
    let start = Instant::now();
    let mut schedule = ProbeSchedule::default();
    schedule.input(b"\r", start);
    schedule.probed(at(start, 50), true);
    assert_eq!(schedule.deadline(), Some(at(start, 500)));
    schedule.probed(at(start, 500), true);
    assert_eq!(schedule.deadline(), Some(at(start, 1_500)));
    schedule.input(b"\r", at(start, 700));
    schedule.probed(at(start, 750), true);
    schedule.probed(at(start, 1_200), true);
    assert_eq!(
        (schedule.triggers.len(), schedule.deadline()),
        (0, Some(at(start, 2_200))),
        "each probe replaces the poll instead of adding another"
    );
}

/// Kills a fixture's process group when the test ends, including on a
/// failed assertion. A leaked child keeps the runner's output pipes open.
#[cfg(unix)]
struct GroupGuard(std::process::Child);

#[cfg(unix)]
impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Ok(group) = i32::try_from(self.0.id()) {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(group),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        let _ = self.0.wait();
    }
}

#[cfg(unix)]
#[test]
fn an_idle_root_shell_that_execs_a_script_is_renamed() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let directory = std::env::temp_dir()
        .join(format!("huterm-exec-rename-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let script = directory.join("second-script");
    std::fs::write(&script, "echo second\nwhile :; do sleep 1; done\n")
        .unwrap();
    let mut child = GroupGuard(
        Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "echo first; read line; exec /bin/sh '{}'",
                script.display()
            ))
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut output = BufReader::new(child.0.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let root = child.0.id();
    let group = i32::try_from(root).ok();
    let idle = probe(group, Some(root));
    assert_eq!((idle.name, idle.job), (None, false));
    // Same PID, start time, and (on macOS) kernel name after the exec.
    std::io::Write::write_all(child.0.stdin.as_mut().unwrap(), b"go\n")
        .unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    assert_eq!(line, "second\n");
    let job = probe(group, Some(root));
    assert_eq!(
        (job.name.as_deref(), job.job),
        (Some("second-script"), true)
    );
    drop(child);
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(unix)]
#[test]
fn names_a_foreground_job_and_treats_the_root_shell_as_idle() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let mut child = GroupGuard(
        Command::new("/bin/sh")
            .args(["-c", "echo ready; exec sleep 30"])
            .process_group(0)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    BufReader::new(child.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let pid = child.0.id();
    let group = i32::try_from(pid).ok();
    let deadline = Instant::now() + Duration::from_secs(5);
    let job = loop {
        let probe = probe(group, None);
        if probe.name.as_deref() == Some("sleep") || Instant::now() > deadline {
            break probe;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(job.name.as_deref(), Some("sleep"));
    assert!(job.job);
    assert_eq!(job.group, group);
    let expected = std::env::current_dir().unwrap().canonicalize().unwrap();
    let directory = job.directory.unwrap();
    assert!(directory.is_local());
    assert_eq!(
        std::path::Path::new(directory.path())
            .canonicalize()
            .unwrap(),
        expected
    );
    // The same process as root reads as a non-shell program, not idle.
    assert!(probe(group, Some(pid)).job);
    let unknown = probe(None, Some(pid));
    assert_eq!((unknown.name, unknown.job), (None, false));
    // An exited job's empty group can hold the terminal until the shell
    // reclaims it; keep polling until then.
    let empty = probe(Some(i32::MAX), Some(pid));
    assert_eq!((empty.name, empty.job), (None, true));
    assert!(unknown.directory.is_some(), "falls back to the root");
}
