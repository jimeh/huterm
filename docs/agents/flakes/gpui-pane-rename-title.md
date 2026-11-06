+++
id = "gpui-pane-rename-title"
title = "huterm-gpui pane rename title on hosted macOS"
status = "open"
kind = "rust"
platforms = ["macos-arm64", "linux-x86_64", "linux-aarch64"]
retries = 2
review_by = "2026-12-06"

[selector]
binary = "huterm-gpui"
tests = [
  "desktop::windows::window_tests::pane_names::renaming_an_unfocused_pane_republishes_its_title",
]

[signature]
match = ["waiting for the cleared name to leave the published title"]

[[occurrences]]
date = "2026-10-31"
url = "https://github.com/jimeh/huterm/actions/runs/41163015658/job/124424493459"
pr = 353
sha = "65daebae"
platform = "macos-arm64"

[[occurrences]]
date = "2026-11-04"
url = "https://github.com/jimeh/huterm/actions/runs/41757767367/job/126337838217"
pr = 383
sha = "42b6f5d9"
platform = "macos-arm64"
+++

- **Label:** `desktop::windows::window_tests::pane_names::renaming_an_unfocused_pane_republishes_its_title`
  in `Checks (macOS arm64)` timed out "waiting for the cleared name to
  leave the published title".
- **Occurrences:** 2026-10-31 on PR #353 (run 41163015658, job
  124424493459, head `65daebae`), in the same slow 127.96-second suite as
  the [Edit menu flake](gpui-edit-menu-select-all.md). It passed eight
  times in a row locally on the same head. It recurred on 2026-11-04 on
  PR #383 (run 41757767367, job 126337838217, head `42b6f5d9`) in a
  161.72-second suite. Since the
  head's last fully passing run, only a plan document and a documentation
  rename from main had changed.
- **Evidence:** the test captures the right pane's default label before
  any rename and waits for the published title to return to it. That label
  resolves the terminal's title and directory, and the first foreground
  probe publishes the shell's directory about 50 ms after its first output
  (see the core runtime guide), so on a slow runner the label the test
  captured may have changed by the time the name clears. Unconfirmed.
- **Triage:** record the run here and rerun. A hardening is to wait for the
  pane's directory before capturing the default label.
