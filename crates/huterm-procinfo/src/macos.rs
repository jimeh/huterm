//! macOS process facts through `libproc` and `sysctl`.
//!
//! The unsafe code is confined to three helpers that pass correctly sized
//! buffers to the kernel. Everything they return is parsed in safe code.
#![expect(
    unsafe_code,
    reason = "macOS exposes process facts only through C APIs"
)]

use std::ffi::{c_char, c_int, c_void};
use std::mem::{MaybeUninit, size_of};
use std::sync::OnceLock;

use crate::{Process, ProcessTable, StartTime};

/// `proc_listpids` filters from `<sys/proc_info.h>`; `libc` omits them.
const PROC_ALL_PIDS: u32 = 1;
const PROC_PGRP_ONLY: u32 = 2;
const PROC_TTY_ONLY: u32 = 3;
/// `NODEV`: the process has no controlling terminal.
const NO_DEVICE: u32 = u32::MAX;

pub(crate) fn process(pid: u32) -> Option<Process> {
    let id = c_int::try_from(pid).ok()?;
    if let Some(info) =
        pid_info::<libc::proc_bsdinfo>(id, libc::PROC_PIDTBSDINFO)
    {
        // `pbi_name` holds up to 32 bytes and is empty when no name is
        // registered; `pbi_comm` holds up to 16.
        let name = Some(c_string(&info.pbi_name))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| c_string(&info.pbi_comm));
        return (info.pbi_pid == pid).then(|| Process {
            pid,
            parent: info.pbi_ppid,
            group: info.pbi_pgid,
            tty: (info.e_tdev != NO_DEVICE).then_some(u64::from(info.e_tdev)),
            zombie: info.pbi_status == libc::SZOMB,
            started: Some(StartTime {
                seconds: info.pbi_start_tvsec,
                fraction: info.pbi_start_tvusec,
            }),
            name,
        });
    }
    // Other users' processes, such as `sudo`, only expose the short info,
    // which has no terminal or start time.
    let info =
        pid_info::<libc::proc_bsdshortinfo>(id, libc::PROC_PIDT_SHORTBSDINFO)?;
    (info.pbsi_pid == pid).then(|| Process {
        pid,
        parent: info.pbsi_ppid,
        group: info.pbsi_pgid,
        tty: None,
        zombie: info.pbsi_status == libc::SZOMB,
        started: None,
        name: c_string(&info.pbsi_comm),
    })
}

/// Reads argv through `KERN_PROCARGS2`. The kernel only permits this for the
/// caller's own processes unless it runs as root.
pub(crate) fn arguments(pid: u32) -> Option<Vec<String>> {
    let mut buffer = vec![0_u8; argument_limit()?];
    let mut mib = [
        libc::CTL_KERN,
        libc::KERN_PROCARGS2,
        c_int::try_from(pid).ok()?,
    ];
    let mut size = buffer.len();
    // SAFETY: `buffer` has `size` writable bytes. `sysctl` writes at most
    // `size` bytes and stores the number written back into `size`.
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return None;
    }
    buffer.truncate(size);
    parse_arguments(&buffer)
}

pub(crate) fn group_members(group: u32) -> Option<Vec<u32>> {
    list_pids(PROC_PGRP_ONLY, group)
}

/// Reads every process, and asks the kernel for each terminal's members
/// because other users' processes do not report their terminal.
pub(crate) fn process_table(ttys: &[u64]) -> Option<ProcessTable> {
    let processes = list_pids(PROC_ALL_PIDS, 0)?
        .into_iter()
        .filter_map(process)
        .collect();
    let ttys = ttys
        .iter()
        .map(|device| {
            let filter = u32::try_from(*device).ok()?;
            Some((
                *device,
                list_pids(PROC_TTY_ONLY, filter)?.into_iter().collect(),
            ))
        })
        .collect::<Option<_>>()?;
    Some(ProcessTable { processes, ttys })
}

/// Reads one `proc_pidinfo` flavour. Only instantiate with `libc`'s
/// `proc_info` structs, which contain integers and byte arrays only.
fn pid_info<T: Copy>(pid: c_int, flavor: c_int) -> Option<T> {
    let size = c_int::try_from(size_of::<T>()).ok()?;
    let mut info = MaybeUninit::<T>::zeroed();
    // SAFETY: `info` is a writable buffer of exactly `size` bytes, and the
    // kernel writes at most `size` bytes into it.
    let written = unsafe {
        libc::proc_pidinfo(pid, flavor, 0, info.as_mut_ptr().cast(), size)
    };
    // SAFETY: all-zero bytes are a valid value for these plain structs, and a
    // full-size write replaced every field.
    (written == size).then(|| unsafe { info.assume_init() })
}

