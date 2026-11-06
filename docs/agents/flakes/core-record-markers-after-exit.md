+++
id = "core-record-markers-after-exit"
title = "huterm-core command markers queued behind the exit"
status = "open"
kind = "rust"
platforms = ["macos-arm64"]
retries = 0
review_by = "2026-12-06"

[selector]
binary = "huterm-core"
tests = [
  "terminal::record_tests::markers_queued_behind_the_exit_and_end_of_output_still_apply",
]

[signature]
match = ["last status had lifecycle Running and records"]

[[occurrences]]
date = "2026-11-06"
local = true
platform = "macos-arm64"
note = "one of three full nextest runs at a load average near 22"
+++

- **Label:**
  `terminal::record_tests::markers_queued_behind_the_exit_and_end_of_output_still_apply`
  panics at `record_tests.rs:107` with `records [(1, Finished { exit_code:
  Some(0) }), (2, Finished { exit_code: Some(3) }), (3, Abandoned)]: last
  status had lifecycle Running and records [(1, Abandoned)]`.
- **Occurrences:** once, 2026-11-06, in one of three full local
  `mise run test:rust` runs under nextest on a developer Mac at a load
  average near 22, while adding nextest. The other two runs and a
  `cargo test` run passed it. CI has not failed it.
- **Evidence:** after 5 seconds the runtime still reported the terminal as
  running, command 1 was abandoned, and none of the three markers the test
  queued before the end of output had applied. The test pauses the
  runtime, lets the shell exit, queues the markers, a writer failure, and
  the end of output, then resumes; the real reader's end of output may
  arrive anywhere around them. A state where the end of output abandoned
  command 1 and the queued markers and the exit were never processed looks
  like an ordering gap in the runtime rather than a slow runner, but that
  is unconfirmed.
- **Triage:** registered for classification only, with no retry, because
  the state points at a possible runtime bug. On a recurrence, record the
  last status and whether the run was under load, and investigate the
  order in which the runtime handles a queued end of output, a writer
  failure, and output already queued behind them.
