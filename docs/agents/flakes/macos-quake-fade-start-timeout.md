+++
id = "macos-quake-fade-start-timeout"
title = "macOS quake smoke normalized fade never starts"
status = "open"
kind = "smoke"
platforms = ["macos-arm64"]
retries = 0
review_by = "2026-12-14"

[selector]
steps = ["macos-quake"]

[signature]
match = [
  "timed out waiting for quake normalized fade starts",
]

[[occurrences]]
date = "2026-11-13"
url = "https://github.com/jimeh/huterm/actions/runs/43504337503/job/132157942504"
sha = "40987b7e3a040e6db64115cb7219b41e9c54e6c1"
platform = "macos-arm64"
note = "main push, attempt 1; the rerun passed"
+++

- **Label:** the opacity block in `scripts/check-quake.ts` (about line 415):
  after a reload to `animation = "fade"` and `[window] opacity = 0.7`, the
  script sends `default toggle_quake` and waits for an `Animate` stage with a
  native alpha between 0.1 and 0.6.
- **Evidence (2026-11-13):** the toggle returned `Ok(Accepted)`, but the
  final state shows the default profile `stage=Idle`, `desired=false`,
  `visible=false`, and native alpha 0.7, so the window never started to show.
  `trace.jsonl`, which records every transition, ends at generation 9, a
  hide that settled before the toggle; no later generation appears. The
  configured opacity check just before the toggle passed. The retained
  evidence is the run's `smoke-evidence-macOS-ARM64-1` artifact.
- **Not the departed-witness flake:** the
  [departed-witness entry](macos-quake-departed-witness-activation.md) fails
  at a different label, with the quake active and focus taken back by a
  witness. Here the quake never left `Idle`.
- **Triage:** if it recurs, compare the trace's last generation with the
  command sequence. A missing generation after an accepted toggle points at
  the toggle being dropped while a reload or the preceding hide settles; a
  new generation that never reaches `Animate` points at the fade itself.
