+++
id = "core-foreground-job-tests"
title = "huterm-core foreground-job tests under full local runs"
status = "fixed"
kind = "rust"
platforms = ["macos-arm64"]
retries = 0
review_by = "2026-12-06"

[selector]
binary = "huterm-core"
tests = [
  "terminal::tests::close_confirmation_distinguishes_idle_shell_foreground_job_and_exit",
  "terminal::tests::foreground_jobs_are_named_while_running_and_cleared_on_return",
  "terminal::tests::osc7_reports_apply_only_while_their_reporter_holds_the_foreground",
  "terminal::tests::job_titles_apply_only_while_the_job_holds_the_foreground",
  "terminal::tests::shutdown_should_terminate_a_distinct_foreground_process_group",
  "mux::lifecycle::tests::orphans_holding_the_terminal_need_consent_and_are_cleaned_up",
  "mux::lifecycle::tests::foreground_exit_and_unavailable_runtime_are_assessed_conservatively",
]

[signature]
match = [
  "(?:foreground job not (?:detected|observed)|orphan missing|!orphan\\.foreground|terminal child did not report exit)",
]

[[occurrences]]
date = "2026-10-20"
local = true
platform = "macos-arm64"
note = "4 of 7 full runs during the dependency refresh"

[[occurrences]]
date = "2026-10-26"
local = true
platform = "macos-arm64"
note = "PR #245 split-pane runtime work"

[[occurrences]]
date = "2026-10-26"
local = true
platform = "macos-arm64"
note = "terminal geometry work, two tests in one verify run"

[[occurrences]]
date = "2026-10-26"
local = true
platform = "macos-arm64"
note = "PR #243 scrollback verify run"

[[occurrences]]
date = "2026-10-26"
local = true
platform = "macos-arm64"
note = "ReleaseSafe run, orphan reported as foreground"

[[occurrences]]
date = "2026-10-27"
local = true
platform = "macos-arm64"
note = "pane drag work, three tests in one run"

[[occurrences]]
date = "2026-10-30"
local = true
platform = "macos-arm64"
note = "modifier pane drag verify run"

[[occurrences]]
date = "2026-11-04"
local = true
platform = "macos-arm64"
note = "#399 transparency work, two runs and a mux run"

[[occurrences]]
date = "2026-11-05"
local = true
platform = "macos-arm64"
note = "word and line selection ReleaseSafe run"

[[occurrences]]
date = "2026-11-06"
local = true
platform = "macos-arm64"
note = "pane tear-off work (#401)"

[[occurrences]]
date = "2026-11-03"
url = "https://github.com/jimeh/huterm/actions/runs/41731482596/job/126249669413"
pr = 383
sha = "9d5e1247"
platform = "macos-arm64"
note = "hosted CI: the orphan test's `!orphan.foreground`"
+++

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
  passed, at a load average near 9. On 2026-10-30, during the modifier pane
  drag work, which changes no `huterm-core` code, one local `mise run
  verify` failed `close_confirmation_distinguishes_idle_shell_foreground_job_and_exit`
  ("terminal child did not report exit"); the rerun passed at a load
  average near 22. On 2026-11-04, during the per-window transparency work
  (#399), which changes no `huterm-core` code, two consecutive local `mise
  run test` runs failed
  `foreground_exit_and_unavailable_runtime_are_assessed_conservatively`
  at a load average near 6, and a `mux::` filtered run then failed the
  orphan test. It passed in 3 isolated reruns, the whole `huterm-core`
  suite passed with `--test-threads=4`, and a third full workspace run
  passed it. On 2026-11-05, during the word and line selection work, one
  `mise run test:ghostty-releasesafe` run failed it with the same message,
  although the change touches no job or close-assessment code; it passed in
  3 isolated ReleaseSafe reruns. On 2026-11-06, during the pane tear-off
  work (#401), which changes no `huterm-core` code, one local `mise run test`
  failed it with the same message (`sleep 30` still in a background group);
  it passed in 3 isolated reruns.
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
- **CI:** recording the registry's runs found one hosted failure the prose
  above missed: on 2026-11-03, PR #383 (run 41731482596, job 126249669413)
  failed the orphan test at `assert!(!orphan.foreground)` in
  `Checks (macOS arm64)`.
- **Fix (2026-11-06):** the fixtures own their process groups. The test
  binary reruns itself as a helper that calls `setpgid` and `tcsetpgrp`,
  prints a marker once it holds the foreground, and returns the terminal on
  request, so no test relies on `/bin/sh` job control; see
  [the core runtime guide](../core-runtime.md#process-information). Before
  the fix, 4 of 30 full `huterm-core` lib runs failed at a load average of 7
  to 25, two at a time; after it, 20 of 20 passed at 20 to 29, as did 20 of
  20 `mux::` and 20 of 20 `terminal::tests` runs.
- **Triage:** a failure after the fix is new; read its last observation,
  which the shared `wait_until` prints, and check `uptime`.
