# Known flakes

Intermittent failures whose cause is unconfirmed or whose fix is deferred. Check
this list before treating a red check as a regression, and compare against
main's recent runs for the same job. Add an entry when a failure recurs without
a code cause. Update it when evidence changes, and delete it once a fix lands
and the failure stops.

Each entry names the failing label, the evidence so far, and how to triage it.

## `smoke:macos-quake` departed-witness activation

- **Label:** the departed-witness block in `scripts/check-quake.ts` (about lines
  470 to 480), usually "default visible endpoint" or "external target
  termination moved focus before hide".
- **Rate:** about 1 in 5 hosted macOS smoke jobs since 2026-09-26. Failures and
  passes interleave on the same code, and two consecutive failures on one head
  have occurred without a regression (PR #194, 2026-09-30). It has also failed
  once in the local Tart VM.
- **Evidence:** the final state shows `activation_seen=true`, `active=false`,
  and `current_focus_id` set to the departed witness. SettleVisible reaches
  Idle only after activation is seen, so the quake was active and the newly
  launched witness took activation back. `trace.jsonl` records animation
  samples but not focus, so the reactivation mechanism is unconfirmed.
- **Hypothesis and mitigation (2026-10-02):** the departed witness requested
  activation at launch and again on the `focus` command. Under cooperative
  activation a queued second request may apply after Huterm has seen its own
  activation, and Huterm deliberately does not re-arm activation retries after
  that. The departed witness now launches with `--no-launch-activation`, so the
  awaited `focus` command is its only request. Every witness also logs
  activation requests and active/resign notifications with uptime to
  `witness-events`, and departed-block failures print that log.
- **Triage:** if the label recurs, read the "departed witness activation
  events" in the failure. A `became_active` after Huterm summoned the quake
  confirms a late activation; its absence refutes the hypothesis. Remove this
  entry after about ten clean macOS smoke runs, the rate at which it used to
  fail at least once. Otherwise a failure at these labels is this flake; rerun.
  Investigate failures at other quake labels. The hosted runner's work area is 1024x686,
  so the quarter-height quake grid has only 8 rows: a new step that prints
  rows before the geometry section scrolls `READY:` out of view and fails
  "resize replaced retained window or shell" only in CI.

## Linux renderer smoke startup hang

- **Label:** `Smoke renderer` in `Rust smoke (Linux x86_64)` or
  `Rust smoke (Linux aarch64)` times out at 15 seconds with empty stdout and
  stderr.
- **Occurrences:** 2026-09-27 (PR #175) and 2026-09-30 (PR #196) on x86_64,
  then on main: 2026-10-25 on x86_64 (run 40211388780) and 2026-10-26 on
  aarch64 (run 40346320545). Every other Linux smoke in those jobs passed,
  and reruns passed.
- **Evidence:** the smoke prints its first `RENDERER_SMOKE prepared` line
  during the first prepare, and Rust line-buffers stdout even into a pipe,
  so an empty stdout means the process hung before its first prepare. It
  did not reproduce in 460 local runs under Xvfb in Docker on arm64,
  including 400 pinned to four cores with eight running at once. Since
  2026-10-26 the smoke prints `RENDERER_SMOKE launching` before creating
  the application and `application started` inside it; `prepared` lines
  come from the first draw inside `open_window`, and `window opened` after
  it returns. At its deadline the smoke supervisor also records a `threads`
  event in `events.jsonl` with each thread's name, state, kernel wait
  channel, and current syscall.
- **Triage:** download the `smoke-evidence-Linux-*` artifact and read
  `renderer-process/stdout.log` for the last marker and
  `renderer-process/events.jsonl` for the `threads` event. Record both here,
  then rerun once.

## `smoke:linux-integration` held-link hover timeout

- **Label:** a timeout at `hover https://example.test/a_(b)?x=1&y=2` inside the
  held-link loop.
- **Occurrences:** 2026-09-23 (main) and 2026-09-29 (Dependabot).
- **Evidence:** an owned link press without a target never triggers another
  lookup, so hover stays empty. PR #203 added `owned_link` and `window_active`
  to the smoke state, a bounded press retry that logs
  `DESKTOP_INTEGRATION ... retry=N`, and a state dump on hover timeouts. The
  retry may only reduce the rate.
- **Triage:** read the failure dump. A `retry=` line means the press had no
  target at press time. No retry line with an emptied `owned_link` means the
  target was lost during the hold, which the retry cannot cover. Check
  `window_active` for the X11 focus-flicker theory.

## Linux client-frame smoke: first key lost after a title-bar drag

- **Label:** `Smoke Linux client-side frame` (part of `Smoke Linux command
  palette` before 2026-10-02) timed out waiting for `ACK:ackdragx`, and the
  terminal showed `ckdragx`.
- **Occurrences:** once, 2026-10-01 (Release Please PR #202).
- **Evidence:** the smoke already waits until Openbox releases its move grab.
  The first key after the grab still reached the window before GPUI drew the
  refocused frame and was dropped, as documented for other input smokes.
- **Mitigation:** the post-drag acknowledgment now resends its token within a
  bound and logs `CLIENT_FRAME_SMOKE ... ackdragx retry=N` when it had to.
- **Triage:** a `retry=` line shows the drop recurred and the mitigation
  absorbed it. A failure after ten attempts means input stopped reaching the
  window, which is a real regression.

## `smoke:macos-refresh` resize indicator

- **Label:** "one-pixel resize activates the indicator without resizing the
  grid", with opacity 0 in the last observed state.
- **Occurrences:** once, 2026-09-26 (PR #174); the rerun passed.
- **Candidates:** after `check_animations`, `resize_visibility` keeps a 100 ms
  hold, which leaves `check_render_resize` about 500 ms to see nonzero
  opacity; a runner stall would miss it. Alternatively, no frame rendered after
  `window.resize`. Restoring the default hold at the start of
  `check_render_resize` would only help the stall case.
- **Triage:** read the step's panic output first; this smoke has caught real
  bugs. On a second occurrence, look for frame evidence before changing the
  check.
- **Not this flake:** "missing pending_work_cancellation success" failed twice
  on feature branches (2026-10-01 alongside a `projection.rs` panic, and
  2026-10-02 on in-progress shared-viewer work). Treat that signature as a
  regression on the branch, not as this flake. Read the `REFRESH_SMOKE
  failed:` line above it. "this terminal viewer was revoked" failed three
  times on one branch (2026-10-25, before PR #238 merged) and not since.
  "detached pending worker retained its receiver" failed once on main
  (2026-10-26, run 40320456365): the smoke checked the receiver once, but
  the executor drops a canceled task's future, and the receiver it holds,
  only when it next runs that task. The smoke now waits for the receiver
  to close, so its timeout, "detached pending worker to release its
  receiver: last observed receiver_closed=false", is a real leak.

## huterm-core foreground-job tests under full local runs

- **Label:**
  `terminal::tests::close_confirmation_distinguishes_idle_shell_foreground_job_and_exit`,
  `mux::lifecycle::tests::orphans_holding_the_terminal_need_consent_and_are_cleaned_up`
  ("orphan missing" or `!orphan.foreground`), and
  `mux::lifecycle::tests::foreground_exit_and_unavailable_runtime_are_assessed_conservatively`
  ("foreground job not observed"). Under heavier load other job-control
  tests in `terminal::tests` fail the same way, among them
  `foreground_jobs_are_named_while_running_and_cleared_on_return`,
  `osc7_reports_apply_only_while_their_reporter_holds_the_foreground`,
  `job_titles_apply_only_while_the_job_holds_the_foreground`, and
  `shutdown_should_terminate_a_distinct_foreground_process_group`.
- **Rate:** about 1 in 3 full `cargo test -p huterm-core --lib` runs on a
  developer Mac, at the 3-second "foreground job not detected" deadline. All
  three pass alone and in CI. On 2026-10-20, during the dependency refresh, 4
  of 7 full runs on a loaded developer Mac failed one of them (3 were the
  orphan test). On 2026-10-26, during the split-pane runtime work (PR #245),
  one full local `mise run test` run failed the
  `foreground_exit_and_unavailable_runtime_are_assessed_conservatively` test:
  its last assessment still showed `sleep 30` in a background group, not yet
  the foreground one `fg` moves it to. It passed in 5 isolated reruns and 3
  full reruns of the `mux::` suite, and PR #245 did not change the code it
  exercises. Later that day one `mise run verify` run for the pushed terminal
  geometry work, which changes no `huterm-core` code, failed both that test
  and `close_confirmation_distinguishes_idle_shell_foreground_job_and_exit`
  in the same run; the rerun passed. A `mise run verify` run on the
  scrollback branch (PR #243) also failed that test once that day. Also on
  2026-10-26, one `mise run test:ghostty-releasesafe` run on the pane moves
  runtime branch (#29 PR 1), which changes no job or close-assessment code,
  failed the orphan test with a new signature: the `sleep 30` job was found
  but reported as foreground (`assert!(!orphan.foreground)`). The cause is
  unconfirmed. The rerun passed all 391 tests. On 2026-10-27, during the
  pane drag work (#29 PR 3), which changes no `huterm-core` code, one full
  local `mise run test` failed three at once:
  `foreground_exit_and_unavailable_runtime_are_assessed_conservatively`,
  `close_confirmation_distinguishes_idle_shell_foreground_job_and_exit`, and
  `job_titles_apply_only_while_the_job_holds_the_foreground`. The rerun
  passed, at a load average near 9.
- **Cause:** the system shell's job control, not Huterm. These tests already
  poll their conditions. With the load average around 90 on a 16-core Mac
  (2026-10-26), four concurrent `huterm-core` lib runs failed 11 of 24 times.
  With every deadline in `terminal/tests.rs` and `mux/lifecycle/tests.rs`
  raised to 60 seconds, 8 of 24 still failed, so the jobs were stuck, not
  slow. The failing screens showed macOS `/bin/sh` (bash 3.2) printing
  `child setpgid (N to N): Operation not permitted`, or `fg` running
  `sleep 30` to completion while the terminal never named a separate
  foreground group. The `foreground_reporter` comment in `terminal/tests.rs`
  describes the same shell behavior. Longer deadlines would not help; a fix
  would give these fixtures a process group that they create and hand the
  terminal to themselves, rather than relying on `/bin/sh` job control.
- **Triage:** rerun once, and check `uptime` before suspecting the change.
  CI has not failed these.

## huterm-procinfo live group leader arguments on Linux

- **Label:**
  `tests::reads_a_live_process_group_leader_and_its_arguments` in
  `Checks (Linux x86_64)` or `Checks (Linux aarch64)`, panicking at
  `arguments(pid).unwrap()` with "called `Option::unwrap()` on a `None`
  value". After the fix, a failure there times out with "condition never
  held; last observed" and the process facts and arguments it last read.
- **Occurrences:** four, each at `lib.rs:262:35`: 2026-10-21 on x86_64
  (PR #226, run 39695920472), then on aarch64 on 2026-10-25 (Release Please,
  run 40219352235) and 2026-10-26 (Dependabot run 40266500383, and main run
  40320456365). Reruns passed.
- **Cause:** Linux renames the process `sleep` before the exec records where
  its arguments are, and `cmdline` reads as empty until then. The public
  `arguments` turns an empty list into `None`. Reading `cmdline` the moment
  `stat` first named `sleep` returned nothing in 494 of 500 tries in Docker
  on arm64. The test now waits for the name and the arguments in one poll.
  With that poll running without a pause, the old test failed 174 of 200
  runs and the new one passed 200 of 200.
- **Triage:** a failure at this test after the fix landed is new; read the
  last observed facts and arguments in its panic before rerunning. Remove
  this entry once the fixed test has passed in about 20 Linux check jobs.

## huterm-gpui replaced view's clipboard write on Linux aarch64

- **Label:**
  `desktop::windows::window_tests::a_replaced_view_receives_output_status_exit_and_clipboard_without_frames`
  in `Checks (Linux aarch64)`, "timed out waiting for the clipboard write".
- **Occurrences:** three, all on aarch64: on `main` (run 40211388780,
  2026-10-25), on PR #243 (run 40264983260, head `0ddb1fa4`), and on `main`
  again (run 40308946971, 2026-10-26). It passed 20 of 20 isolated runs on
  a macOS host.
- **Evidence:** the output and title steps before it passed, so the
  replacement viewer was live. It did not reproduce in Docker on arm64:
  700 isolated runs, including 300 pinned to one CPU, and about 100 full
  `huterm-gpui` suite runs under load. Temporary logging in the engine's
  clipboard callback saw only accepted admissions. Admission is
  nonblocking: a contended lock answers `Busy`, and Ghostty drops the write,
  so a lost race would also lose a real OSC 52 write. That remains a
  candidate, not a finding.
- **Triage:** since 2026-10-26 the window-test failure state lists each
  tab's focused view with its `epoch`, `revoked`, `title`,
  `clipboard_writes`, and `text_tail`, and the fixture prints `CLIPPED`
  right after its OSC 52 write. The PTY echoes the typed `clip` before the
  shell reads it, so `clip` alone shows only that the input arrived.
  `CLIPPED` with `clipboard_writes=0` at `epoch=1` means the terminal
  parsed the write and it never reached the view. `clip` without `CLIPPED`
  means the shell had not printed it, or that output had not reached the
  view either. A higher epoch means a second replacement. Record which,
  then rerun once.

## huterm-gpui terminfo temp-directory abort on hosted macOS

- **Label:**
  `terminfo::tests::linux_bundle_discovery_is_relative_to_the_executable` in
  `Checks (macOS arm64)`, where `create_dir_all` under `temp_dir()` returns
  EINVAL. The `TestDirectory` drop then panics on NotFound, and the double
  panic aborts every huterm-gpui lib test.
- **Occurrences:** once, 2026-10-01 (PR #204); the next push passed.
- **Triage:** check whether the change touches `terminfo.rs` or temp-directory
  handling, then rerun. A non-panicking cleanup in the drop would keep one
  failure from aborting the binary.

## `smoke:macos-palette` stray DEL at an early step

- **Label:** an early terminal-bytes check in `scripts/check-palette.ts`
  received an unexpected DEL (`\x7f`).
- **Occurrences:** local runs only, 2026-09-16 and 2026-10-26; reruns
  passed, and 3 of 3 host runs passed later on 2026-10-26. The 2026-09-16
  report put it down to other Huterm instances running on the host. Neither
  failure's output was kept.
- **Evidence:** the smoke sends Backspace as DEL. If the failing check was
  step 1's `terminalBytes("e")`, the Backspace sent just before it does not
  explain the DEL: the `query="tfs"` check between them passes only if that
  Backspace reached the palette. Real HID events from another process on
  the host, such as the quake witness's global shortcuts, go to whichever
  window is frontmost.
- **Triage:** keep the failing step, its expected and received bytes, and a
  list of what else was running on the host; record them here, then rerun.

## `smoke-process.test.ts` retained-pipe deadline

- **Label:** `smoke deadline closes pipes retained by a descendant after
  parent exit` in `scripts/smoke-process.test.ts`, with `timedOut` false
  where the test expects true, after 179.50ms.
- **Occurrences:** once in a local `mise run test` on macOS, 2026-10-26,
  while Cargo compiled the workspace in the same run; the same file passed
  alone right after, and the run before passed it too.
- **Evidence:** the fixture spawns `sh -c "sleep 30"` with inherited stdio
  and exits at once. Ending well before the 1,500ms deadline means both
  pipes closed early, as if the descendant never held them; no output from
  the descendant was kept.
- **Triage:** rerun the file alone. If it recurs, record whether the
  descendant started, for example by having it write a marker file.

## CI tool installation

`Install job tools with Mise` and Zig installation steps can fail on Zig
community-mirror rate limits (HTTP 429) or a GitHub API 403. The current
bootstrap and its limits are in [the CI guide](ci.md#toolchain-bootstrap).
Treat a single 429 or 403 there as an infrastructure failure and rerun it.
Repeated failures usually follow Actions cache eviction: the repository has
exceeded GitHub's 10 GB cache limit before, which forced cold installs.

## Local environment hazards

These are not flakes, but they produce failures that look like regressions:

- macOS frame-driven benchmarks and smokes, such as `bench:output-latency` and
  the renderer smoke, get no frames while the screen is locked or the window
  is occluded, because GPUI stops the display link. Check
  `ioreg -n Root -d1 | rg CGSSessionScreenIsLocked` first. The first run after
  unlocking can still miss intervals; retry once.
- The `smoke:macos-quake` external grab probe needs one free global chord from
  `GRAB_CANDIDATES`. If every candidate is owned by another application, the
  probe writes `failed` with each candidate's error; read that file instead of
  assuming a regression.
