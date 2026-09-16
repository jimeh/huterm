mod lifecycle;

pub(crate) use lifecycle::JobLifecycle;

use std::collections::BTreeSet;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crate::RuntimeClient;

const PROCESS_NAME_BYTE_LIMIT: usize = 256;

#[derive(Clone, Debug)]
pub(crate) struct JobContext {
    pub lifecycle: std::sync::Arc<JobLifecycle>,
    pub shell: Option<u32>,
    pub foreground: Option<i32>,
    #[cfg(test)]
    pub exited: bool,
    #[cfg(test)]
    pub pty_eof: bool,
    pub tty: Option<String>,
}

/// Observable process evidence for a terminal close assessment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobState {
    /// Only an idle shell remains, or the terminal root has exited.
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
    /// Group leader creation time, when its process record is still present.
    /// Command changes caused by exec do not change this identity.
    pub group_started: Option<String>,
    /// Whether this process belongs to the PTY foreground process group.
    pub foreground: bool,
    /// OS command name and creation time, used to detect changed evidence.
    pub identity: String,
}

/// Whether current evidence stays within previously assessed job groups.
/// A live group leader's creation time tolerates exec and child churn. A
/// leaderless group needs a surviving original member to prove continuity.
pub(crate) fn covered_by(current: &JobState, consent: &JobState) -> bool {
    match (current, consent) {
        (JobState::Idle, _) | (JobState::Unknown, JobState::Unknown) => true,
        (JobState::Running(current), JobState::Running(consent)) => {
            current.iter().all(|process| {
                consent.iter().any(|previous| {
                    if process.group != previous.group {
                        return false;
                    }
                    match (&process.group_started, &previous.group_started) {
                        (Some(current), Some(consent)) => current == consent,
                        // A leader cannot newly appear within an existing group.
                        (Some(_), None) => false,
                        _ => current.iter().any(|member| {
                            member.group == previous.group
                                && member.pid == previous.pid
                                && member.identity == previous.identity
                        }),
                    }
                })
            })
        }
        _ => false,
    }
}

#[derive(Clone)]
struct Process {
    pid: u32,
    parent: u32,
    group: i32,
    zombie: bool,
    tty: String,
    command: String,
    started: String,
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
    let scanner_pid = child.id();
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
    // The owned ps observer has already been waited and is not a terminal job.
    text.lines()
        .map(parse_process)
        .collect::<Option<Vec<_>>>()
        .map(|table| {
            table
                .into_iter()
                .filter(|process| process.pid != scanner_pid)
                .collect()
        })
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
        started: start,
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
    let table = contexts
        .iter()
        .flatten()
        .any(|context| context.lifecycle.running())
        .then(process_table)
        .flatten();
    contexts
        .into_iter()
        .map(|context| inspect(context, table.as_deref()))
        .collect()
}

