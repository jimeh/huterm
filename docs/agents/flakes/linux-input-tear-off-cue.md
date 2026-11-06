+++
id = "linux-input-tear-off-cue"
title = "Linux input smoke tear-off cue timeout"
status = "fixed"
kind = "smoke"
platforms = ["linux-x86_64", "linux-aarch64"]
retries = 0
review_by = "2026-12-06"

[selector]
steps = ["linux-input"]

[signature]
match = ["timed out: tear-off cue"]

[[occurrences]]
date = "2026-11-04"
url = "https://github.com/jimeh/huterm/actions/runs/41795373417/job/126458105787"
sha = "07678e49"
platform = "linux-aarch64"

[[occurrences]]
date = "2026-11-04"
url = "https://github.com/jimeh/huterm/actions/runs/41884931970/job/126766733997"
pr = 398
sha = "8799e94f"
platform = "linux-aarch64"
+++

- **Label:** `Smoke Linux input` in `Rust smoke (Linux aarch64)`, at
  `scripts/check-cross-window-input.ts`: "timed out: tear-off cue" while
  waiting for `w0.drop=tear_off`.
- **Occurrences:** two consecutive aarch64 runs on 2026-11-04: `main` at
  `07678e49` (run 41795373417, job 126458105787) and PR #398 at `8799e94f`
  (run 41884931970, job 126766733997). Linux x86_64 passed in both runs, and
  the aarch64 job passed on `7d0abfa0` and `4ebf8dd0`.
- **Evidence:** both last states are identical: `windows=2`, `w0.tabs=2`,
  `w0.busy=false`, `w0.drop=none`, `w0.window_drag=false`. The tab-slot,
  pane-edge, and Escape steps before it passed, and the whole cross-window
  sequence took under two seconds. The step pressed on tab 1 as soon as the
  model reported the new tab. `tabs_rects` comes from the model, and a tab's
  press listener exists only from the frame that draws it. With titles fitted
  to 125 points, a press at tab 1's centre on the old one-tab frame lands past
  the only label and the add button, which starts no reorder, so no drag record
  exists and `drop` stays `none`. `drop=none` alone would also fit a drag
  whose locator sample came back `Blocked`, but `window_drag` reports any
  pending reorder from its press on, and the button was still held at the
  timeout, so `window_drag=false` means no reorder existed. This cause is
  inferred from the state; no run captured the frame.
- **Fix:** the smoke publishes `drawn_tab_ids`, the tabs the last frame drew,
  and waits for them to match `tab_ids` before each tab press.
- **Triage:** a recurrence with `drawn_tab_ids` equal to `tab_ids` rules this
  cause out; record the last state here. A timeout at "window N active and
  drawn" or "tear-off source ready" with stale `drawn_tab_ids` means frames
  stopped; compare it with the
  [Linux renderer smoke startup hang](linux-renderer-startup-hang.md). Remove
  this entry after about 20 clean aarch64 runs with the fix.