/// Lists PIDs with a kernel filter. `proc_listpids` returns 0 both for an
/// empty list and for an error, so `errno` is cleared first to tell them
/// apart.
fn list_pids(kind: u32, filter: u32) -> Option<Vec<u32>> {
    let needed = list_pid_bytes(kind, filter, std::ptr::null_mut(), 0)?;
    // Leave room for processes that start between the two calls.
    let mut pids = vec![0_u32; needed / size_of::<u32>() + 64];
    let capacity = c_int::try_from(pids.len() * size_of::<u32>()).ok()?;
    let written =
        list_pid_bytes(kind, filter, pids.as_mut_ptr().cast(), capacity)?;
    pids.truncate(written / size_of::<u32>());
    pids.retain(|pid| *pid != 0);
    Some(pids)
}

fn list_pid_bytes(
    kind: u32,
    filter: u32,
    buffer: *mut c_void,
    capacity: c_int,
) -> Option<usize> {
    // SAFETY: `__error` returns the calling thread's `errno` location.
    unsafe { *libc::__error() = 0 };
    // SAFETY: `buffer` is null with a zero capacity, which asks only for the
    // size, or points to `capacity` writable bytes.
    let written =
        unsafe { libc::proc_listpids(kind, filter, buffer, capacity) };
    let failed = std::io::Error::last_os_error().raw_os_error() != Some(0);
    if written < 0 || (written == 0 && failed) {
        return None;
    }
    usize::try_from(written).ok()
}

fn argument_limit() -> Option<usize> {
    static LIMIT: OnceLock<Option<usize>> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
        let mut limit: c_int = 0;
        let mut size = size_of::<c_int>();
        // SAFETY: `limit` has `size` writable bytes, matching `KERN_ARGMAX`'s
        // `int` value.
        let result = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                2,
                (&raw mut limit).cast(),
                &raw mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        (result == 0 && size == size_of::<c_int>())
            .then(|| usize::try_from(limit).ok())
            .flatten()
    })
}

/// Parses a `KERN_PROCARGS2` buffer: `argc` as a native `int`, the executable
/// path, NUL padding, then `argc` NUL-terminated arguments and the
/// environment.
fn parse_arguments(bytes: &[u8]) -> Option<Vec<String>> {
    let (count, rest) = bytes.split_first_chunk::<4>()?;
    let count = usize::try_from(i32::from_ne_bytes(*count)).ok()?;
    if count == 0 {
        return Some(Vec::new());
    }
    let path_end = rest.iter().position(|byte| *byte == 0)?;
    let rest = &rest[path_end..];
    let start = rest.iter().position(|byte| *byte != 0)?;
    Some(
        rest[start..]
            .split(|byte| *byte == 0)
            .take(count)
            .map(|argument| String::from_utf8_lossy(argument).into_owned())
            .collect(),
    )
}

fn c_string(characters: &[c_char]) -> String {
    let bytes: Vec<u8> = characters
        .iter()
        .map(|character| character.to_ne_bytes()[0])
        .take_while(|byte| *byte != 0)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn procargs(count: i32, parts: &[&[u8]]) -> Vec<u8> {
        let mut bytes = count.to_ne_bytes().to_vec();
        for part in parts {
            bytes.extend_from_slice(part);
        }
        bytes
    }

    #[test]
    fn procargs_skip_the_executable_path_and_padding() {
        let bytes = procargs(
            2,
            &[b"/bin/sleep\0\0\0\0", b"sleep\0", b"30\0", b"HOME=/tmp\0"],
        );
        assert_eq!(parse_arguments(&bytes).unwrap(), ["sleep", "30"]);
    }

    #[test]
    fn procargs_tolerate_missing_and_truncated_arguments() {
        assert_eq!(
            parse_arguments(&procargs(0, &[b"/bin/x\0"])).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            parse_arguments(&procargs(3, &[b"/bin/x\0", b"x\0"])).unwrap(),
            ["x", ""]
        );
        assert_eq!(parse_arguments(&procargs(1, &[b"/bin/x"])), None);
        assert_eq!(parse_arguments(&[1, 0]), None);
        assert_eq!(parse_arguments(&procargs(-1, &[b"/bin/x\0x\0"])), None);
    }

    #[test]
    fn listing_every_pid_includes_this_process_and_launchd() {
        let pids = list_pids(PROC_ALL_PIDS, 0).unwrap();
        assert!(pids.contains(&std::process::id()));
        assert!(pids.contains(&1));
    }

    #[test]
    fn empty_group_lists_are_not_errors() {
        // No process group has this ID, so the kernel returns zero PIDs.
        let missing = u32::try_from(i32::MAX).unwrap();
        assert_eq!(group_members(missing), Some(Vec::new()));
    }
}
