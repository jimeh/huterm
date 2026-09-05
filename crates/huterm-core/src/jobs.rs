use std::collections::BTreeSet;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub(crate) struct JobContext {
    pub shell: Option<u32>,
    pub foreground: Option<i32>,
    pub exited: bool,
    pub pty_eof: bool,
    pub tty: Option<String>,
}

/// Observable process evidence for a terminal close assessment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobState {
    /// The child has exited or only its idle shell remains.
    Idle,
    /// Non-shell processes, including descendants in background groups.
    Running(Vec<JobProcess>),
    /// The operating system could not provide complete evidence.
    Unknown,
}

/// Identity and job-control role of a process at assessment time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobProcess {
    /// Operating system process ID.
    pub pid: u32,
    /// Process group used for assessed terminal cleanup.
    pub group: i32,
    /// Whether this process belongs to the PTY foreground process group.
    pub foreground: bool,
    /// OS command name and creation time, used to detect changed evidence.
    pub identity: String,
}

struct Process {
    pid: u32,
    parent: u32,
    group: i32,
    zombie: bool,
    tty: String,
    command: String,
    identity: String,
}

// This runs on the assessment worker, never the terminal parser or UI thread.
// Bound both command runtime and output. ps supports these columns on macOS/Linux.
fn process_table() -> Option<Vec<Process>> {
    let mut child = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,pgid=,stat=,tty=,lstart=,comm="])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let Ok(reader) = std::thread::Builder::new()
        .name("huterm-job-scan".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            (bytes.len() <= 4 * 1024 * 1024).then_some(bytes)
        })
    else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let deadline = Instant::now() + Duration::from_secs(1);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let bytes = reader.join().ok()??;
    if !success {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    text.lines().map(parse_process).collect()
}

fn parse_process(line: &str) -> Option<Process> {
    let mut fields = line.split_whitespace();
    let pid = fields.next()?.parse().ok()?;
    let parent = fields.next()?.parse().ok()?;
    let group = fields.next()?.parse().ok()?;
    let zombie = fields.next()?.contains('Z');
    let tty = fields.next()?.to_owned();
    let start = fields.by_ref().take(5).collect::<Vec<_>>().join(" ");
    let command = fields.collect::<Vec<_>>().join(" ");
    if start.is_empty() || command.is_empty() {
        return None;
    }
    Some(Process {
        pid,
        parent,
        group,
        zombie,
        tty,
        identity: format!("{start} {command}"),
        command,
    })
}

fn is_shell(command: &str) -> bool {
    matches!(
        command
            .rsplit('/')
            .next()
            .unwrap_or(command)
            .trim_start_matches('-'),
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "tcsh" | "csh"
    )
}

pub(crate) fn inspect_all(contexts: Vec<Option<JobContext>>) -> Vec<JobState> {
    if contexts.is_empty() {
        return Vec::new();
    }
    let table = process_table();
    contexts
        .into_iter()
        .map(|context| inspect(context, table.as_deref()))
        .collect()
}

fn inspect(context: Option<JobContext>, table: Option<&[Process]>) -> JobState {
    let Some(context) = context else {
        return JobState::Unknown;
    };
    if context.exited && context.pty_eof {
        return JobState::Idle;
    }
    let Some(shell) = context.shell else {
        return JobState::Unknown;
    };
    if !context.exited && context.foreground.is_none() {
        return JobState::Unknown;
    }
    let foreground = context.foreground.unwrap_or(0);
    let Some(table) = table else {
        return JobState::Unknown;
    };
    let state = classify(
        table,
        shell,
        foreground,
        context.tty.as_deref(),
        context.exited,
    );
    if context.exited && state == JobState::Idle {
        JobState::Unknown
    } else {
        state
    }
}

fn classify(
    table: &[Process],
    shell: u32,
    foreground: i32,
    tty: Option<&str>,
    exited: bool,
) -> JobState {
    if (exited && tty.is_none())
        || (!exited && !table.iter().any(|p| p.pid == shell))
    {
        return JobState::Unknown;
    }
    let mut descendants = if exited {
        BTreeSet::new()
    } else {
        BTreeSet::from([shell])
    };
    loop {
        let before = descendants.len();
        for process in table {
            if descendants.contains(&process.parent) {
                descendants.insert(process.pid);
            }
        }
        if descendants.len() == before {
            break;
        }
    }
    let mut jobs: Vec<_> = table
        .iter()
        .filter(|p| {
            !p.zombie
                && (descendants.contains(&p.pid)
                    || (!exited && p.group == foreground)
                    || tty.is_some_and(|tty| {
                        tty.trim_start_matches("tty")
                            == p.tty.trim_start_matches("tty")
                    }))
        })
        .filter(|p| exited || p.pid != shell || !is_shell(&p.command))
        .map(|p| JobProcess {
            pid: p.pid,
            group: p.group,
            foreground: !exited && p.group == foreground,
            identity: p.identity.clone(),
        })
        .collect();
    jobs.sort_by_key(|p| p.pid);
    if jobs.is_empty() {
        JobState::Idle
    } else {
        JobState::Running(jobs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn process(
        pid: u32,
        parent: u32,
        group: i32,
        tty: &str,
        command: &str,
    ) -> Process {
        Process {
            pid,
            parent,
            group,
            tty: tty.into(),
            zombie: false,
            command: command.into(),
            identity: format!("start {command}"),
        }
    }
    #[test]
    fn exited_shell_matches_macos_tty_abbreviation_without_following_reused_pid()
     {
        let table = vec![
            process(10, 1, 10, "s999", "sh"),
            process(11, 10, 10, "s999", "sleep"),
            process(12, 1, 12, "s000", "sleep"),
        ];
        let JobState::Running(jobs) =
            classify(&table, 10, 10, Some("ttys000"), true)
        else {
            panic!("missing orphaned tty job");
        };
        assert_eq!(jobs.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![12]);
    }
    #[test]
    fn idle_foreground_background_and_unknown_evidence_are_distinct() {
        let shell = process(10, 1, 10, "pts/0", "/bin/sh");
        assert_eq!(
            classify(&[shell], 10, 10, Some("pts/0"), false),
            JobState::Idle
        );
        let table = vec![
            process(10, 1, 10, "pts/0", "sh"),
            process(11, 10, 11, "pts/0", "vim"),
            process(12, 10, 12, "pts/0", "sleep"),
        ];
        let JobState::Running(jobs) =
            classify(&table, 10, 11, Some("pts/0"), false)
        else {
            panic!("missing jobs");
        };
        assert!(jobs[0].foreground);
        assert!(!jobs[1].foreground);
        assert_eq!(inspect(None, Some(&table)), JobState::Unknown);
        assert_eq!(
            inspect(
                Some(JobContext {
                    shell: Some(10),
                    foreground: Some(10),
                    exited: false,
                    pty_eof: false,
                    tty: None
                }),
                None
            ),
            JobState::Unknown
        );
        assert_eq!(classify(&[], 10, 0, Some("pts/0"), true), JobState::Idle);
    }
    #[test]
    fn root_program_and_exec_replacement_are_jobs() {
        let table = vec![process(10, 1, 10, "pts/0", "vim")];
        assert!(matches!(
            classify(&table, 10, 10, Some("pts/0"), false),
            JobState::Running(_)
        ));
    }
}
