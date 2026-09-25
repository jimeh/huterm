# Foreground process detection

Status: agreed in discussion on 2026-09-25 and implemented in two commits:
foreground labels first, then close-confirmation evidence.

## Outcome and scope

Replace every `ps` invocation with direct, in-process queries through a new
`huterm-procinfo` crate. Idle terminals do no process work. Labels follow
command start, `exec`, and exit within about a second, usually much sooner.
Close confirmation keeps its existing consent model and classification but
reads its evidence without forking.

Out of scope: shell integration (OSC 133) and the prompt-based close checks it
would allow, kqueue process events, and Windows.

## Behaviour before this change

- With `tabs.label` set to `process` or `process_and_directory`, the desktop's
  `start_process_metadata_sampler` ran every second. It asked every runtime for
  a `JobContext`, forked `ps -axo pid,ppid,pgid,stat,tty,lstart,comm` once for
  the batch, and published names through
  `RuntimeControl::ForegroundProcess`. The runtime rejected samples whose
  foreground group had changed since sampling. A scan measured about 80 ms of
  CPU on a macOS host with about 1,350 processes, whether or not any terminal
  was busy.
- Close assessment (`jobs::inspect_all`) forks the same `ps` scan on demand,
  with a one-second timeout and a reader thread, then classifies processes by
  descent from the shell, foreground group, and TTY name.

## How other terminals do this

Every surveyed terminal asks the PTY for its foreground group with
`tcgetpgrp`, then queries only that process. None forks `ps` for labels.

| Project | Name source | When it checks |
| --- | --- | --- |
| tmux | `proc_pidinfo` short info or `/proc/<pgrp>/cmdline` | Only after pane output, at most every 500 ms |
| Zed | `sysinfo`, refreshing one PID | On each output wakeup, one refresh in flight |
| WezTerm | `proc_pidpath` or `/proc/<pid>/exe` | On demand, stale-while-revalidate, 300 ms TTL |
| kitty | Group map from `proc_listallpids` or `/proc` | Batched, 1 s cache; OSC 133 for close checks |
| Ghostty | none | Close checks use OSC 133 prompt state only |
| iTerm2 | libproc and sysctl | kqueue fork/exec/exit events, rebuilds at most every 0.5 s |
| hubris | `/proc/<pid>/comm`, `proc_pidpath`, `proc_name` | Every 750 ms per tab, 5 s name cache |

## Crate boundary

`huterm-procinfo` reports operating-system facts and has no dependency on other
Huterm crates. Its macOS backend calls `proc_pidinfo`, `proc_listpids`, and
`sysctl` through three small unsafe helpers in `macos.rs`; the rest of the
crate, like the workspace, denies unsafe code.

- `process(pid)` returns the parent, process group, controlling TTY device,
  zombie state, an optional start time, and the short command name.
- `arguments(pid)` returns argv.
- `group_members(group)` lists a group's PIDs.
- `display_name(arguments)` applies the naming rule below.
- `process_table(ttys)` returns every process and each terminal's members,
  for close confirmation.
- `tty_device(path)` reads a terminal's device number.

`huterm-core` keeps behaviour: `tcgetpgrp` on its own PTY, probe scheduling,
metadata publishing, `JobState` classification, consent, and shutdown groups.

### Platform backends

| Need | macOS | Linux (standard library only) |
| --- | --- | --- |
| Process facts | `proc_pidinfo` BSD info, or short BSD info for other users | `/proc/<pid>/stat` |
| argv | `sysctl(KERN_PROCARGS2)` | `/proc/<pid>/cmdline` |
| Group members | `proc_listpids` group filter | Scan `/proc/*/stat` |
| Close candidates | Every PID, plus the TTY filter | Scan `/proc/*/stat` |

The macOS backend depends only on `libc`. A first draft used the `libproc` and
`sysinfo` crates, but neither can read other users' processes: full BSD info
fails for them, so a `sudo` job had no label and would escape close
confirmation. Short BSD info reads every process but has no TTY or start time,
and argv of other users' processes is unavailable, so their names fall back to
the kernel's 16-byte command name. `sysctl(KERN_PROC_PID)` answers for every
user. `libc` does not define its `kinfo_proc`, so the backend checks the
648-byte size and parses only the leading start-time `timeval`.
`proc_listpids` returns 0 for both an empty list and an error; the `libproc`
crate misread empty lists as failures because it never clears `errno`. The
backend clears `errno` before each call.

Linux cannot filter processes by group or TTY in the kernel. A full
`/proc/*/stat` scan measured about 5 ms for 1,503 processes. The
`task/<tid>/children` file is faster but documented as imprecise while
processes change, so close evidence does not use it. Parse `stat` fields after
the last `)`, because command names can contain spaces and parentheses.

## Display names

Take the basename of argv[0] and trim a login shell's leading `-`. If that
names a known interpreter and argv[1] exists and does not start with `-`, use
the basename of argv[1] instead. This names scripts the same way on both
platforms:

- macOS `comm`, `proc_name`, and `proc_pidpath` all report the interpreter,
  and do not match what was typed: `/bin/sh` reports `bash` and `python3`
  reports `python3.14`.
- Linux `comm` names direct `#!/bin/sh` scripts but not `#!/usr/bin/env node`
  scripts, which re-exec the interpreter.

