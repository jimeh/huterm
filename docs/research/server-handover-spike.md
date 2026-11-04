# Server handover spike: exec in place versus descriptor passing

Status: complete. Spike for [#394], run on 2026-11-04. The harness is
throwaway code on the [`spike/394-handover-harness`][harness] branch
(commit `f5a068d4`) and is never merged.

## Result

Both approaches hand live PTYs to a new server image without a hangup, lost
output, or a wrong exit status, on macOS and on Linux. Every scenario passed
for both. They differ in what they cost and in which failures they survive.

**Recommendation: exec in place (A)**, provided four things ship with it. It
needs a `--check-handover` dry run that rehearses the whole restore, and a
rollback copy that is taken at startup, replaced atomically, and covers the
whole bundle. The rollback target must travel outside the manifest, and the
child waiter must change as described below.

A needs about 465 lines of its own code and 8 `unsafe` sites. It keeps
portable-pty's spawn path, adds no processes, and keeps the server's PID.
Descriptor passing with a shim (B) needs about 800 lines and 17 `unsafe`
sites, most of them a second copy of the fork-and-exec child setup. B also
adds a resident process per terminal, which costs about three quarters of an
idle bash's memory on macOS.

B's material advantage is in failure handling. Any failure before its commit,
including a crash or panic while the new build parses or restores the state,
leaves every terminal with the old server. A survives the failures that the
new build detects and turns into a rollback, but a crash after `execve` loses
every terminal. The rehearsal dry run closes most of that gap.

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
  too, so the socket path and server ID carry over by construction. No
  scenario asserts them, and clients reconnect only by retrying each request.
- **A.** Spawning goes through the vendored portable-pty, as in Huterm. The
  handover pauses the readers, writes the manifest to an unlinked temporary
  file, clears `FD_CLOEXEC` on the masters, the listener, and the state file,
  and calls `execve`. The new image sets close-on-exec again before it spawns
  anything, which a scenario asserts.
  - At startup each server copies its own executable to a versioned path,
    through a temporary file and a rename. The rollback path and the source
    build ID travel in argv, not in the manifest.
  - A build that rejects the state, or fails to parse it, execs that copy
    with the same descriptors.
  - `check-handover` validates the real manifest in a child process before
    anything pauses.
- **B.** The old server spawns the new one and runs herdr's handshake on a
  0600 socket with a token: manifest, `validated`, descriptors over
  `SCM_RIGHTS` in batches of 64, `ready`, `committed`, `owned`.
  - The old server then calls `_exit` without running destructors. Before the
    commit, any failure kills the new process and resumes the old readers. So
    does a socket that closes after the commit but before `owned`, since the
    new server sends `owned` before it reads anything.
  - Each terminal has a shim, the shell's real parent. The server opens the
    PTY and passes the slave to the shim on stdin and a status pipe on fd 3.
    The shim calls `setsid`, ignores SIGPIPE, and forks. Its child performs
    portable-pty's `pre_exec` steps and execs the shell.
  - The shim then closes the slave, waits, and writes one 16-byte record:
    magic, version, kind, raw wait status, and PID. The status pipe is handed
    over with the master, so a record written during the cutover waits in the
    pipe for the new server.
  - Shims come in two forms: a dedicated binary (`handover-shim`, 267 to
    286 KB) and a `pty-shim` mode of the main harness binary.
- **New build.** It is the same binary started with a different build ID.
  `--supported-formats 2` makes it reject the format-1 manifest.
  `--require-manifest-field` makes its parse fail, as a changed schema would.
  `--crash-on-import` makes it abort after it receives the state. No real
  version skew was exercised.
- **Platforms.** macOS 27.0.1 on an Apple M3 Max, with `/bin/bash` 3.2.57.
  Linux was Debian 12 aarch64 under OrbStack's Docker, kernel 7.2.12, with
  bash 5.2.15 and `tini` as PID 1. Rust was 1.99.1 on both.
- **Repetition.** The final matrix ran every scenario once for each approach
  on each platform, after development runs that are not recorded. The
  heavy-output scenario ran six times. The benchmarks ran 20 handovers for
  each terminal count. All raw JSON is in [`spikes/handover/results`][results].
- **Cleanup.** Each run tracks its servers, shims, and shells and kills
  everything in the shells' sessions. Servers also exit when their driver
  dies. After each final matrix, `pgrep -f` found no spike process on either
  platform.

## Corrections made during the spike

An independent review of an earlier draft found several defects, and the
numbers below come from the corrected harness:

- A's rollback copy overwrote the file a rolled-back image was running from.
  On macOS that truncated it, so a second rejection crashed the server. On
  Linux the copy failed with `ETXTBSY`, which left the rolled-back server
  with no rollback.
- A panicked, instead of rolling back, on a manifest it could not parse.
- The blackout timer started after the readers had stopped, which hid B's
  worker-stop time. With the timer fixed, B's blackout at 128 terminals is
  3 ms shorter than A's on macOS and 2.5 ms longer on Linux, where the draft
  had it about half of A's.
- The htop check passed without any nudge, because htop repaints on its own
  timer.
- Two servers from early deadlocked runs were still alive. A manual `ps`
  filter (`rg -v rg`) had hidden them, because their path contains `target`.

The early harness also had two of its own bugs. Its readers stopped under
the state lock and deadlocked, and its unbuffered manifest writes issued one
syscall per JSON token.

## Scenarios

| # | Scenario | A macOS | A Linux | B macOS | B Linux |
| --- | --- | --- | --- | --- | --- |
| 1 | Job control: background job survives; `fg`, Ctrl-Z, `bg`, Ctrl-C work after handover | pass | pass | pass | pass |
| 2 | TUIs redraw after the nudge and not without it (minimal TUI, vim, htop) | pass | pass | pass | pass |
| 3 | 50 MiB of counter-patterned raw output, two handovers mid-stream: exact bytes | pass, 6 of 6 runs | pass, 6 of 6 | pass, 6 of 6 | pass, 6 of 6 |
| 4 | Exit status after handover: `exit 42`, SIGKILL, exit during the cutover, and a new-server control | pass | pass | pass | pass |
| 5 | Injected failures (see below) | pass | pass | pass | pass |
| 6 | Close hangs up the child (trap sees SIGHUP and exits 129; bash dies of SIGHUP); adopted masters stay close-on-exec; shims exit | pass | pass | pass | pass |
| 7 | Server and shims run from a FUSE mount whose daemon is killed and lazily unmounted | n/a | pass | n/a | pass |

In scenario 1 the shell stayed its own session leader (`getsid` equals its
PID) and kept the PTY as its controlling terminal. `fg` moved the terminal's
foreground group to the job, and Ctrl-Z returned it to the shell. In A the
shell's parent stayed the server PID. In B the parent stayed the shim, and
the server PID changed.

In scenario 2 each real program first went through a handover with no nudge,
as a negative control. vim (`vim.tiny` on Linux) did not repaint its buffer,
and htop with a 60-second refresh delay wrote 0 bytes. After a shrink nudge,
vim repainted and htop wrote 3.5 to 7.5 KB.

In scenario 3 the generator writes little-endian 64-bit counters to a terminal
in raw mode. The checker compares every 8-byte word, so a lost or repeated
byte would report its offset. None did.

Scenario 4's mid-cutover case sends SIGUSR1 after the readers pause and waits
until the exit is pending. In A that is a zombie that nobody has reaped. In B
it is a readable record in the status pipe. Both reported status 7 after the
handover.

Scenario 6's check that shells hold no stray descriptors is evidence for B
only. portable-pty closes every descriptor above 2 in the child, so in A that
check would pass even if the new image left masters inheritable. The A
evidence is the scenario's direct check of the close-on-exec flags.

## Results

Cutover runs from the handover request until the new owner's readers resume.
Blackout runs from the moment the first reader is told to stop until the
resume, which is the time nothing reads the PTYs. Both are medians, with the
p95 in parentheses, over 20 handovers of idle `cat` terminals. Times are in
milliseconds.

| Measure | A macOS | A Linux | B macOS | B Linux |
| --- | --- | --- | --- | --- |
| Cutover, 1 terminal | 3.7 (4.8) | 1.5 (1.9) | 2.9 (4.6) | 2.1 (12.1) |
| Cutover, 16 terminals | 6.0 (8.6) | 5.7 (8.7) | 6.2 (8.4) | 6.3 (9.0) |
| Cutover, 128 terminals | 14.3 (17.8) | 14.3 (20.8) | 14.1 (15.8) | 18.1 (24.2) |
| Blackout, 128 terminals | 14.3 (17.8) | 14.3 (20.8) | 11.1 (12.0) | 16.8 (22.8) |
| Cutover, 128 terminals, with `check-handover` | 18.0 | 16.5 | n/a | n/a |
| Reader stall under heavy output, range over 12 handovers | 3.1 to 9.0 | 0.47 to 0.81 | 0.38 to 1.5 | 0.16 to 0.29 |
| Rejected or unparseable state: time until terminals read again | 50 to 56 | 2.7 to 6.3 | 2.5 to 3.9 | 2.0 to 3.6 |
| Approach-specific code (non-blank, non-comment) | 465 lines | same code | 801 lines | same code |
| `unsafe` sites, and lines inside `unsafe` | 8, about 22 lines | same code | 17, about 180 lines | same code |
| Exit status correct | all cases | all cases | all cases, except a killed shim | all cases, except a killed shim |
| Survives rejection, a second rejection, and an unparseable manifest | yes, through the rollback copy | yes | yes, the old server keeps them | yes |
| Survives a crash on import | no, 0 of 3 | no, 0 of 3 | yes, 3 of 3 | yes, 3 of 3 |

The cutover rows include the server's own work but no emulator, so a real
runtime adds its restore time to them. In A the blackout equals the cutover,
because the readers stop as soon as the request arrives. In B, spawning and
connecting the new server happen before the blackout.

Where the time goes at 128 terminals:

- A: stopping readers takes 3.9 to 5.2 ms, `execve` until `main` takes 2.1
  to 4.6 ms, and starting 128 readers and 128 waiter threads takes 5.2 to
  6.5 ms. Writing and parsing the state takes 0.4 to 0.6 ms.
- B: spawning and connecting the new server, then stopping 256 workers, takes
  8.8 to 11.1 ms, partly inside the blackout. Moving 257 descriptors and the
  manifest takes 0.4 ms, and starting the workers takes 4.9 to 6.6 ms.

B's shim is one extra resident process per terminal; A has none. Medians over
16 shims, each the parent of an idle `sleep`, and over 4 idle shells:

| Process | macOS footprint | macOS RSS | Linux PSS | Linux private | Linux RSS |
| --- | --- | --- | --- | --- | --- |
| Dedicated shim binary (267 to 286 KB) | 1,376 KiB | 1,872 KiB | 179 KiB | 128 KiB | 1,552 KiB |
| Harness `pty-shim` mode (1.4 to 1.6 MB) | 1,360 KiB | 2,080 KiB | 219 KiB | 144 KiB | 2,028 KiB |
| Real Huterm binary idling before GPUI (26.7 MB) | 2,240 KiB | 7,984 KiB | not measured | not measured | not measured |
| Reference: idle `bash -i` | 1,792 KiB | 2,736 KiB | 962 KiB | 556 KiB | 3,268 KiB |

macOS reports `ri_phys_footprint`; Linux reports `Pss` and `Private_*` from
`smaps_rollup`. The Huterm row runs the release binary with a spike-only
`--pty-shim-idle` switch that parks before any GPUI setup, so it measures the
cost of loading the binary and its frameworks.

Code size counts the rustfmt-formatted harness. A's figure is
`approach_a.rs` plus its `waitid` helpers; B's is `approach_b.rs`,
`shim_core.rs`, and `openpty`. The shared server core, manifest, and syscall
wrappers add about 890 lines for either approach; the driver and test
programs are excluded. Ported into Huterm, B also keeps portable-pty for
spawns that do not need handover, or it retires portable-pty's Unix spawn
path.

### Failure survival in detail

| Injected failure | A | B |
| --- | --- | --- |
| New build rejects the format, twice in a row | The new image execs the rollback copy with the same descriptors. Same PID, all 3 terminals usable both times. | The import process answers `rejected`. The old server never released anything and resumes. All 3 usable both times. |
| New build cannot parse the manifest | The new image rolls back using the path from argv. All 3 usable. | The import process panics, and the old server sees the socket close and resumes. All 3 usable. |
| `check-handover` rejects (A only) | Nothing pauses. All 3 usable. | n/a |
| New binary missing | `execve` fails in the old image, which resumes. All 3 usable. | Spawning the new server fails, and the old one resumes. All 3 usable. |
| New build crashes after receiving the state | The process dies, so every master closes and every shell gets SIGHUP. 0 of 3 survive. | The old server sees the socket close, kills the import process, and resumes. 3 of 3 usable. |
| Shim killed (B only) | n/a | The shell keeps running and its terminal stays usable. The harness marks the terminal exited with an `unknown` status as soon as the status pipe closes, although the shell still runs. A real server should record the shim as lost and declare the exit only when the PTY hangs up. |

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
2. **A rollback copy must never be written over itself.** A rolled-back image
   runs from its rollback copy, so refreshing the copy in place truncated the
   running executable on macOS and failed with `ETXTBSY` on Linux. A copy
   through a temporary file and a rename fixes both, and the second-rejection
   step now tests it.
3. **The rollback target cannot live only in the manifest.** A new build that
   cannot parse the manifest cannot read a rollback path from it either. The
   harness passes the path and source build in argv.
4. **Pausing must not hold a lock the readers need.** The first harness
   stopped readers while holding the state lock, and a reader that was
   recording a chunk deadlocked against it. In Huterm the matching rule is
   that the pause must drain the reader-to-runtime channel and the parser
   before it freezes the runtime thread. It must never join a worker that is
   blocked on the runtime.
5. **A same-size `TIOCSWINSZ` sends no SIGWINCH on either platform.** No TUI
   redrew with the `same` nudge or with `none`. herdr's shrink-one-row-then-
   restore nudge worked everywhere, and so did `kill(-tcgetpgrp(master),
   SIGWINCH)`, which redraws without a size change. The shrink sleeps 30 ms
   per terminal, and the harness ran the nudges serially. That took 4.7 s for
   128 terminals after the readers had resumed, so the nudges must run in
   parallel or from each terminal's own thread.
6. **macOS rollback is slow.** Every exec of a freshly written rollback copy
   took 50 to 56 ms on macOS, against 2.7 to 6.3 ms on Linux. Later execs of
   the same new build took about 3 to 5 ms. A plausible cause, not verified,
   is that macOS checks the code signature of a file it has not executed
   before.
7. **The rollback copy must be taken at startup, and on macOS it should be
   the bundle.** After an upgrade replaces the executable, macOS gives no
   path to the running image, while Linux can still read `/proc/self/exe`.
   The harness binary is self-contained, so copying it worked on both. A
   packaged `Huterm.app` binary loads Sparkle through
   `@executable_path/../Frameworks` (`build.rs`). The link is weak, so a bare
   copy would probably still start. It would then run without Sparkle, its
   `Info.plist`, or its bundle identity. This was not tested.
8. **B moves reaping out of the server, which reopens PID reuse.** The shim
   reaps the shell before the server reads the record, so the shell's PID can
   be reused while the server may still signal it. The window is narrower
   than it looks: a PID stays reserved while it is a live process group or
   session ID, so `kill(-pgid)` stays safe while any job remains. Linux
   `pidfd_send_signal` closes the gap, and so would the shim delivering
   signals itself, which also works on macOS. This was not tested.
9. **Received descriptors are not close-on-exec atomically on macOS.** Linux
   `recvmsg` takes `MSG_CMSG_CLOEXEC`, but macOS has no such flag. The
   harness sets the flag after receiving, and a concurrent spawn in that
   window would leak a master into a child.
10. **`execve` keeps more than descriptors.** The calling thread's signal mask
    and ignored signal dispositions carry into the new image. The harness
    blocks nothing, but a Huterm thread that blocks signals would hand its
    mask on. portable-pty resets both in every child.
11. **Exiting the old server safely needs care in B.** portable-pty's Unix
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

The spike used level 1. A fresh emulator plus the nudge redrew every TUI
tested. A plain shell loses its visible screen and scrollback.

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
  footprint per terminal on macOS, or 2.2 MiB if it is the Huterm binary,
  next to bash's 1.8 MiB. On Linux it costs about 0.2 MiB of PSS, next to
  bash's 1 MiB.
- Exit statuses come from `waitpid` as they do today, and there is no status
  protocol that every later version must read.
- Its cutover and blackout times match B's within a few milliseconds at every
  size measured.
- The PID stays the same. This keeps the AppImage runtime's process model
  intact. It also keeps what launchd and systemd track as the service's main
  process, but that is inferred, not tested.
- Huterm can adopt it at any time, while B's shim must run from a terminal's
  first spawn.

A holds up only with these preconditions:

- **A rehearsal dry run, required for every handover.** `check-handover`
  should parse the real manifest, rebuild every emulator from the level-2
  history, and exit before anything pauses. That catches deterministic bugs
  in the restore path, which are the likely version-skew failures.
- **A rollback that works without the manifest.** Pass the rollback target
  outside the manifest. Replace the copy atomically. Copy what actually runs:
  the AppImage file, or the whole `.app` bundle on macOS, where an APFS clone
  keeps the copy cheap.
- **A short, infallible restore after `execve`.** Take the descriptors and
  the manifest first, and defer optional work, such as rich history, until
  the readers resume.

Choose **B** instead if any of these becomes true:

- Huterm needs to survive crashes or panics in the new build's parse and
  restore path that a rehearsal cannot catch. That could happen if level 3
  restores native parser state through code that can crash on foreign input.
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
- Take the rollback copy at server startup, write it through a rename, and
  pass its path outside the manifest. Copy the AppImage file and exec
  AppImage files on Linux, which answers the issue's open AppImage question.
  Copy or clone the whole bundle on macOS. Keep masters off fd 1023, and set
  close-on-exec on the AppImage runtime's keepalive descriptor.
- Extend `--check-handover` from a version check to a restore rehearsal, and
  run it before every handover.
- Nudge with `SIGWINCH` to the foreground process group, or with herdr's
  shrink and restore, in parallel across terminals, never with a same-size
  resize.
- Reword the acceptance criterion about a version that cannot load the state.
  Under A it holds for failures the new build detects, including a manifest
  it cannot parse. A crash or panic during restore loses the terminals unless
  the rehearsal catches it first.

## Not covered

- The new build differed from the old one only by its flags, not by real
  version skew. Its schema change was simulated with a required field.
- Clients reconnected only by retrying each request. #44's version
  renegotiation, snapshot resync, and the persistence of the server ID were
  not asserted.
- Linux ran only on aarch64 inside OrbStack, not on x86_64 or on a desktop
  session.
- The Huterm binary's shim memory was measured only on macOS. No rollback
  ran from a packaged `.app` or AppImage.
- The AppImage type-2 runtime, launchd and systemd supervision, page-cache
  eviction on a dead FUSE mount, PID reuse under B, and an inherited signal
  mask were analysed but not run.
- Restore level 2 was not built, and no production-quality tests were
  written.

## Sources

- [#394], [#44], and [#45] on GitHub.
- herdr (Apache-2.0): [`src/server/handoff.rs`][herdr-handoff],
  [`src/pty/actor/unix.rs`][herdr-actor], and the
  [session-state docs][herdr-docs]. herdr gives imported panes no wait
  handle and reports their exit reason as `Handoff`, which is the status gap
  that B's shim fills. It nudges by shrinking one row for 30 ms, and it
  proceeds when the ownership ack does not arrive.
- [AppImage type-2 runtime][type2] `src/runtime/runtime.c`.
- Huterm: `build.rs`, `third-party/vendor/portable-pty-0.9.0/src/unix.rs`,
  `crates/huterm-core/src/pty.rs`, `crates/huterm-core/src/child_wait.rs`,
  and [the core runtime guide](../agents/core-runtime.md).

[#394]: https://github.com/jimeh/huterm/issues/394
[#44]: https://github.com/jimeh/huterm/issues/44
[#45]: https://github.com/jimeh/huterm/issues/45
[harness]: https://github.com/jimeh/huterm/tree/f5a068d4224e3affaa5acf5a686f931107f0d0bb/spikes/handover
[results]: https://github.com/jimeh/huterm/tree/f5a068d4224e3affaa5acf5a686f931107f0d0bb/spikes/handover/results
[herdr-handoff]: https://github.com/herdrdev/herdr/blob/master/src/server/handoff.rs
[herdr-actor]: https://github.com/herdrdev/herdr/blob/master/src/pty/actor/unix.rs
[herdr-docs]: https://herdr.dev/docs/session-state/
[type2]: https://github.com/AppImage/type2-runtime/blob/main/src/runtime/runtime.c
