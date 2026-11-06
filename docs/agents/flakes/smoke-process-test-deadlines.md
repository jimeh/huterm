+++
id = "smoke-process-test-deadlines"
title = "smoke-process.test.ts deadlines under parallel builds"
status = "fixed"
kind = "bun"
platforms = ["macos-arm64"]
retries = 0
review_by = "2026-12-06"

[selector]
file = "scripts/smoke-process.test.ts"
tests = [
  "smoke deadline closes pipes retained by a descendant after parent exit",
  "smoke process drains both pipes and records the real exit",
  "success output does not forgive a stuck exit or ignored TERM",
]

[signature]
match = ["(?:timedOut|EPERM|timed out after 5000ms|toBe|toContain)"]

[[occurrences]]
date = "2026-10-26"
local = true
platform = "macos-arm64"
note = "descendant pipes closed early"

[[occurrences]]
date = "2026-10-27"
local = true
platform = "macos-arm64"
note = "EPERM from the group signal"

[[occurrences]]
date = "2026-10-28"
local = true
platform = "macos-arm64"
note = "Bun's 5000ms test timeout after cargo clean"

[[occurrences]]
date = "2026-10-30"
local = true
platform = "macos-arm64"
note = "stdout assertion before the deadline"
+++

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
- **Second signature:** a local `mise run test` on macOS, 2026-10-27, while
  Cargo built in parallel, failed the same test at its 1,500ms deadline:
  `process.kill(-child.pid, "SIGTERM")` in `smoke-process.ts` threw `EPERM`
  instead of signalling the group. EPERM there suggests the group's leader
  had already exited and its ID belonged to a group the user cannot signal.
  The file passed alone right after.
- **Third signature:** a local `mise run check` on macOS, 2026-10-28, run
  right after `cargo clean` while Clippy compiled the workspace in
  parallel, timed out `smoke process drains both pipes and records the real
  exit` at Bun's 5,000ms test timeout. That fixture only prints and exits.
  The file passed three times alone, and the next `mise run check` passed.
- **Fourth signature:** a local `mise run check` on macOS, 2026-10-30, while
  Clippy compiled the workspace in parallel, failed `success output does not
  forgive a stuck exit or ignored TERM` at its `stdout` assertion after
  978.75ms, before the fixture's 1,500ms deadline. The next
  `mise run check` passed it.
- **Cause of the EPERM:** macOS answers `EPERM`, not `ESRCH`, when
  `kill(-pgid)` reaches a group whose members have all exited but are not
  yet reaped; a probe with one zombie-only group reproduced it. That fits a
  descendant that had already exited, as in the first signature.
- **Fix (2026-11-06):** the supervisor treats `EPERM` as finished only when
  every member of the group is a zombie, and records the members and their
  states in a `signal-failed` event; a live member still fails. The
  retained-pipe fixture exits only after its descendant writes a marker,
  and the test asserts that the marker exists. Each test now has a
  20-second Bun timeout, the stuck-exit fixture's deadline is 3 seconds,
  and its assertions compare the whole outcome so a failure shows every
  field.
- **Triage:** a failure after the fix shows the full outcome; record it.
