+++
id = "gpui-inactive-tab-close-button"
title = "huterm-gpui inactive tab close button"
status = "fixed"
kind = "rust"
platforms = ["macos-arm64", "linux-x86_64", "linux-aarch64"]
retries = 0
review_by = "2026-12-17"

[selector]
binary = "huterm-gpui"
tests = [
  "desktop::windows::window_tests::buttons::a_press_on_an_inactive_tabs_close_button_neither_selects_nor_drags_it",
]

[signature]
match = ["timed out waiting for the click to close the inactive tab"]

[[occurrences]]
date = "2026-10-28"
url = "https://github.com/jimeh/huterm/actions/runs/40766561133/job/123092154740"
pr = 319
sha = "2a5e7882"
platform = "macos-arm64"

[[occurrences]]
date = "2026-10-31"
url = "https://github.com/jimeh/huterm/actions/runs/41180152337/job/124475722453"
pr = 358
sha = "5199386c"
platform = "linux-aarch64"

[[occurrences]]
date = "2026-11-05"
url = "https://github.com/jimeh/huterm/actions/runs/42100951208/job/127504182683"
pr = 412
sha = "606f1b21"
platform = "macos-arm64"

[[occurrences]]
date = "2026-11-06"
url = "https://github.com/jimeh/huterm/actions/runs/42188279420/job/127793745152"
pr = 415
sha = "5c6c808b"
platform = "macos-arm64"
+++

- **Label:** `desktop::windows::window_tests::buttons::a_press_on_an_inactive_tabs_close_button_neither_selects_nor_drags_it`
  in `Checks (macOS arm64)` fails at `window_tests.rs:427` with "timed out
  waiting for the click to close the inactive tab".
- **Occurrences:** 2026-10-28 (PR #319 at `2a5e7882`, run 40766561133, job
  123092154740). The rerun passed, the test passed locally, and it passed
  on the PR's previous head.
- **Evidence:** the failed run's `huterm-gpui` suite took 99.58 seconds
  against about 13 locally, and the PR touched no tab code. The last state
  still held both tabs with the active one unchanged, so the click had not
  closed the inactive tab before the harness deadline.
- **Second occurrence:** 2026-10-31 on PR #358 (run 41180152337, job
  124475722453, head `5199386c`), in `Checks (Linux aarch64)`, so not
  macOS only. The `huterm-gpui` suite took 77.22 seconds there. The PR
  changes Settings and the terminal window's title row, not menus or
  selection.
- **Third occurrence:** 2026-11-05 on PR #412 (run 42100951208, attempt 1,
  job 127504182683, head `606f1b21`), hosted macOS, now failing at
  `window_tests.rs:510` with the same message and both tabs still open.
  The `huterm-gpui` suite took 199.48 seconds against about 23 locally.
  The rerun passed, although its suite took 197.29 seconds, so a slow
  suite alone does not make the test fail.
- **Fourth occurrence:** 2026-11-06 on PR #415 (run 42188279420, job
  127793745152, head `5c6c808b`), in `Checks (macOS arm64)` at
  `window_tests.rs:510`. The `huterm-gpui` suite took 203.84 seconds. The
  PR adds terminal accessibility and touches no tab or button code; the
  previous head's macOS checks passed this test.
- **Mitigation (2026-11-06):** CI reruns this signature up to twice.
  Nextest's JUnit report records each test's own duration, which replaces
  the suite wall time as evidence.
- **Cause (2026-11-17):** the click never fired. Tabs default to Fit
  widths, and a tab shows its title (`sh`) until the first foreground probe
  publishes its directory, about 50 ms after the child's first output (see
  [the pane rename flake](gpui-pane-rename-title.md)). The test read the
  close button's bounds while a tab still showed `sh`. The press's
  `run_until_parked` then delivered the directory, the tab widened, and
  the button moved from x=201 to x=234. GPUI fires a click only when the
  release lands on the pressed hitbox, so the release at the old point did
  nothing. Running the test binary 160 at a time on a 16-core arm64 Mac
  failed 5 times in 7,000 runs. Each of four instrumented failures
  read the bounds with one label still `sh` and found the button moved
  after the press. In each, the close pipeline was idle: no assessment,
  confirmation, or failure. Waiting for the closed tab's READY would not
  have helped: in one failure the stale label was on the tab that had
  already printed READY.
- **Fix (2026-11-17):** the test waits until every tab labels itself with
  its directory before reading the bounds, and asserts that the button did
  not move under the press. 10,000 parallel runs passed. With the wait
  removed, the assertion caught 2 of 4,000 runs. The harness's last state
  now also prints the close failure count and each window's close
  pipeline: busy, current, pending, confirmation, assessed jobs, and
  notices.
- **Triage:** a failure after the fix is new. "The close button moved under
  the press" means a label still changed after the wait. A timeout whose
  last state shows a confirmation and `Running` or `Unknown` jobs means the
  assessment saw the fresh tab as busy, which this fix does not address.
