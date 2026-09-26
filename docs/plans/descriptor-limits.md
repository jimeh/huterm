# Descriptor limits

Status: implemented for [#165](https://github.com/jimeh/huterm/issues/165).

## Outcome and scope

A Huterm launched with a soft `RLIMIT_NOFILE` of 256, as macOS launchd applies
to Finder and Dock launches, can open at least 200 working terminals. Terminals
whose descriptors are numbered 1024 or higher behave like any other terminal.
Child shells see the soft limit that Huterm started with, as they do in Ghostty.

Two ceilings exist today. Each terminal holds 9 descriptors, and nothing raises
the soft limit, so a 256 limit fails the 29th spawn. Above that, the macOS
readiness wait uses `filedescriptor` 0.8.3's `select(2)` path, which rejects
descriptors at or above `FD_SETSIZE` (1024). That failure happens after startup,
so the user sees a stopped runtime instead of a creation error.

This plan covers:

- Replacing the `select(2)` readiness wait with `poll(2)` on every Unix
  platform.
- Raising the soft descriptor limit at startup, following Ghostty.
- Restoring the original limit in each child between `fork` and `exec`, through
  a vendored `portable-pty` patch.

Out of scope:

- Reducing descriptors per terminal. Sharing one cancellation pair per terminal
  and waiting on the reader and writer descriptors that each thread already owns
  would take a terminal from 9 descriptors to 5. The limit raise makes this
  unnecessary for the acceptance criteria.
- A shared I/O reactor replacing the per-terminal reader and writer threads.
- `portable-pty`'s `close_random_fds`, which allocates between `fork` and
  `exec`. That is a pre-existing hazard, unrelated to limits.
- Verifying `poll(2)` on macOS 14. Jim will run that check separately; see
  [Open questions](#open-questions).

## Findings

### `poll(2)` on macOS PTY masters

The "macOS `poll(2)` is unreliable for PTYs" rule arrived in #2
(`ddb3c3eb`). Its only source is `filedescriptor`'s own comment ("macOS has a
broken poll(2) implementation"); the repository records no reproduction. The
belief dates from old macOS releases, where `poll` returned `POLLNVAL` for
devices.

A throwaway probe on macOS 27 duplicated a PTY master to descriptor 1500 and
exercised `poll(2)` and `kqueue`. Both behaved correctly:

| Case | `poll` result |
| --- | --- |
| Idle master | Timed out |
| Slave wrote one byte | `POLLIN` |
| Master write buffer full | No `POLLOUT` |
| Slave drained | `POLLOUT` |
| Slave closed | `POLLIN\|POLLHUP`, then `read` returned 0 |
| `sh` child printed and exited | `POLLIN` with output, then `POLLIN\|POLLHUP` and EOF |
| Socketpair after `shutdown(SHUT_WR)`, fd 1700 | `POLLIN\|POLLHUP`, repeated on the next wait |

Ghostty's read thread polls its PTY with `poll(2)` on macOS
(`src/termio/Exec.zig`).

`kqueue` with `EVFILT_USER` could replace each waiter's master dup and
cancellation pair. It is macOS-only, however, so Linux would keep a second
implementation of the sticky-cancellation contract. `poll(2)` keeps one code path
and the existing waiter structure.

`nix::poll::poll` is safe here: `PollFd::new` takes a `BorrowedFd`, and both
`filedescriptor::FileDescriptor` and `UnixStream` implement `AsFd`. This avoids
the nix 0.29 `AsFd` constraint that AGENTS.md describes for `fcntl`.

### Ghostty's limit handling

At the pinned native revision:

- `src/os/file.zig` `fixMaxFiles` reads `RLIMIT_NOFILE`. It returns the old
  value unchanged when the soft limit already equals the hard limit. With a
  finite hard limit, it sets the soft limit to the hard limit. With an infinite
  hard limit, it binary-searches the highest accepted soft limit up to 2^20. A
  failed query returns no value, so nothing is restored later.
- `src/global.zig` calls it once, as early as possible during startup, and keeps
  the original value in global `ResourceLimits`.
- `src/Command.zig` restores that value in the child after `chdir` and before
  pre-exec callbacks and `execve`. Restoration is best-effort, and failures are
  ignored.

macOS reports an infinite hard limit. Its `setrlimit(2)` page recommends
`min(OPEN_MAX, rlim_max)`, but this host accepts values up to
`kern.maxfilesperproc` (245760). The binary search finds the real ceiling on
either behavior.

systemd recommends the same pattern: raise the soft limit to the hard limit, and
reset it before executing children because of `select(2)` users. See Lennart
Poettering's [File Descriptor Limits](https://0pointer.net/blog/file-descriptor-limits.html).

### PTY crates

No crate provides a safe child-limit option:

- `portable-pty` 0.9.0 (February 2025) is still the latest release. Upstream
  commits since then are Windows and typo fixes, and no open WezTerm pull
  request or issue covers `pre_exec` or `RLIMIT_NOFILE`. Its Unix
  `spawn_command` already runs a private `pre_exec` closure (signal reset,
  `setsid`, `TIOCSCTTY`, `close_random_fds`, `umask`), but it exposes no hook.
- `pty-process` 0.5.3 exposes only an `unsafe fn pre_exec`, which core's
  `unsafe_code` denial rules out.
- `alacritty_terminal` 0.26.0 bundles a whole terminal emulator and has no limit
  handling.

A vendored `portable-pty` patch keeps the unsafe child-side call in vendored
code, as with the other vendored crates.

### Failure reporting

`TerminalRuntime::spawn` already turns `openpty`, `dup` and `socketpair`
failures into a spawn error. `pty::spawn` and `PtyProcess::into_parts` run
before startup is published, and the runtime thread sends their error through
the startup channel (`crates/huterm-core/src/terminal.rs`). The dead tab comes
only from waiter errors after startup. Removing `select(2)` removes that path.
The plan adds a regression test instead of new error plumbing.

## Settled decisions and constraints

- Follow Ghostty: raise the soft limit once at startup, record the original
  limit, and restore it in each child between `fork` and `exec` on a
  best-effort basis.
- Use `nix::poll` on all Unix platforms. Do not use `kqueue` or unlimited
  `select`.
- Keep the waiter's descriptor dup and cancellation pair. The dup ensures a
  waiter never waits on a descriptor number that another owner has closed and
  the kernel has reused.
- Keep `unsafe_code` denied in Huterm crates. `nix::sys::resource` covers the
  parent side safely; the child side lives in the vendor patch.
- The workspace pins `nix` 0.29, which needs its `poll` and `resource`
  features added, plus `term` for the raw-mode test setup.
- Validate on macOS natively and on Linux with `mise run linux:test`. The macOS
  14 check is deferred.

## Implementation sequence

Land each step as its own commit, with its tests.

### 1. Readiness wait through `poll(2)`

- Replace `filedescriptor::poll` in `ReadinessWaiter::wait`
  (`crates/huterm-core/src/pty.rs`) with `nix::poll::poll`. Map `Readiness`
  to `POLLIN` or `POLLOUT` and wait for `POLLIN` on the cancellation stream.
  Convert `Option<Duration>` to `PollTimeout`, with `None` meaning no timeout,
  and clamp durations that exceed its range.
- Treat `EINTR` as a spurious wake, like `poll_was_interrupted` does today.
  Callers already retry their reads and writes after every wake, so hangup and
  error bits need no special handling.
- Return a small outcome from `ReadinessWaiter::wait`: ready, cancelled, or
  timed out. Runtime callers keep treating every outcome as a wake, but tests
  can then assert which event released the wait.
- Add `ReadinessWaiter` unit tests in `pty.rs` whose waiter descriptors are
  above 1024. Raise the test process's soft limit and hold `/dev/null` filler
  descriptors, then create the waiter and assert its descriptor numbers. If a
  parallel test frees a low number and the waiter takes it, keep that
  descriptor as another filler and retry. Each test owns a real PTY pair with
  the slave in raw mode through `nix::sys::termios`, which needs nix's `term`
  feature. A canonical-mode master can accept megabytes without `WouldBlock`.
  Bound every fill loop. Each wait runs on a worker thread that signals just
  before calling `wait`; the test changes readiness only after that signal.
  - *Write readiness:* fill the master until `WouldBlock`. A zero-timeout
    write wait reports timed out. Start a wait with no timeout, then drain the
    slave, and require that wait to report ready within a deadline.
  - *Cancellation without hangup:* keep the master full and the slave open.
    Start a write wait with no timeout and cancel it after the start signal.
    The wait must report cancelled within a deadline. Also cancel before the
    wait, as
    `readiness_cancellation_survives_notification_before_or_during_wait` does.
  - *Read readiness:* a zero-timeout read wait on an idle master reports timed
    out. Start a wait with no timeout, then write to the slave, and require
    that wait to report ready.

  No portable signal shows that a thread is already blocked inside `poll`, so
  readiness can occasionally change just before the worker's call. The
  existing cancellation test accepts the same race. When the worker does block
  first, the outcome exposes the realistic defect, a wrong timeout conversion,
  because such a wait reports timed out instead of ready or cancelled.

  Runtime tests cannot observe that the writer reached its wait:
  `send_input` reports `Busy` from ingress accounting, which the runtime
  releases before writing to the PTY.
- Update the waiter comment and the `IoCancellation` doc comment, which both
  mention `select`.
- Replace the AGENTS.md rule ("Use `filedescriptor` for this wait ... using
  `select(2)`") with the `poll(2)` rule and the probe evidence. Also note that
  descriptors at or above `FD_SETSIZE` must remain usable.

### 2. Vendor `portable-pty` 0.9.0

- Add `portable-pty` 0.9.0 to `third-party/vendor/sources.json` from its
  crates.io archive, with checksum and upstream metadata. Start it with an empty
  patch list.
- Add the `[patch.crates-io]` override and the workspace `exclude` entry, then
  run `vendor:check` against the unpatched baseline.
- Follow `third-party/vendor/README.md`.

### 3. `child-nofile-limit` patch

- Add the named empty patch file and its complete manifest entry, including
  the description, with no upstream link. Then start the session with
  `vendor:start`. The workflow rejects manifest edits during a session.
- Add a Unix-only `CommandBuilder` setter that stores an optional
  `(soft, hard)` `RLIMIT_NOFILE` value. The field mirrors the existing
  `umask: Option<libc::mode_t>` field. A per-command value keeps process-wide
  state out of the vendored crate.
- In the Unix `pre_exec` closure, after `close_random_fds` and before the umask,
  call `libc::setrlimit` when the value is present and ignore its result.
  `setrlimit` is async-signal-safe.
- Run `vendor:finish` and `vendor:check`, then add the patch to the vendor
  README's provenance notes.

### 4. Raise the limit in core

- Add a small core module that exposes an idempotent `raise_open_file_limit()`.
  It implements Ghostty's `fixMaxFiles` with `nix::sys::resource`, including the
  2^20 search bound, and records the original limit in a `OnceLock`. A failed
  query records nothing.
- Call it first in `src/main.rs`. The smoke and benchmark examples don't need
  it.
- In `pty::spawn`, pass the recorded original to the new `CommandBuilder`
  setter when one exists. Without a raise there is nothing to restore, so
  children simply inherit.
- Document in AGENTS.md that every binary that hosts terminals, including the
  planned local server, must call the raise at startup.

## Testing strategy

`RLIMIT_NOFILE`, the descriptor table and the recorded original limit are
process-wide. The limit tests therefore go in a dedicated core integration test
binary, `crates/huterm-core/tests/descriptor_limits.rs`, which runs as its own
process. Inside it:

- A shared one-time setup lowers only the soft limit to 256 and then calls
  `raise_open_file_limit()`. This happens exactly once, so every test sees the
  same recorded original of 256. No test calls the raise again.
- Each test holds a shared mutex for its whole duration, so no test observes
  another's filled table or temporarily lowered limit. Each test closes its
  filler descriptors and restores the raised soft limit before releasing the
  mutex.
- The setup fails with a clear message when the inherited hard limit is too low
  for the 200-terminal test. It must not skip silently.

The tests:

- **Runtime above descriptor 1024.** Fill the table with `/dev/null`
  descriptors until the next one allocated is above 1024, then spawn a terminal.
  Require an input round trip, output after a burst, and a bounded explicit
  close. Before step 1 this fails on macOS with a stopped runtime, which gives
  the test-first failure.
- **200 terminals from a soft limit of 256.** Spawn 200 terminals and require
  every one to answer an input round trip. Then close them all.
- **Child limit restored.** Spawn `sh -c 'ulimit -Sn'` and assert that the
  output is 256. Before step 3, the shell reports the raised value.
- **Exhaustion fails at creation.** Temporarily lower the soft limit a few
  descriptors above the highest one in use, then require
  `TerminalRuntime::spawn` to return an error. It must not return a runtime
  that later reports `Stopped`.
- **Raise logic.** Unit-test the search as a pure function over a fake
  `setrlimit`: already at maximum, finite hard limit, infinite hard limit with a
  rejecting ceiling, and a failed query.
- **Existing coverage.** Run `mise run test` and the PTY integration tests on
  macOS, then `mise run linux:test`. Finish with `mise run verify`, which
  includes `vendor:check`, the license audit, and the architecture check.
- **Manual app check.** Launch the dev build with `sh -c 'ulimit -Sn 256; exec
  ...'`, which lowers only the soft limit. Plain `ulimit -n` also lowers the
  hard limit, which would prevent the raise. Open well over 30 tabs, and
  confirm with `ulimit -Sn` that a tab reports 256. Also launch the packaged
  `Huterm.app` from Finder once.

Confirm from the runner output that each new integration test ran by name.

## Implementation notes

- `ReadinessWaiter::wait` reports `EINTR` as `Ready`, and a wait whose
  cancellation descriptor fired as `Cancelled` even if the PTY is also ready.
  Timeouts round up to whole milliseconds so short waits cannot spin.
- Step 1 added nix's `poll`, `resource`, and `term` features together, because
  its unit tests already raise the soft limit.
- Linux moves PTY input into the line discipline on a kernel worker, so a full
  master can regain room after `WouldBlock`. The unit tests refill until a
  50 ms write wait times out. This is a wall-clock exception: no event
  reports that the worker has finished.
- The cancellation-during-wait test usually cancels before the worker enters
  `poll` on macOS, so it rarely detects a wrong timeout conversion. The read
  and write tests detect it reliably.
- The exhaustion test fills every free descriptor below the highest one in use
  before lowering the soft limit, so gaps cannot satisfy the spawn.
- The `huterm` binary depends on `huterm-core` directly to call the raise before
  `huterm_gpui::run()`.
- On the macOS 27 host, `setrlimit` accepted soft limits up to 1048575 despite
  `kern.maxfilesperproc` 245760, so the search stops just below 2^20.

## Open questions

- Do we accept the macOS 27 probe as enough evidence to drop the `select(2)`
  rule before the macOS 14 check? This plan assumes yes, with Jim running the
  macOS 14 probe later. If that check fails, a `kqueue` waiter for macOS is the
  fallback.
- Should we offer the `CommandBuilder` setter upstream to WezTerm? Upstream has
  not released since February 2025, so the vendor patch cannot wait for it.
