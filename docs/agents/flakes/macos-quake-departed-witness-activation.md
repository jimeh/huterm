+++
id = "macos-quake-departed-witness-activation"
title = "macOS quake smoke departed-witness activation"
status = "open"
kind = "smoke"
platforms = ["macos-arm64"]
retries = 1
review_by = "2026-12-06"

[selector]
steps = ["macos-quake"]

[signature]
match = [
  "(?:external target termination moved focus before hide|default visible endpoint)",
]

[[occurrences]]
date = "2026-09-30"
pr = 194
platform = "macos-arm64"
note = "two consecutive failures on one head; about 1 in 5 hosted jobs from 2026-09-26"

[[occurrences]]
date = "2026-10-30"
local = true
platform = "macos-arm64"
note = "busy shared desktop, animation matrix block"
+++

- **Label:** the departed-witness block in `scripts/check-quake.ts` (about lines
  470 to 480), usually "default visible endpoint" or "external target
  termination moved focus before hide".
- **Rate:** about 1 in 5 hosted macOS smoke jobs since 2026-09-26. Failures and
  passes interleave on the same code, and two consecutive failures on one head
  have occurred without a regression (PR #194, 2026-09-30). It has also failed
  once in the local Tart VM.
- **Evidence:** the final state shows `activation_seen=true`, `active=false`,
  and `current_focus_id` set to the departed witness. SettleVisible reaches
  Idle only after activation is seen, so the quake was active and the newly
  launched witness took activation back. `trace.jsonl` records animation
  samples but not focus, so the reactivation mechanism is unconfirmed.
- **Hypothesis and mitigation (2026-10-02):** the departed witness requested
  activation at launch and again on the `focus` command. Under cooperative
  activation a queued second request may apply after Huterm has seen its own
  activation, and Huterm deliberately does not re-arm activation retries after
  that. The departed witness now launches with `--no-launch-activation`, so the
  awaited `focus` command is its only request. Every witness also logs
  activation requests and active/resign notifications with uptime to
  `witness-events`, and departed-block failures print that log.
- **Other block:** on 2026-10-30 a local host run on a busy shared desktop
  failed the same way inside the animation matrix (`show_quake` then
  `settled(true)`, about line 667), with `activation_seen=true` and
  `active=false`, after "focus restoration failed after hiding: previous
  application disappeared". The immediate rerun on the same head passed.
- **Triage:** if the label recurs, read the "departed witness activation
  events" in the failure. A `became_active` after Huterm summoned the quake
  confirms a late activation; its absence refutes the hypothesis. Remove this
  entry after about ten clean macOS smoke runs, the rate at which it used to
  fail at least once. Otherwise a failure at these labels is this flake; rerun.
  Investigate failures at other quake labels. The hosted runner's work area is 1024x686,
  so the quarter-height quake grid has only 8 rows: a new step that prints
  rows before the geometry section scrolls `READY:` out of view and fails
  "resize replaced retained window or shell" only in CI.
