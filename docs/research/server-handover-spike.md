# Server handover spike: exec in place versus descriptor passing

Status: complete. Spike for [#394], run on 2026-11-04. The harness is
throwaway code on the [`spike/394-handover-harness`][harness] branch
(commit `4c9b45e3`) and is never merged.

## Result

Both approaches hand live PTYs to a new server image without a hangup, lost
output, or a wrong exit status, on macOS and on Linux. Every scenario passed
for both. They differ in what they cost and in which failures they survive.

**Recommendation: exec in place (A)**, with the rollback copy, a
`--check-handover` dry run that rehearses the whole restore, and the
child-wait change described below. A needs about 445 lines of its own code
and 8 `unsafe` sites. It keeps portable-pty's spawn path, adds no processes,
and keeps the server's PID. Descriptor passing with a shim (B) needs about 790
lines and 17 `unsafe` sites, most of them a second copy of the fork-and-exec
child setup. It also adds a resident process per terminal, which costs about
three quarters of an idle bash's memory on macOS. B's one material advantage
is that a new build that crashes while importing leaves every terminal with
the old server. A loses them all in that case. A rehearsal dry run closes most
of that gap, as the recommendation explains.

## Method

The harness is a small Rust server, separate from Huterm's runtime, that owns
terminals and a Unix control socket. A driver process talks to it over that
socket, runs the scenarios, and checks the results.

- **Shared parts.** Both approaches use the same versioned JSON manifest,
  reader pause and drain, control-listener handover, and restore at level 1:
  a fresh stand-in emulator plus a redraw nudge. Each terminal's emulator is a
  capture file that receives every byte read from the master. A stopped reader
  has written every byte it read before it stops, so the capture across all
  handovers is the exact output stream. The control listener is handed over
  too, so the socket path and server ID survive. Clients see a reconnect.
- **A.** Spawning goes through the vendored portable-pty, as in Huterm. The
  handover pauses the readers, writes the manifest to an unlinked temporary
  file, clears `FD_CLOEXEC` on the masters, the listener, and the state
  file, and calls `execve`. The new image sets close-on-exec again before it
  spawns anything. At startup each server copies its own executable to a
  versioned path. A build that rejects the state execs that copy with the
  same descriptors. `check-handover` validates the real manifest in a child
  process before anything pauses.
- **B.** The old server spawns the new one and runs herdr's handshake on a
  0600 socket with a token: manifest, `validated`, descriptors over
  `SCM_RIGHTS` in batches of 64, `ready`, `committed`, `owned`. It then calls
  `_exit` without running destructors. Before the commit, any failure kills
  the new process and resumes the old readers. Each terminal has a shim, the
  shell's real parent: the server opens the PTY and passes the slave to the
  shim on stdin and a status pipe on fd 3. The shim calls `setsid`, ignores
  SIGPIPE, and forks. Its child performs portable-pty's `pre_exec` steps and
  execs the shell. The shim then closes the slave, waits, and writes one
  16-byte record (magic, version, kind, raw wait status, PID). The status pipe
  is handed over with the master, so a record written during the cutover
  waits in the pipe for the new server. Shims come in two forms: a dedicated
  binary (`handover-shim`, 267 to 286 KB) and a `pty-shim` mode of the main
  harness binary.
- **New build.** It is the same binary started with a different build ID.
  `--supported-formats 2` makes it reject the format-1 manifest, and
  `--crash-on-import` makes it abort after it receives the state. No real
  version skew was exercised.
- **Platforms.** macOS 27.0.1 on an Apple M3 Max, with `/bin/bash` 3.2.57.
  Linux was Debian 12 aarch64 under OrbStack's Docker, kernel 7.2.12, with
  bash 5.2.15 and `tini` as PID 1. Rust was 1.99.1 on both.
- **Repetition.** Every scenario ran twice for each approach on each platform.
  The heavy-output scenario ran six times. The benchmarks ran 20 handovers for
  each terminal count. All raw JSON is in [`spikes/handover/results`][results].
- **Cleanup.** Every run tracked its servers, shims, and shells, and killed
  everything in the shells' sessions. No spike process outlived any run on
  either platform, which was checked with `ps` after each matrix.

Two harness bugs distorted early numbers and were fixed before the final
runs. A reader stopped under the state lock deadlocked against its own
counter update. Writing the manifest with `serde_json::to_writer` on an
unbuffered file or socket issued one syscall per token, which added about
0.7 ms per earlier handover record to A's cutover. The deadlock carries a
real lesson, listed under surprises.

## Scenarios

| # | Scenario | A macOS | A Linux | B macOS | B Linux |
| --- | --- | --- | --- | --- | --- |
| 1 | Job control: background job survives; `fg`, Ctrl-Z, `bg`, Ctrl-C work after handover | pass | pass | pass | pass |
| 2 | TUI redraws after the nudge (minimal TUI, vim on macOS, htop on both) | pass | pass | pass | pass |
| 3 | 50 MiB of counter-patterned raw output, two handovers mid-stream: exact bytes | pass, 6 of 6 runs | pass, 6 of 6 | pass, 6 of 6 | pass, 6 of 6 |
| 4 | Exit status after handover: `exit 42`, SIGKILL, exit during the cutover, and a new-server control | pass | pass | pass | pass |
| 5 | Injected failures (see below) | pass | pass | pass | pass |
| 6 | Close hangs up the child (trap sees SIGHUP and exits 129; bash dies of SIGHUP); no master leaks into shells; shims exit | pass | pass | pass | pass |
| 7 | Server and shims run from a FUSE mount whose daemon is killed and lazily unmounted | n/a | pass | n/a | pass |

In scenario 1 the shell stayed its own session leader (`getsid` equals its
PID) and kept the PTY as its controlling terminal. `fg` moved the terminal's
foreground group to the job, and Ctrl-Z returned it to the shell. In A the
shell's parent stayed the server PID. In B the parent stayed the shim, and
the server PID changed.

In scenario 3 the generator writes little-endian 64-bit counters to a terminal
in raw mode. The checker compares every 8-byte word, so a lost or repeated
byte would report its offset. None did.

Scenario 4's mid-cutover case sends SIGUSR1 after the readers pause and waits
until the exit is pending. In A that is a zombie that nobody has reaped. In B
it is a readable record in the status pipe. Both reported status 7 after the
handover.

## Results

Cutover runs from the handover request until the new owner's readers resume.
Blackout runs from the pause until the resume, which is the time nothing reads
the PTYs. Both are medians, with the p95 in parentheses, over 20 handovers of
idle `cat` terminals. Times are in milliseconds.

| Measure | A macOS | A Linux | B macOS | B Linux |
| --- | --- | --- | --- | --- |
| Cutover, 1 terminal | 3.7 (5.4) | 1.0 (2.1) | 3.4 (4.3) | 2.1 (4.2) |
| Cutover, 16 terminals | 4.2 (7.4) | 4.9 (7.2) | 4.6 (7.3) | 7.2 (11.0) |
| Cutover, 128 terminals | 11.9 (16.2) | 16.4 (23.2) | 13.5 (16.8) | 17.4 (24.3) |
| Blackout, 128 terminals | 9.0 (10.6) | 10.6 (12.4) | 5.1 (5.6) | 7.0 (8.7) |
| Cutover, 128 terminals, with `check-handover` | 17.4 | 17.3 | n/a | n/a |
| Reader stall under heavy output, 12 handovers | 2.9 to 4.0 | 0.43 to 0.78 | 0.46 to 3.0, one 15.8 | 0.15 to 0.26 |
| Rejected state: time until terminals read again | 60 (118 in an earlier run) | 4.6 to 16 | 3.0 | 2.1 |
| Approach-specific code (non-blank, non-comment) | 445 lines | same code | 788 lines | same code |
| `unsafe` sites, and lines inside `unsafe` | 8, about 22 lines | same code | 17, about 180 lines | same code |
| Exit status correct | all cases | all cases | all cases, except a killed shim | all cases, except a killed shim |
| Survives a rejected state | yes, through the rollback copy | yes | yes, old server keeps them | yes |
| Survives a crash on import | no, 0 of 3 | no, 0 of 3 | yes, 3 of 3 | yes, 3 of 3 |

The cutover rows include the server's own work but no emulator, so a real
runtime adds its restore time to them.

B's shim is one extra resident process per terminal; A has none. Medians over
16 shims, each the parent of an idle `sleep`:

| Process | macOS footprint | macOS RSS | Linux PSS | Linux private | Linux RSS |
| --- | --- | --- | --- | --- | --- |
| Dedicated shim binary (267 to 286 KB) | 1,376 KiB | 1,872 KiB | 178 KiB | 128 KiB | 1,552 KiB |
| Harness `pty-shim` mode (1.4 to 1.6 MB) | 1,360 KiB | 2,096 KiB | 223 KiB | 148 KiB | 2,032 KiB |
| Real Huterm binary idling before GPUI (26.7 MB) | 2,256 KiB | 8,000 KiB | not measured | not measured | not measured |
| Reference: idle `bash -i` | 1,792 KiB | 2,736 KiB | 1,801 KiB | 1,212 KiB | 3,304 KiB |

macOS reports `ri_phys_footprint`; Linux reports `Pss` and `Private_*` from
`smaps_rollup`. The Huterm row runs the release binary with a spike-only
`--pty-shim-idle` switch that parks before any GPUI setup, so it measures the
cost of loading the binary and its frameworks.

Where the time goes at 128 terminals:

- A: stopping readers takes 3 to 6 ms, `execve` until `main` takes 2.6 to
  3.7 ms, and starting 128 readers and 128 waiter threads takes 4.7 to 7.5 ms.
  Writing and parsing the state takes under 0.4 ms.
- B: spawning and connecting the new server and stopping the workers takes 9
  to 10 ms, before the blackout starts. Moving 257 descriptors and the
  manifest takes 0.3 ms, and starting the workers takes 4.8 to 6.7 ms.

Code size counts the rustfmt-formatted harness. A's figure is
`approach_a.rs` plus its `waitid` helpers; B's is `approach_b.rs`,
`shim_core.rs`, and `openpty`. The shared server core, manifest, and
syscall wrappers add about 850 lines for either approach; the driver and
test programs are excluded. Ported into Huterm, B also keeps portable-pty
for spawns that do not need handover, or it retires portable-pty's Unix
spawn path.

### Failure survival in detail

| Injected failure | A | B |
| --- | --- | --- |
| New build rejects the format | The new image execs the rollback copy with the same descriptors. Same PID, all 3 terminals usable. | The import process answers `rejected`. The old server never released anything and resumes. All 3 usable. |
| `check-handover` rejects (A only) | Nothing pauses. All 3 usable. | n/a |
| New binary missing | `execve` fails in the old image, which resumes. All 3 usable. | Spawning the new server fails, and the old one resumes. All 3 usable. |
| New build crashes after receiving the state | The process dies, so every master closes and every shell gets SIGHUP. 0 of 3 survive. | The old server sees the socket close, kills the import process, and resumes. 3 of 3 usable. |
| Shim killed (B only) | n/a | The shell keeps running and stays usable. Its exit later reports as `unknown`, from status-pipe EOF without a record. |

A crash after the commit loses everything in both approaches, since the new
server alone holds the masters.

## Surprises

1. **Huterm's child waiter loses statuses under A as written.**
   `child_wait.rs` blocks in `child.wait()` on its own thread, which reaps
   whenever the child exits. A status reaped just before `execve`, but not yet
   recorded, would be gone. The harness waits with `waitid(WEXITED |
   WNOWAIT)` and reaps only under the lock the handover holds until `execve`.
   An exit during the cutover then stays a zombie for the new image to reap.
   macOS supports `WNOWAIT`, and the mid-cutover exit test passed on both
   platforms. A also cannot rebuild portable-pty's `Child` from a PID after
   `execve`, so the new image waits on raw PIDs.
2. **Pausing must not hold a lock the readers need.** The first harness
   stopped readers while holding the state lock, and a reader that was
   recording a chunk deadlocked against it. In Huterm the matching rule is
   that the pause must drain the reader-to-runtime channel and the parser
   before it freezes the runtime thread. It must never join a worker that is
   blocked on the runtime.
3. **A same-size `TIOCSWINSZ` sends no SIGWINCH on either platform.** No TUI
   redrew with the `same` nudge or with `none`. herdr's shrink-one-row-then-
   restore nudge worked everywhere, and so did `kill(-tcgetpgrp(master),
   SIGWINCH)`, which redraws without a size change. The shrink sleeps 30 ms
   per terminal, and the harness ran the nudges serially. That took 4.7 s for
   128 terminals after the readers had resumed, so the nudges must run in
   parallel or from each terminal's own thread.
4. **macOS rollback is slow on first use.** Execing the freshly copied
   rollback binary took 60 ms in the final macOS run and 118 ms in an earlier
   one, against about 5 ms on Linux and about 5 ms for later execs of the
   same new build. A plausible
   cause, not verified, is that macOS checks the code signature of a file it
   has not executed before. Copying at startup, as A must, does not avoid the
   cost at exec time.
5. **The rollback copy must be taken at startup.** After an upgrade replaces
   the executable, macOS gives no path to the running image. Linux can still
   read `/proc/self/exe`. A copy taken when the server starts works on both.
6. **B moves reaping out of the server, which reopens PID reuse.** The shim
   reaps the shell before the server reads the record, so the shell's PID is
   free while the server may still signal it, for example to close process
   groups. Linux `pidfd_send_signal` avoids this; macOS has no equivalent.
   This was not tested.
7. **Received descriptors are not close-on-exec atomically on macOS.** Linux
   `recvmsg` takes `MSG_CMSG_CLOEXEC`, but macOS has no such flag. The
   harness sets the flag after receiving, and a concurrent spawn in that
   window would leak a master into a child.
8. **The server identity carries over without extra work.** Handing over the
   listening socket keeps the socket path and any queued connections. In B,
   connections that the old server accepted during the handover are dropped
   and must be retried.
9. **Exiting the old server safely needs care in B.** portable-pty's Unix
   writer writes a newline and EOF when dropped. Huterm already avoids it by
   duplicating the master, and the harness exits with `_exit`. The
   heavy-output byte check confirms that no bytes were injected.

## AppImage and FUSE (scenario 7)

The harness ran the server, and B's main-binary shims, from a squashfuse
mount, then killed the FUSE daemon and lazily unmounted it. The mount
returned `ENOTCONN`. Both approaches kept working, rolled back from the copy
in the run directory (A), handed over to a binary on a second mount, and
reported the right exit statuses. Docker did not allow dropping the page
cache, so the binaries survived only because their pages were still
resident. A page that is evicted and faulted in again from a dead FUSE mount
raises SIGBUS. Treat this result as "worked while cached", not as safe.

Reading the [type-2 runtime source][type2] shows how Huterm's AppImage
behaves. The analysis below comes from that source and was not run:

- The runtime forks the FUSE daemon, then `execv`s `AppRun` in the PID the
  user started. Huterm's `AppRun` links to `usr/bin/huterm`. The daemon
  stays alive until it gets EPIPE on a keepalive pipe whose read end every
  process inherits without close-on-exec. The runtime also places a
  mount-directory descriptor at fd 1023.
- For A, the exec target must be the new AppImage file. Its runtime mounts
  the new image and execs the new `AppRun` in the same PID, so the shells
  stay children of the server. The rollback copy must also be the AppImage
  file (`$APPIMAGE`), not `/proc/self/exe`, whose private libraries resolve
  relative to the old mount. Two hazards remain. The runtime's `dup2` onto fd
  1023 would close a master that happens to sit there. The server should
  also set close-on-exec on the inherited keepalive descriptor, or each
  upgrade leaves the old mount alive.
- For B, the old server's exit does not release the old mount while any shim
  still holds the inherited keepalive descriptor. Old mounts then pile up
  until their terminals close. A shim that closes it instead runs from a
  mount that can disappear, and the SIGBUS risk above applies. Either way the
  shim should be a stable copy outside the mount.

## Restore levels

The spike used level 1. A fresh emulator plus the nudge was enough for every
TUI tested. A plain shell loses its visible screen and scrollback.

Level 2 adds history and mode state to the manifest. Both transports already
carry an arbitrarily large manifest: A uses an unlinked file, because a pipe's
buffer would block, and B uses the socket stream. A's dry run can parse the
history and replay it into throwaway emulators before anything pauses.
Neither level can carry a partly parsed escape sequence. Output after the
cutover can start in the middle of one, which shows as stray characters until
the next redraw. Level 3 would need a parser-state API in libghostty-vt.

## Recommendation

Choose **A, exec in place**, because:

- It has less code, and much less `unsafe` code. It does not reimplement
  child setup, which is the most fragile part of spawning.
- It adds no process or memory per terminal. A shim costs about 1.4 MiB of
  footprint per terminal on macOS, or 2.3 MiB if it is the Huterm binary,
  next to bash's 1.8 MiB. On Linux it costs about 0.2 MiB.
- Exit statuses come from `waitpid` as they do today, and there is no status
  protocol that every later version must read.
- Its cutover times match B's within a few milliseconds at every size
  measured.
- The PID stays the same. This keeps the AppImage runtime's process model
  intact. It also keeps what launchd and systemd track as the service's main
  process, but that is inferred, not tested.
- Huterm can adopt it at any time, while B's shim must run from a terminal's
  first spawn.

A covers a crash during import less well, and two measures shrink that gap:

- Make `check-handover` rehearse the restore. It should parse the real
  manifest, rebuild every emulator from the level-2 history, and exit before
  anything pauses. That catches deterministic bugs in the restore path, which
  are the likely version-skew failures.
- Keep the restore after `execve` minimal and infallible where possible. It
  should take descriptors and the manifest first, and defer optional work,
  such as rich history, to after the readers resume.

Choose **B** instead if any of these becomes true:

- Huterm needs to survive a crash in the new build's restore path that a
  rehearsal cannot catch. That could happen if level 3 restores native parser
  state through code that can crash on foreign input.
- Huterm gains a per-terminal helper process for another reason, such as
  privilege separation or a server sandbox. The shim's cost is then already
  paid.
- Terminals must move between two live servers, for example between named
  servers from [#45]. That needs `SCM_RIGHTS` anyway, and with it the exit
  status problem that only the shim solves.

## Implications for #394's plan

These are proposals for [#394], not edits to it:

- Record A as the chosen approach, with B's conditions above.
- Make the child waiter observe exits with `waitid(WNOWAIT)` and reap under
  runtime control, so a handover can stop all reaping before `execve`.
- Take the rollback copy at server startup. For AppImage installs, copy the
  AppImage file and exec AppImage files, which answers the issue's open
  AppImage question. Keep masters off fd 1023, and set close-on-exec on the
  runtime's keepalive descriptor.
- Extend `--check-handover` from a version check to a restore rehearsal.
- Nudge with `SIGWINCH` to the foreground process group, or with herdr's
  shrink and restore, in parallel across terminals, never with a same-size
  resize.
- Reword the acceptance criterion about a version that cannot load the state.
  Under A it holds for failures the new build detects and reports. A crash
  during restore loses the terminals unless the rehearsal catches it first.

## Not covered

- The new build differed from the old one only by its flags, not by real
  version skew or a changed manifest schema.
- Clients reconnected only by retrying each request. #44's version
  renegotiation and snapshot resync were not exercised.
- Linux ran only on aarch64 inside OrbStack, not on x86_64 or on a desktop
  session. `vim` was not installed under that name in the image, so htop
  covered real TUIs on Linux.
- The Huterm binary's shim memory was measured only on macOS.
- The AppImage type-2 runtime, launchd and systemd supervision, page-cache
  eviction on a dead FUSE mount, and PID reuse under B were analysed but not
  run.
- Restore level 2 was not built, and no production-quality tests were
  written.

## Sources

- [#394], [#44], and [#45] on GitHub.
- herdr (Apache-2.0): [`src/server/handoff.rs`][herdr-handoff],
  [`src/pty/actor/unix.rs`][herdr-actor], and the
  [session-state docs][herdr-docs]. herdr gives imported panes no wait
  handle and reports their exit reason as `Handoff`, which is the status gap
  that B's shim fills. It nudges by shrinking one row for 30 ms.
- [AppImage type-2 runtime][type2] `src/runtime/runtime.c`.
- Huterm: `third-party/vendor/portable-pty-0.9.0/src/unix.rs`,
  `crates/huterm-core/src/pty.rs`, `crates/huterm-core/src/child_wait.rs`,
  and [the core runtime guide](../agents/core-runtime.md).

[#394]: https://github.com/jimeh/huterm/issues/394
[#44]: https://github.com/jimeh/huterm/issues/44
[#45]: https://github.com/jimeh/huterm/issues/45
[harness]: https://github.com/jimeh/huterm/tree/4c9b45e3557cbc5dd4ed9af135eb717a19231ae7/spikes/handover
[results]: https://github.com/jimeh/huterm/tree/4c9b45e3557cbc5dd4ed9af135eb717a19231ae7/spikes/handover/results
[herdr-handoff]: https://github.com/herdrdev/herdr/blob/master/src/server/handoff.rs
[herdr-actor]: https://github.com/herdrdev/herdr/blob/master/src/pty/actor/unix.rs
[herdr-docs]: https://herdr.dev/docs/session-state/
[type2]: https://github.com/AppImage/type2-runtime/blob/main/src/runtime/runtime.c
