+++
id = "gpui-replaced-view-clipboard-write"
title = "huterm-gpui replaced view's clipboard write on Linux"
status = "open"
kind = "rust"
platforms = ["linux-x86_64", "linux-aarch64"]
retries = 2
review_by = "2026-12-06"

[selector]
binary = "huterm-gpui"
tests = [
  "desktop::windows::window_tests::a_replaced_view_receives_output_status_exit_and_clipboard_without_frames",
]

[signature]
match = ["timed out waiting for the clipboard write"]

[[occurrences]]
date = "2026-10-25"
url = "https://github.com/jimeh/huterm/actions/runs/40211388780"
sha = "792f4f00"
platform = "linux-aarch64"

[[occurrences]]
date = "2026-10-26"
url = "https://github.com/jimeh/huterm/actions/runs/40264983260"
pr = 243
sha = "0ddb1fa4"
platform = "linux-aarch64"

[[occurrences]]
date = "2026-10-26"
url = "https://github.com/jimeh/huterm/actions/runs/40308946971"
platform = "linux-aarch64"

[[occurrences]]
date = "2026-10-28"
url = "https://github.com/jimeh/huterm/actions/runs/40633635594"
pr = 294
sha = "fa3fe8f8"
platform = "linux-aarch64"

[[occurrences]]
date = "2026-10-28"
url = "https://github.com/jimeh/huterm/actions/runs/40647544648"
pr = 304
sha = "4aa96be9"
platform = "linux-x86_64"
+++

- **Label:**
  `desktop::windows::window_tests::a_replaced_view_receives_output_status_exit_and_clipboard_without_frames`
  in `Checks (Linux aarch64)` or `Checks (Linux x86_64)`, "timed out
  waiting for the clipboard write".
- **Occurrences:** five. Four on aarch64: on `main` (run 40211388780,
  2026-10-25), on PR #243 (run 40264983260, head `0ddb1fa4`), on `main`
  again (run 40308946971, 2026-10-26), and on PR #294 (run 40633635594,
  head `fa3fe8f8`, 2026-10-28). The fifth, the first on x86_64, was on
  PR #304 (run 40647544648, head `4aa96be9`, 2026-10-28). It passed 20 of
  20 isolated runs on a macOS host.
- **Triaged state:** the PR #294 and PR #304 runs both show the triage
  state below: `epoch=1 revoked=false exited=false title="two"
  clipboard_writes=0 text_tail="READYout OUTtitle clip CLIPPED"`. That is
  the case where the terminal parsed the write and it never reached the
  view, with no second replacement. On PR #294 the next head passed.
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
