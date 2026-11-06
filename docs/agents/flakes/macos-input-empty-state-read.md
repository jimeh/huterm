+++
id = "macos-input-empty-state-read"
title = "macOS input smoke empty state read"
status = "fixed"
kind = "smoke"
platforms = ["macos-arm64"]
retries = 0
review_by = "2026-12-06"

[selector]
steps = ["macos-input"]

[signature]
match = ["(?:no pane_rects in|no pane_grids in) *$"]

[[occurrences]]
date = "2026-10-27"
url = "https://github.com/jimeh/huterm/actions/runs/40587040829/job/122481163722"
pr = 281
sha = "5e1c3c66"
platform = "macos-arm64"

[[occurrences]]
date = "2026-10-28"
url = "https://github.com/jimeh/huterm/actions/runs/40768430893/job/123099018222"
pr = 318
sha = "86322b08"
platform = "macos-arm64"

[[occurrences]]
date = "2026-10-30"
local = true
platform = "macos-arm64"
+++

- **Label:** `Smoke macOS input` in `Rust smoke (macOS arm64)`, "no
  pane_rects in" followed by nothing, from `field(panes, "pane_rects")` at
  `scripts/check-native-input.ts` right after the `pane-keys-consumed`
  marker.
- **Occurrences:** 2026-10-27, on PR #281 (run 40587040829, job
  122481163722, head `5e1c3c66`). The PR does not change the input smoke or
  the code it drives; the same job passed on its earlier head `60c82966`.
  Again on 2026-10-28, on PR #318 (run 40768430893, job 123099018222, head
  `86322b08`), which also left the input smoke alone; the next head passed.
- **Evidence:** `currentState()` returned an empty string, yet the state
  read for the error message moments later held `pane_rects`. The smoke
  writes `state` every 10ms with `std::fs::write`
  (`desktop/input_smoke.rs`), which truncates the file before writing it,
  so a read that lands in between sees it empty. `currentState()` turns a
  failed read into an empty string as well.
  Seen locally on 2026-10-30 as "no pane_grids in" at the divider step,
  while adding the modifier drag step.
- **Fix:** `currentState()` retries an empty read until the file holds the
  state. Since 2026-11-06 `input_smoke.rs` also writes the state to a
  temporary file and renames it into place, as every other smoke fixture
  does, so a reader can no longer see a truncated file. If the error
  returns, the read is failing in another way.
