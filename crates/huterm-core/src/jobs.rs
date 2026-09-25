mod lifecycle;

pub(crate) use lifecycle::JobLifecycle;

use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub(crate) struct JobContext {
    pub lifecycle: std::sync::Arc<JobLifecycle>,
    pub shell: Option<u32>,
    pub foreground: Option<i32>,
    #[cfg(test)]
    pub exited: bool,
    #[cfg(test)]
    pub pty_eof: bool,
    /// Device number of the terminal's PTY.
    pub tty: Option<u64>,
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
    /// Whether the terminal being assessed is this process's terminal.
    on_tty: bool,
    command: String,
    started: String,
    identity: String,
}

/// Converts one table into evidence for one terminal.
fn evidence(
    table: &huterm_procinfo::ProcessTable,
    tty: Option<u64>,
) -> Vec<Process> {
    table
        .processes
        .iter()
        .filter_map(|process| {
            let started = process.started.map_or_else(
                || "unknown".to_owned(),
                |started| started.to_string(),
            );
            Some(Process {
                pid: process.pid,
                parent: process.parent,
                group: i32::try_from(process.group).ok()?,
                zombie: process.zombie,
                on_tty: tty.is_some_and(|tty| table.on_tty(tty, process.pid)),
                identity: format!("{started} {}", process.name),
                command: process.name.clone(),
                started,
            })
        })
        .collect()
}

fn is_shell(command: &str) -> bool {
    huterm_procinfo::is_shell(
        command
            .rsplit('/')
            .next()
            .unwrap_or(command)
            .trim_start_matches('-'),
    )
}

pub(crate) fn inspect_all(contexts: Vec<Option<JobContext>>) -> Vec<JobState> {
    if contexts.is_empty() {
        return Vec::new();
    }
    // This runs on the assessment worker, never the terminal parser or UI
    // thread. On Linux it scans /proc once for every terminal.
    let ttys: Vec<u64> = contexts
        .iter()
        .flatten()
        .filter_map(|context| context.tty)
        .collect();
    let table = contexts
        .iter()
        .flatten()
        .any(|context| context.lifecycle.running())
        .then(|| huterm_procinfo::process_table(&ttys))
        .flatten();
    contexts
        .into_iter()
        .map(|context| {
            let evidence = table
                .as_ref()
                .zip(context.as_ref())
                .map(|(table, context)| evidence(table, context.tty));
            inspect(context, evidence.as_deref())
        })
        .collect()
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
        classify(table, shell, foreground)
    })
}

fn classify(table: &[Process], shell: u32, foreground: i32) -> JobState {
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
                    || p.on_tty)
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
        on_tty: bool,
        command: &str,
    ) -> Process {
        Process {
            pid,
            parent,
            group,
            on_tty,
            zombie: false,
            command: command.into(),
            started: "start".into(),
            identity: format!("start {command}"),
        }
    }
    #[test]
    fn orphans_on_the_terminal_are_jobs_but_other_terminals_are_not() {
        // Both left the shell's tree and group; only one holds its terminal.
        let table = vec![
            process(10, 1, 10, true, "sh"),
            process(12, 1, 12, true, "sleep"),
            process(13, 1, 13, false, "sleep"),
        ];
        let JobState::Running(jobs) = classify(&table, 10, 10) else {
            panic!("missing tty job");
        };
        assert_eq!(jobs.iter().map(|p| p.pid).collect::<Vec<_>>(), vec![12]);
    }
    #[test]
    fn idle_foreground_background_and_unknown_evidence_are_distinct() {
        let shell = process(10, 1, 10, true, "/bin/sh");
        assert_eq!(classify(&[shell], 10, 10), JobState::Idle);
        let table = vec![
            process(10, 1, 10, true, "sh"),
            process(11, 10, 11, true, "vim"),
            process(12, 10, 12, true, "sleep"),
        ];
        let JobState::Running(jobs) = classify(&table, 10, 11) else {
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
        assert_eq!(classify(&[], 10, 0), JobState::Unknown);
    }

    #[test]
    fn consent_rejects_new_groups_reused_leaders_and_unknown_widening() {
        let original = vec![
            process(10, 1, 10, true, "sh"),
            process(20, 10, 20, true, "make"),
            process(21, 20, 20, true, "cc"),
        ];
        let consent = classify(&original, 10, 20);
        let churn = vec![
            process(10, 1, 10, true, "sh"),
            process(20, 10, 20, true, "cargo"),
            process(22, 20, 20, true, "rustc"),
        ];
        let current = classify(&churn, 10, 20);
        assert!(
            covered_by(&current, &consent),
            "leader exec and child churn must preserve group consent"
        );
        let mut new_group = churn;
        new_group.push(process(30, 10, 30, true, "vim"));
        assert!(!covered_by(&classify(&new_group, 10, 20), &consent));
        new_group.pop();
        new_group[1].started = "later incarnation".into();
        assert!(!covered_by(&classify(&new_group, 10, 20), &consent));
        assert!(!covered_by(&JobState::Unknown, &consent));
        assert!(!covered_by(&current, &JobState::Idle));
        assert!(covered_by(&JobState::Idle, &consent));
    }

    #[test]
    fn leaderless_group_needs_an_original_surviving_member() {
        let original = vec![
            process(10, 1, 10, true, "sh"),
            process(20, 10, 20, true, "make"),
            process(21, 20, 20, true, "cc"),
        ];
        let consent = classify(&original, 10, 20);
        let mut current = vec![
            process(10, 1, 10, true, "sh"),
            process(21, 1, 20, true, "cc"),
            process(22, 1, 20, true, "ld"),
        ];
        assert!(covered_by(&classify(&current, 10, 20), &consent));
        current.remove(1);
        assert!(
            !covered_by(&classify(&current, 10, 20), &consent),
            "group number alone cannot prove continuity"
        );
    }

    #[test]
    fn root_program_and_exec_replacement_are_jobs() {
        let table = vec![process(10, 1, 10, true, "vim")];
        assert!(matches!(classify(&table, 10, 10), JobState::Running(_)));
    }
}
