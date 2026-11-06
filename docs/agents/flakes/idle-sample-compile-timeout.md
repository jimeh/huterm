+++
id = "idle-sample-compile-timeout"
title = "idle-sample.test.ts native sampler compile timeout"
status = "open"
kind = "bun"
platforms = ["macos-arm64"]
retries = 0
review_by = "2026-12-06"

[selector]
file = "scripts/idle-sample.test.ts"
tests = [
  "native idle sampler rejects missing windows in either visibility state",
]

[signature]
match = ["timed out after 5000ms"]

[[occurrences]]
date = "2026-10-27"
pr = 283
platform = "macos-arm64"

[[occurrences]]
date = "2026-11-06"
local = true
platform = "macos-arm64"
note = "mise run verify with test:rust compiling in parallel"
+++

- **Label:** `native idle sampler rejects missing windows in either
  visibility state` in `scripts/idle-sample.test.ts`, timing out after
  5000ms with `exitCode` null where the test expects 0 (line 30).
- **Occurrences:** once in the hosted `Checks (macOS arm64)` job on
  2026-10-27, for PR #283, which changes no script or idle code; the
  rerun passed. Once locally on 2026-11-06 during `mise run verify`, with
  `test:rust` compiling in parallel, on a dependency-bump branch that
  changes no script or idle code; the file alone passed in about 1 second.
- **Evidence:** the test runs `xcrun clang` through `Bun.spawnSync` with no
  timeout option, and neither `bunfig.toml` nor the test overrides Bun's
  default 5,000ms test timeout. The null exit code at line 30 is the
  compiler's, so it ended by a signal. The synchronous spawn blocks the
  event loop, so the test timer cannot have interrupted it, and the signal's
  source is unknown; the log shows no compiler output. A slow first `xcrun`
  or SDK lookup on a cold runner is a hypothesis, not a confirmed cause.
- **Triage:** rerun the job. If it recurs, time the compile step in the
  test or give it a longer timeout.
