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

- **Label:** the opacity block in `scripts/check-quake.ts` (about line 415).
  The quake is visible from the tab-removal block. After a reload to
  `animation = "fade"`, `animation_ms = 1000`, and `[window] opacity = 0.7`,
  the script sends `default toggle_quake`, which hides it, and waits for an
  `Animate` stage with a native alpha between 0.1 and 0.6.
- **Evidence (2026-11-13):** the toggle returned `Ok(Accepted)`. The final
  state shows the default profile `stage=Idle`, `desired=false`,
  `visible=false`, and native alpha 0.7. In `trace.jsonl`, generation 8 is
  the earlier `show_quake`, and no command between it and the toggle hides
  the quake, so generation 9, the last one, is most likely the toggle's hide.
  It ran as a slide of about 150 ms (frame moving, first sample already at
  progress 0.38), like the pre-reload `animation_ms = 150` hides, not as a
  1000 ms fade, so native alpha never left 0.7. The configured opacity check
  just before the toggle passed. The retained evidence is the run's
  `smoke-evidence-macOS-ARM64-1` artifact.
- **Hypothesis (unconfirmed):** the reload reported `reloading=false` and
  applied the window opacity before the quake profile's animation settings
  took effect, so the hide used the previous slide. The trace has no command
  timestamps, so the link between generation 9 and the toggle is inferred.
- **Not the departed-witness flake:** the
  [departed-witness entry](macos-quake-departed-witness-activation.md) fails
  at a different label, with the quake active and focus taken back by a
  witness. Here the hide completed.
- **Triage:** if it recurs, check the animation of the trace's last
  generation. A short slide after the fade reload supports the hypothesis; a
  fade that never reached the alpha window points at the fade itself, and no
  new generation after the toggle points at a dropped toggle.
