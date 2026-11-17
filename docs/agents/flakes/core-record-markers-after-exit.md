+++
id = "core-record-markers-after-exit"
title = "huterm-core command markers queued behind the exit"
status = "fixed"
kind = "rust"
platforms = ["macos-arm64"]
retries = 0
review_by = "2026-12-17"

[selector]
binary = "huterm-core"
tests = [
  "terminal::record_tests::markers_queued_behind_the_exit_and_end_of_output_still_apply",
]

[signature]
match = [
  'last status had lifecycle Running and records|assertion failed: matches!\(ended\.lifecycle, TerminalLifecycle::Exited\(_\)\)',
]

[[occurrences]]
date = "2026-11-06"
local = true
platform = "macos-arm64"
note = "one of three full nextest runs at a load average near 22"

[[occurrences]]
date = "2026-11-10"
url = "https://github.com/jimeh/huterm/actions/runs/42811005391/job/129812112364"
pr = 458
sha = "b0c97a67"
platform = "macos-arm64"
retried = false
+++

- **Label:**
  `terminal::record_tests::markers_queued_behind_the_exit_and_end_of_output_still_apply`
  panics at `record_tests.rs:107` with `records [(1, Finished { exit_code:
  Some(0) }), (2, Finished { exit_code: Some(3) }), (3, Abandoned)]: last
  status had lifecycle Running and records [(1, Abandoned)]`, or, less
  often, at `assertion failed: matches!(ended.lifecycle,
  TerminalLifecycle::Exited(_))` once the records are right.
- **Occurrences:** 2026-11-06, in one of three full local
  `mise run test:rust` runs under nextest on a developer Mac at a load
  average near 22. On 2026-11-10 it failed `Checks (macOS arm64)` on PR
  #458 (run 42811005391, job 129812112364, head `b0c97a67`, merge commit
  `ef5ec744`) with the same last status. The entry retried nothing, so that
  job needed a manual rerun.
- **Cause:** the test, not the runtime. The test waited until
  `huterm_procinfo` showed the root as a zombie, then sent a writer report
  so the runtime would observe the exit before parsing the queued markers.
  The runtime reads the exit from the root's waiter thread in
  `child_wait.rs`, which records it only after its own `wait` returns. When
  that thread lagged under load, the runtime handled the writer report
  before it saw the exit, treated it as a failure, and stopped. It never
  parsed the queued markers, and its teardown abandoned record 1 without
  publishing the exit.
  The timeout message hid the `"probe"` failure. When the reader's real end
  of output arrived first, it parsed the markers and the test failed at
  the lifecycle assertion instead. Real hangups reach the runtime as
  `WriterHangup`, so a terminal does not stay running after its child exits.
- **Evidence (2026-11-17):** delaying the waiter by 5 ms before it records
  the exit, and the reader by 5 ms before its end of output, failed 18 of
  20 runs with the exact signature and `failures ["probe"]`. Delaying only
  the reader passed. Unchanged, the test failed 8 of 300 runs under 32
  busy loops on 16 CPUs.
- **Fix (2026-11-17):** the test waits until the waiter has recorded the
  exit, through a test-only probe on `JobContext`, and the timeout message
  lists failures. Afterwards 20 of 20 runs passed at each delay setting:
  the waiter alone at 50 and 300 ms, the reader alone at 300 ms, and both
  at 5, 10, 20, 50, and 300 ms. 300 of 300 runs passed under the same busy
  loops at load averages up to 77.
- **Triage:** a failure after the fix is new. Read the failures the
  timeout message now prints, and check `uptime`.