/// Samples foreground-process display names for a batch of terminal runtimes.
///
/// Every runtime context is requested before waiting. The batch then uses one
/// process-table read and publishes only through each runtime's ordered control
/// queue, where stale foreground-group samples are rejected.
pub fn sample_foreground_processes(clients: &[RuntimeClient]) {
    if clients.is_empty() {
        return;
    }
    let requests: Vec<Option<Receiver<JobContext>>> = clients
        .iter()
        .map(RuntimeClient::request_job_context)
        .collect();
    let deadline = Instant::now() + Duration::from_secs(1);
    let contexts: Vec<Option<JobContext>> = requests
        .into_iter()
        .map(|request| {
            let remaining = deadline.saturating_duration_since(Instant::now());
            request?.recv_timeout(remaining).ok()
        })
        .collect();
    let samples = foreground_samples(&contexts, process_table);
    for (client, (sampled_group, name)) in clients.iter().zip(samples) {
        client.update_foreground_process(sampled_group, name);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SampledForegroundGroup {
    Unavailable,
    Observed(Option<i32>),
}

fn foreground_samples(
    contexts: &[Option<JobContext>],
    read_table: impl FnOnce() -> Option<Vec<Process>>,
) -> Vec<(SampledForegroundGroup, Option<String>)> {
    let table = contexts
        .iter()
        .flatten()
        .any(|context| context.lifecycle.running())
        .then(read_table)
        .flatten();
    contexts
        .iter()
        .map(|context| {
            (
                context
                    .as_ref()
                    .map_or(SampledForegroundGroup::Unavailable, |value| {
                        SampledForegroundGroup::Observed(value.foreground)
                    }),
                foreground_process(context.as_ref(), table.as_deref()),
            )
        })
        .collect()
}

fn foreground_process(
    context: Option<&JobContext>,
    table: Option<&[Process]>,
) -> Option<String> {
    let context = context?;
    if !context.lifecycle.running() {
        return None;
    }
    let shell = context.shell?;
    let foreground = context.foreground?;
    let table = table?;
    let mut candidates: Vec<_> = table
        .iter()
        .filter(|process| !process.zombie && process.group == foreground)
        .collect();
    candidates.sort_by_key(|process| {
        (
            i32::try_from(process.pid).ok() != Some(foreground),
            process.pid,
        )
    });
    let selected = candidates.first()?;
    if candidates.len() == 1
        && selected.pid == shell
        && is_shell(&selected.command)
    {
        return None;
    }
    let name = selected
        .command
        .rsplit('/')
        .next()
        .unwrap_or(&selected.command)
        .trim_start_matches('-');
    if name.is_empty() {
        return None;
    }
    let end = name
        .char_indices()
        .map(|(index, character)| index + character.len_utf8())
        .take_while(|end| *end <= PROCESS_NAME_BYTE_LIMIT)
        .last()?;
    Some(name[..end].to_owned())
}

fn inspect(context: Option<JobContext>, table: Option<&[Process]>) -> JobState {
    let Some(context) = context else {
        return JobState::Unknown;
    };
    context.lifecycle.assess(|| {
        let Some(shell) = context.shell else {
            return JobState::Unknown;
        };
        let Some(foreground) = context.foreground else {
            return JobState::Unknown;
        };
        let Some(table) = table else {
            return JobState::Unknown;
        };
        classify(table, shell, foreground, context.tty.as_deref())
    })
}

fn classify(
    table: &[Process],
    shell: u32,
    foreground: i32,
    tty: Option<&str>,
) -> JobState {
    if !table.iter().any(|p| p.pid == shell) {
        return JobState::Unknown;
    }
    let mut descendants = BTreeSet::from([shell]);
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
                    || p.group == foreground
                    || tty.is_some_and(|tty| {
                        tty.trim_start_matches("tty")
                            == p.tty.trim_start_matches("tty")
                    }))
        })
        .filter(|p| p.pid != shell || !is_shell(&p.command))
        .map(|p| JobProcess {
            pid: p.pid,
            group: p.group,
            group_started: table
                .iter()
                .find(|leader| i32::try_from(leader.pid).ok() == Some(p.group))
                .map(|leader| leader.started.clone()),
            foreground: p.group == foreground,
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
            started: "start".into(),
            identity: format!("start {command}"),
        }
    }
    #[test]
    fn live_shell_matches_macos_tty_abbreviation() {
        let table = vec![
            process(10, 1, 10, "s000", "sh"),
            process(12, 1, 12, "s000", "sleep"),
        ];
        let JobState::Running(jobs) = classify(&table, 10, 10, Some("ttys000"))
        else {
            panic!("missing tty job");
        };
        assert_eq!(jobs.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![12]);
    }
    #[test]
    fn idle_foreground_background_and_unknown_evidence_are_distinct() {
        let shell = process(10, 1, 10, "pts/0", "/bin/sh");
        assert_eq!(classify(&[shell], 10, 10, Some("pts/0")), JobState::Idle);
        let table = vec![
            process(10, 1, 10, "pts/0", "sh"),
            process(11, 10, 11, "pts/0", "vim"),
            process(12, 10, 12, "pts/0", "sleep"),
        ];
        let JobState::Running(jobs) = classify(&table, 10, 11, Some("pts/0"))
        else {
            panic!("missing jobs");
        };
        assert!(jobs[0].foreground);
        assert!(!jobs[1].foreground);
        assert_eq!(inspect(None, Some(&table)), JobState::Unknown);
        assert_eq!(
            inspect(
                Some(JobContext {
                    lifecycle: std::sync::Arc::new(JobLifecycle::default()),
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
        assert_eq!(classify(&[], 10, 0, Some("pts/0")), JobState::Unknown);
    }

    #[test]
    fn foreground_labels_prefer_live_leader_then_lowest_pid() {
        let lifecycle = std::sync::Arc::new(JobLifecycle::default());
        let context = JobContext {
            lifecycle,
            shell: Some(10),
            foreground: Some(20),
            exited: false,
            pty_eof: false,
            tty: Some("pts/0".into()),
        };
        let table = vec![
            process(10, 1, 10, "pts/0", "-zsh"),
            process(20, 10, 20, "pts/0", "/usr/bin/vim"),
            process(21, 20, 20, "pts/0", "helper"),
        ];
        assert_eq!(
            foreground_process(Some(&context), Some(&table)).as_deref(),
            Some("vim")
        );
        let without_leader = &table[..1]
            .iter()
            .chain(&table[2..])
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            foreground_process(Some(&context), Some(without_leader)).as_deref(),
            Some("helper")
        );
    }

    #[test]
    fn foreground_labels_hide_idle_shells_zombies_and_missing_evidence() {
        let context = JobContext {
            lifecycle: std::sync::Arc::new(JobLifecycle::default()),
            shell: Some(10),
            foreground: Some(10),
            exited: false,
            pty_eof: false,
            tty: Some("pts/0".into()),
        };
        let shell = process(10, 1, 10, "pts/0", "/bin/-zsh");
        assert_eq!(foreground_process(Some(&context), Some(&[shell])), None);
        let mut zombie = process(11, 10, 10, "pts/0", "sleep");
        zombie.zombie = true;
        assert_eq!(foreground_process(Some(&context), Some(&[zombie])), None);
        assert_eq!(foreground_process(Some(&context), None), None);
    }

    #[test]
    fn foreground_batch_reads_one_table_for_multiple_live_contexts_and_none_for_retired()
     {
        let context = || JobContext {
            lifecycle: std::sync::Arc::new(JobLifecycle::default()),
            shell: Some(10),
            foreground: Some(11),
            exited: false,
            pty_eof: false,
            tty: Some("pts/0".into()),
        };
        let mut scans = 0;
        let samples =
            foreground_samples(&[Some(context()), Some(context())], || {
                scans += 1;
                Some(vec![process(11, 10, 11, "pts/0", "cargo")])
            });
        assert_eq!(scans, 1);
        assert_eq!(
            samples,
            [
                (
                    SampledForegroundGroup::Observed(Some(11)),
                    Some("cargo".into())
                ),
                (
                    SampledForegroundGroup::Observed(Some(11)),
                    Some("cargo".into())
                )
            ]
        );

        let retired = context();
        retired.lifecycle.retire();
        let samples = foreground_samples(&[Some(retired)], || {
            panic!("retired contexts must not scan")
        });
        assert_eq!(
            samples,
            [(SampledForegroundGroup::Observed(Some(11)), None)]
        );
    }
    #[test]
    fn consent_rejects_new_groups_reused_leaders_and_unknown_widening() {
        let original = vec![
            process(10, 1, 10, "pts/0", "sh"),
            process(20, 10, 20, "pts/0", "make"),
            process(21, 20, 20, "pts/0", "cc"),
        ];
        let consent = classify(&original, 10, 20, Some("pts/0"));
        let churn = vec![
            process(10, 1, 10, "pts/0", "sh"),
            process(20, 10, 20, "pts/0", "cargo"),
            process(22, 20, 20, "pts/0", "rustc"),
        ];
        let current = classify(&churn, 10, 20, Some("pts/0"));
        assert!(
            covered_by(&current, &consent),
            "leader exec and child churn must preserve group consent"
        );
        let mut new_group = churn;
        new_group.push(process(30, 10, 30, "pts/0", "vim"));
        assert!(!covered_by(
            &classify(&new_group, 10, 20, Some("pts/0")),
            &consent
        ));
        new_group.pop();
        new_group[1].started = "later incarnation".into();
        assert!(!covered_by(
            &classify(&new_group, 10, 20, Some("pts/0")),
            &consent
        ));
        assert!(!covered_by(&JobState::Unknown, &consent));
        assert!(!covered_by(&current, &JobState::Idle));
        assert!(covered_by(&JobState::Idle, &consent));
    }

    #[test]
    fn leaderless_group_needs_an_original_surviving_member() {
        let original = vec![
            process(10, 1, 10, "pts/0", "sh"),
            process(20, 10, 20, "pts/0", "make"),
            process(21, 20, 20, "pts/0", "cc"),
        ];
        let consent = classify(&original, 10, 20, Some("pts/0"));
        let mut current = vec![
            process(10, 1, 10, "pts/0", "sh"),
            process(21, 1, 20, "pts/0", "cc"),
            process(22, 1, 20, "pts/0", "ld"),
        ];
        assert!(covered_by(
            &classify(&current, 10, 20, Some("pts/0")),
            &consent
        ));
        current.remove(1);
        assert!(
            !covered_by(&classify(&current, 10, 20, Some("pts/0")), &consent),
            "group number alone cannot prove continuity"
        );
    }

    #[test]
    fn root_program_and_exec_replacement_are_jobs() {
        let table = vec![process(10, 1, 10, "pts/0", "vim")];
        assert!(matches!(
            classify(&table, 10, 10, Some("pts/0")),
            JobState::Running(_)
        ));
    }
}
