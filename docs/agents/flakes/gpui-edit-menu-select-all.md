+++
id = "gpui-edit-menu-select-all"
title = "huterm-gpui Edit menu Select All"
status = "fixed"
kind = "rust"
platforms = ["macos-arm64", "linux-x86_64", "linux-aarch64"]
retries = 0
review_by = "2026-12-06"

[selector]
binary = "huterm-gpui"
tests = [
  "desktop::windows::window_tests::menus::the_edit_menus_cut_needs_a_field_and_copy_follows_focus",
]

[signature]
match = ["Select All selected the terminal"]

[[occurrences]]
date = "2026-10-30"
pr = 343
platform = "macos-arm64"

[[occurrences]]
date = "2026-10-31"
pr = 353
platform = "macos-arm64"

[[occurrences]]
date = "2026-10-31"
pr = 356
platform = "macos-arm64"

[[occurrences]]
date = "2026-10-31"
pr = 364
platform = "macos-arm64"

[[occurrences]]
date = "2026-10-31"
url = "https://github.com/jimeh/huterm/actions/runs/41171440439"
sha = "5d501107"
platform = "macos-arm64"

[[occurrences]]
date = "2026-10-31"
url = "https://github.com/jimeh/huterm/actions/runs/41222001449/job/124598321383"
pr = 332
sha = "65e3c8cd"
platform = "linux-x86_64"

[[occurrences]]
date = "2026-10-31"
local = true
platform = "macos-arm64"
note = "12 of 360 module runs on 0052a20a"
+++

- **Label:** `desktop::windows::window_tests::menus::the_edit_menus_cut_needs_a_field_and_copy_follows_focus`
  in `Checks (macOS arm64)` or `Checks (Linux x86_64)` fails at
  `menus.rs:407` with "Select All selected the terminal".
- **Occurrences:** five times in hosted macOS jobs from 2026-10-30 to
  2026-10-31 (PRs #343, #353, #356, and #364, and run 41171440439), in
  `huterm-gpui` suites that took 116 to 150 seconds. Then on Linux x86_64
  on release PR #332 (run 41222001449, job 124598321383, head `65e3c8cd`).
  It also failed locally on a macOS arm64 dev machine at normal speed: 12
  of 360 runs of `cargo test -p huterm-gpui --features test-support
  window_tests::menus` on `0052a20a`.
- **Cause:** `open_tab` starts the tab unsized, and the child can print
  `READY` before the window's first geometry applies. `menu_window`
  returned as soon as the view showed `READY`, so Select All could select
  on that snapshot, at content generation 1 and geometry revision 0, while
  the runtime had already applied the window's geometry, which keeps the
  100x32 grid. Every resize advances the content generation, so the next
  snapshot, at generation 2 and geometry revision 1, cleared the selection.
  Temporary logging caught this sequence in all 8 failures of 150 module
  runs. In each, `select_all` reached the terminal view with a snapshot,
  and the only clear was the generation check in `apply_snapshot`. A
  passing run's view had already seen generation 2 before `READY`
  matched. The selection-extraction reply, viewer invalidation, and
  foreground probe cleared nothing.
- **Fix:** `menu_window` now also waits for its view's snapshot to carry a
  geometry revision above 0. With it the module passed 360 of 360 runs on
  the same machine.
- **Triage:** a failure at this label after the fix is new, since the
  selection is made on a snapshot of the window's own geometry. Check
  whether something else advanced the generation, such as output or a
  second resize. Remove this entry once the fixed test has passed in about
  20 macOS and Linux check jobs.
