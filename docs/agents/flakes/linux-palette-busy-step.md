+++
id = "linux-palette-busy-step"
title = "Linux palette smoke busy step"
status = "fixed"
kind = "smoke"
platforms = ["linux-x86_64", "linux-aarch64"]
retries = 0
review_by = "2026-12-06"

[selector]
steps = ["linux-palette"]

[signature]
match = [
  "(?:expected 3 core tabs, got 4|structural operation in progress)",
]

[[occurrences]]
date = "2026-10-27"
url = "https://github.com/jimeh/huterm/actions/runs/40515770496"
pr = 269
sha = "ad082c2b"
platform = "linux-x86_64"
note = "extra tab"

[[occurrences]]
date = "2026-10-27"
url = "https://github.com/jimeh/huterm/actions/runs/40578248073/job/122451107921"
pr = 279
sha = "ad33b662"
note = "closed palette"

[[occurrences]]
date = "2026-10-28"
url = "https://github.com/jimeh/huterm/actions/runs/40767320033/job/123094963780"
pr = 318
sha = "bbeb8be1"
platform = "linux-x86_64"
note = "closed palette"

[[occurrences]]
date = "2026-10-30"
url = "https://github.com/jimeh/huterm/actions/runs/41006554547/job/123902649454"
pr = 331
sha = "cad04bc4"
note = "extra tab"

[[occurrences]]
date = "2026-10-30"
url = "https://github.com/jimeh/huterm/actions/runs/41007277899/job/123905010885"
pr = 331
sha = "45219c47"
platform = "linux-aarch64"
note = "closed palette"

[[occurrences]]
date = "2026-10-30"
url = "https://github.com/jimeh/huterm/actions/runs/41108259656/job/124243665211"
pr = 343
sha = "10dcb4e7"
note = "extra tab"

[[occurrences]]
date = "2026-10-31"
url = "https://github.com/jimeh/huterm/actions/runs/41161794041/job/124420826357"
pr = 354
sha = "ef1d7012"
platform = "linux-aarch64"
note = "closed palette"

[[occurrences]]
date = "2026-10-31"
url = "https://github.com/jimeh/huterm/actions/runs/41167918791/job/124439483926"
pr = 355
sha = "88542f14"
platform = "linux-x86_64"
note = "closed palette"

[[occurrences]]
date = "2026-10-31"
url = "https://github.com/jimeh/huterm/actions/runs/41209548791/job/124561468799"
pr = 362
sha = "5bdce63f"
note = "extra tab"

[[occurrences]]
date = "2026-10-31"
url = "https://github.com/jimeh/huterm/actions/runs/41212003791/job/124568761526"
pr = 363
sha = "e7793fe7"
note = "extra tab"

[[occurrences]]
date = "2026-10-31"
url = "https://github.com/jimeh/huterm/actions/runs/41213983677/job/124574569527"
pr = 363
sha = "c87cb93a"
platform = "linux-aarch64"
note = "closed palette"

[[occurrences]]
date = "2026-11-03"
url = "https://github.com/jimeh/huterm/actions/runs/41731482596/job/126249669677"
pr = 383
sha = "9d5e1247"
note = "extra tab"

[[occurrences]]
date = "2026-11-03"
url = "https://github.com/jimeh/huterm/actions/runs/41731482596/job/126249669688"
pr = 383
sha = "9d5e1247"
platform = "linux-aarch64"
note = "closed palette"

[[occurrences]]
date = "2026-11-04"
url = "https://github.com/jimeh/huterm/actions/runs/41792849682/job/126450000000"
pr = 397
sha = "8d7b1431"
note = "extra tab"

[[occurrences]]
date = "2026-11-05"
url = "https://github.com/jimeh/huterm/actions/runs/42097959036/job/127493242342"
pr = 412
sha = "179f5e2b"
platform = "linux-x86_64"
note = "closed palette"
+++

- **Label:** `Smoke Linux command palette` in either Linux Rust smoke job,
  from the `busy-on` step of `scripts/check-palette.ts`, with one of two
  signatures:
  - "ghostty: expected 3 core tabs, got 4" from the `coreState(3)` call
    just before `delete-target`, with an extra main-window tab active at
    `w0.active_index=2`; where recorded, its ID, 16, was below
    `fixture-session` 19.
  - A timeout waiting for `selected=new_tab` and "structural operation in
    progress", with `w0.busy=true`, `w0.palette=false`, and last terminal
    bytes `1f 43 44`.
- **Occurrences:** 15 between 2026-10-27 and 2026-11-05, on x86_64 and
  aarch64, on PRs that did not change the palette or its smoke, across
  about 620 CI runs. Reruns and later heads passed where recorded. The extra
  tab: PRs #269 (run 40515770496), #331 (41006554547), #343 (41108259656),
  #362 (41209548791), #363 (41212003791), #383 (41731482596), and #397
  (41792849682). The closed palette: PRs #279 (40578248073), #318
  (40767320033), #331 (41007277899), #354 (41161794041), #355
  (41167918791), #363 (41213983677), #383 (41731482596), and #412
  (42097959036).
- **Cause:** on Linux the smoke sends keys through `xdotool` and commands
  through files, and nothing orders the two. Opening the palette checks
  the window's availability, so a `busy-on` handled before the palette
  shortcut left the palette closed, and the busy window also kept the
  typed `new tab` from the terminal. After Enter, the step waited for a
  state that already held before Enter, then sent `busy-off`, so an Enter
  handled after `busy-off` ran `new_tab`.
- **Reproduction:** in the Linux container, probe copies of the smoke that
  queued each `xdotool` key 150ms late, in order, reproduced both CI states
  6 of 6 times: late keys from the palette shortcut on gave the closed
  palette, and a late Enter and Escape gave the extra tab. The fixed step
  passed both 6 of 6 times. Under heavy CPU load the unmodified smoke hit
  the extra tab once in 240 runs, with no ESC in the terminal bytes as in
  CI. Where the Escape after `busy-off` went in those failures is still
  unexplained; the fix does not depend on it. The fixed smoke had no
  busy-step failure in 240 runs. Its two failures there were the wheel
  flake above and a `show_quake` timeout, which the unmodified smoke also
  hit under heavier load.
- **Fix (PR #427):** the step waits for the palette to open and take
  focus before `busy-on`. After Enter it waits for the palette's
  `diagnostic=Some("Unavailable: structural operation in progress")`,
  which only the refused Enter sets, before `busy-off`. The macOS smoke
  runs the same step. Its `native` keys are queued, not dispatched, when
  the command returns, so it had the same gap, although no macOS failure
  in this step is recorded.
- **Triage:** a failure in this step after the fix is new. Remove this
  entry after about 20 clean Linux palette CI runs with the fix.
