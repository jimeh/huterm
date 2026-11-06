+++
id = "linux-palette-wheel-scroll"
title = "Linux palette smoke wheel does not scroll the results"
status = "open"
kind = "smoke"
platforms = ["linux-x86_64", "linux-aarch64"]
retries = 1
review_by = "2026-12-06"

[selector]
steps = ["linux-palette"]

[signature]
match = ["timed out waiting for palette wheel scroll"]

[[occurrences]]
date = "2026-10-29"
url = "https://github.com/jimeh/huterm/actions/runs/40799335675/job/123203389902"
pr = 322
sha = "0f2298e4"
platform = "linux-aarch64"

[[occurrences]]
date = "2026-10-31"
url = "https://github.com/jimeh/huterm/actions/runs/41180152337/job/124475722569"
pr = 358
sha = "5199386c"
platform = "linux-x86_64"

[[occurrences]]
date = "2026-11-04"
url = "https://github.com/jimeh/huterm/actions/runs/41761415801/job/126349739460"
pr = 383
sha = "0ea47d2a"
platform = "linux-aarch64"
+++

- **Label:** `Smoke Linux command palette` in `Rust smoke (Linux aarch64)`,
  at the wheel step of `scripts/check-palette.ts`: timed out waiting for
  `palette wheel scroll` after `xdotool click 5`.
- **Occurrences:** once, 2026-10-29, on PR #322 (run 40799335675, job
  123203389902, head `0f2298e4`), the PR that moved the palette's results
  onto `huterm-ui`'s `Command`. The x86_64 job passed the same step, and
  both Linux jobs passed it on the next head, `11797d9e`, which did not
  change the wheel path.
- **Evidence:** the last state had `results=82`, `highlighted_row=Some(0)`,
  `hover=Some(0)`, `wheel_events=1`, and `scroll_offset=0.0`: the wheel
  reached the palette, and the pointer was over the first row, but the
  list did not scroll. GPUI scrolls the element whose hitbox was under the
  pointer in the last drawn frame, and `Command`'s rows record hover
  without asking for a frame, so a wheel that arrives before a frame drawn
  with the pointer there may miss the list. The palette's own list before
  #322 redrew on hover. This is unconfirmed.
- **Second occurrence:** 2026-10-31 on PR #358 (run 41180152337, job
  124475722569, head `5199386c`), x86_64 this time, with the same last state
  (`results=84`, `highlighted_row=Some(0)`, `hover=Some(0)`,
  `scroll_offset=0.0`). The PR changes neither the palette nor
  `option_list`.
- **Third occurrence:** 2026-11-04 on PR #383 (run 41761415801, job
  126349739460, head `0ea47d2a`), aarch64, with the same last state
  (`results=84`, `highlighted_row=Some(0)`, `hover=Some(0)`,
  `wheel_events=1`, `scroll_offset=0.0`). The PR bundles a terminal font and
  does not touch the palette or `option_list`.
- **Triage:** rerun and record the run here. If it recurs, have the smoke
  wait for a frame drawn after `movePointerToRow` before the wheel, or make
  a hover change in `option_list` request a frame.