Interpreters: `sh`, `bash`, `zsh`, `dash`, `ksh`, `mksh`, `fish`, `csh`,
`tcsh`, names starting with `python`, `node`, `nodejs`, `deno`, `bun`, `ruby`,
`perl`, `php`, `lua`, `luajit`, `tclsh`, and `pwsh`. If argv cannot be read,
fall back to the short command name. Names keep the existing 256-byte limit.

## Foreground probe

The runtime owner thread probes its own terminal:

1. Read the foreground group with `tcgetpgrp`.
2. Read the facts of the process whose PID equals the group. If it is missing
   or a zombie, use the lowest live PID among the group's members.
3. Reuse the previous name of an idle root shell whose process facts are
   unchanged. Otherwise read argv and apply the naming rule. A job's argv is
   read on every probe, because `exec` of the same interpreter with another
   script keeps the PID, start time, and kernel name.
4. The terminal is idle when the selected process is the root process and its
   name is a shell; publish no name. Otherwise publish the name.

Publishing reuses `publish_metadata`, which suppresses duplicates. Root exit
clears the name immediately. Running on the owner thread removes the
stale-sample check, `SampledForegroundGroup`, and the desktop sampler.

Probing is always on, independent of the label setting. Probes cost
microseconds and idle terminals do no work, so an opt-in flag would only add
plumbing, and foreground names stay available to future clients.

## Probe triggers

| Trigger | Probe | Catches |
| --- | --- | --- |
| Encoded input containing CR, LF, ETX, EOT, SUB, or FS | +50 ms and +500 ms | A command starting, quiet or not; interrupts and suspends |
| First output after 250 ms of silence | +50 ms | A job ending and the prompt redrawing |
| Title change | +50 ms | Shells titling the running command |
| Previous probe found a non-idle foreground | +1 s | A process replacing itself with `exec`, and silent exits |

There is no probe at spawn: the child may not have exec'd yet and would read
as Huterm itself. The first output, such as a shell prompt, triggers the first
probe. Probes run at least 100 ms apart. When the shell is idle, no deadline is
armed, so an idle terminal has no wakeups. A flood arms one probe when it
starts and then the one-second poll only while a job holds the foreground; the
output path never calls `tcgetpgrp`.

The scheduler is a pure state machine that takes explicit instants. The runtime
loop waits until the earliest deadline instead of waiting indefinitely.

## Working directory (follow-up)

Each probe also reads the working directory of the selected foreground
process, falling back to the root's when another user owns that process. The
directory is re-read on every probe because `cd` does not change the
foreground process; the prompt redraw after `cd` triggers a probe. Process
directories are local, so `new_tab_directory = "inherit"` works without shell
integration.

An OSC 7 report records the foreground group at arrival and applies only
while that group keeps the foreground; otherwise the process directory
applies. A shell's report wins at its prompt, a running job shows its own
directory, a nested shell without OSC 7 follows its own `cd`, and a remote
shell's reports under `ssh` expire when `ssh` exits. An empty report clears
the reported value and falls back to the process directory. WezTerm lets any
OSC 7 win once reported; kitty uses it only at an OSC 133 prompt.

## Close confirmation (second commit)

Build the same `Process` evidence from `huterm-procinfo` and keep `classify`,
`covered_by`, `JobLifecycle`, and shutdown groups unchanged.

- macOS candidates: every PID with its facts. Listing every PID and reading its
  info measured about 0.6 ms for 1,363 processes. TTY membership comes from the
  kernel's TTY filter, because other users' short info has no TTY.
- Linux candidates: one `/proc/*/stat` scan, which includes each TTY.
- Compare TTYs by device number from the PTY path instead of normalized names.
- Start times gain precision: microseconds on macOS, clock ticks on Linux,
  instead of `lstart` seconds. Identities change format, which is safe because
  consent and later checks use the same source within one run.
- Remove `process_table`, its reader thread, and its timeout.

## Known limitations

- Reports take the foreground group when the runtime parses them, not when the
  reader read them. If output is backlogged while a job reports and then exits,
  its report is credited to the shell and applies at the prompt until the
  shell's next report. Reading the group per output chunk would put
  `tcgetpgrp` back on the output path. Directory inheritance accepts only local
  directories, so a stale remote report affects only the label.
- A root shell without job control, such as `sh -c 'cd src && vim'`, runs its
  commands in its own group. The probe selects the group leader, so the
  terminal reads as idle and the label shows the directory. The `ps` sampler
  also chose the leader and showed the shell's name. Naming such children would
  need a group-member scan on every probe, a full `/proc` scan on Linux.

## Testing

- `huterm-procinfo`: naming-rule unit tests, `stat` parsing with hostile
  command names, and real-process tests for facts, argv, a shebang script, and
  a spawned process group. These build without Ghostty.
- Scheduler: controlled-time tests for each trigger, the rate limit, the job
  poll, and no deadline while the shell is idle.
- Runtime PTY tests: a quiet command started by input is named; `exec` updates
  the name; returning to the shell clears it; a shebang script shows its name.
- Close confirmation: keep the classification and consent tests; add evidence
  tests for foreground, background, TTY-attached, and leaderless groups.
- Desktop integration smoke: assert the exact `hutermfgprobe` label on macOS
  as well as Linux.
- Run on macOS and in Linux Docker, plus `mise run license` for the new
  dependencies.
