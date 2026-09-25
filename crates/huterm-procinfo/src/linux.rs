use crate::{Process, StartTime};

pub(crate) fn process(pid: u32) -> Option<Process> {
    parse_stat(pid, &read_stat(pid)?)
}

pub(crate) fn arguments(pid: u32) -> Option<Vec<String>> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let bytes = bytes.strip_suffix(b"\0").unwrap_or(&bytes);
    if bytes.is_empty() {
        return Some(Vec::new());
    }
    Some(
        bytes
            .split(|byte| *byte == 0)
            .map(|argument| String::from_utf8_lossy(argument).into_owned())
            .collect(),
    )
}

/// Linux cannot filter processes by group, so this scans every `stat` file.
pub(crate) fn group_members(group: u32) -> Option<Vec<u32>> {
    let entries = std::fs::read_dir("/proc").ok()?;
    Some(
        entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
            .filter_map(|pid| parse_stat(pid, &read_stat(pid)?))
            .filter(|process| process.group == group)
            .map(|process| process.pid)
            .collect(),
    )
}

fn read_stat(pid: u32) -> Option<String> {
    let bytes = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Parses `/proc/<pid>/stat`. The command name sits in parentheses and may
/// itself contain spaces and parentheses, so fields start after the last `)`.
fn parse_stat(pid: u32, stat: &str) -> Option<Process> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_owned();
    let fields: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
    // Fields after the name, counting from zero: state, ppid, pgrp,
    // session, tty_nr, ..., starttime at index 19 (field 22 in proc(5)).
    // tty_nr uses the same encoding as the device's st_rdev; 0 means none.
    let state = *fields.first()?;
    let parent = fields.get(1)?.parse().ok()?;
    let group = fields.get(2)?.parse().ok()?;
    let tty: u64 = fields.get(4)?.parse().ok()?;
    let started = fields.get(19)?.parse().ok()?;
    Some(Process {
        pid,
        parent,
        group,
        tty: (tty != 0).then_some(tty),
        zombie: matches!(state, "Z" | "X"),
        started: Some(StartTime {
            seconds: started,
            fraction: 0,
        }),
        name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "4242 (a) b (c)) S 1 4200 4200 34816 4200 4194560 \
        100 0 0 0 1 2 0 0 20 0 1 0 987654 1234567 100 18446744073709551615";

    #[test]
    fn stat_names_may_contain_spaces_and_parentheses() {
        let process = parse_stat(4242, STAT).unwrap();
        assert_eq!(process.name, "a) b (c)");
        assert_eq!(process.parent, 1);
        assert_eq!(process.group, 4200);
        assert_eq!(process.tty, Some(34_816));
        assert!(!process.zombie);
        assert_eq!(
            process.started,
            Some(StartTime {
                seconds: 987_654,
                fraction: 0
            })
        );
    }

    #[test]
    fn zombie_and_malformed_stat_lines() {
        let zombie = STAT.replace(") S ", ") Z ");
        assert!(parse_stat(4242, &zombie).unwrap().zombie);
        let detached = STAT.replace(" 34816 ", " 0 ");
        assert_eq!(parse_stat(4242, &detached).unwrap().tty, None);
        assert_eq!(parse_stat(4242, "4242 (truncated"), None);
        assert_eq!(parse_stat(4242, "4242 (short) S 1"), None);
    }
}
