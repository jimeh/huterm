+++
id = "linux-overlays-close-tabs-enter"
title = "Linux overlays smoke Enter on \"Close 2 tabs?\" closes no tabs"
status = "fixed"
kind = "smoke"
platforms = ["linux-x86_64", "linux-aarch64"]
retries = 0
review_by = "2026-12-06"

[selector]
steps = ["linux-overlays", "linux-palette"]

[signature]
match = ["w0\\.confirming=true", "w0\\.dialog_focus=primary"]

[[occurrences]]
date = "2026-10-28"
url = "https://github.com/jimeh/huterm/actions/runs/40768430893/job/123099017973"
pr = 318
sha = "86322b08"
platform = "linux-x86_64"

[[occurrences]]
date = "2026-10-28"
url = "https://github.com/jimeh/huterm/actions/runs/40768430893/job/123099018193"
pr = 318
sha = "86322b08"
platform = "linux-aarch64"

[[occurrences]]
date = "2026-10-30"
url = "https://github.com/jimeh/huterm/actions/runs/41002162507/job/123888961905"
pr = 333
sha = "9a732f70"
platform = "linux-aarch64"

[[occurrences]]
date = "2026-10-30"
url = "https://github.com/jimeh/huterm/actions/runs/41000212950/job/123882888482"
pr = 330
sha = "4b7be1f5"
platform = "linux-aarch64"

[[occurrences]]
date = "2026-10-31"
url = "https://github.com/jimeh/huterm/actions/runs/41161296347/job/124419228976"
pr = 353
sha = "d4347ab4"
platform = "linux-x86_64"

[[occurrences]]
date = "2026-10-31"
url = "https://github.com/jimeh/huterm/actions/runs/41176975476/job/124466434006"
pr = 358
sha = "b7aae6e9"
platform = "linux-aarch64"

[[occurrences]]
date = "2026-10-31"
url = "https://github.com/jimeh/huterm/actions/runs/41214596936/job/124576473001"
pr = 363
sha = "9cb9ad94"
platform = "linux-aarch64"
+++

- **Label:** `Smoke palette` or `Smoke Linux overlays` in the Linux Rust
  smoke jobs, at the `close_other_tabs` step of `scripts/check-overlays.ts`:
  timed out waiting for `w0.confirming=false`, `w0.tabs=1`,
  `w0.active_index=0`, and `w0.terminal_focused=true`.
- **Occurrences:** six runs between 2026-10-28 and 2026-10-31, on PRs
  that changed neither the close dialog nor tab closing:
  - PR #318, run 40768430893, jobs [123099017973][overlays-318-x86] and
    [123099018193][overlays-318-arm], head `86322b08`, both architectures.
  - PR #333, run 41002162507, job [123888961905][overlays-333], head
    `9a732f70`, aarch64 only.
  - PR #330, run 41000212950, job [123882888482][overlays-330], head
    `4b7be1f5`, aarch64 only; the rerun passed.
  - PR #353, run 41161296347, job [124419228976][overlays-353], head
    `d4347ab4`, x86_64 only.
  - PR #358, run 41176975476, job [124466434006][overlays-358], head
    `b7aae6e9`, aarch64 only.
  - PR #363, run 41214596936, job [124576473001][overlays-363], head
    `9cb9ad94`, aarch64 only.
- **Evidence correction:** all seven original job logs show the same actual
  state: `w0.confirming=true`, `w0.dialog_focus=primary`, `w0.busy=false`,
  `w0.tabs=3`, `w0.active_index=2`, and `w0.terminal_focused=false`. Earlier
  versions of this entry misread the timeout's expected
  `w0.confirming=false` as the observed state in four runs. The logs do not
  show a dialog closing without closing its tabs.
- **Cause:** the step waited for the confirmation model and title, but not
  for the primary button's focus, unlike the neighboring steps. Since
  migration 7b, focus enters on the dialog's first frame. Enter before that
  focus change presses no button; the dialog later draws with primary focus
  and stays open. `confirming` reports the close model, `dialog_focus`
  reports the button's GPUI focus, and `terminal_focused` reports the active
  terminal's GPUI focus. Missing terminal focus is expected while the dialog
  remains open. No close-dialog product defect was found.
- **Fix:** wait for `w0.dialog_focus=primary` alongside the model and title
  before sending Enter. Keep the final assertion of one tab and restored
  terminal focus.
- **Triage:** record any recurrence with the actual state after `state=`,
  rather than the expected fields in the timeout label. Remove this entry
  after about 20 clean Linux overlays CI runs with the fix.

[overlays-318-x86]: https://github.com/jimeh/huterm/actions/runs/40768430893/job/123099017973
[overlays-318-arm]: https://github.com/jimeh/huterm/actions/runs/40768430893/job/123099018193
[overlays-333]: https://github.com/jimeh/huterm/actions/runs/41002162507/job/123888961905
[overlays-330]: https://github.com/jimeh/huterm/actions/runs/41000212950/job/123882888482
[overlays-353]: https://github.com/jimeh/huterm/actions/runs/41161296347/job/124419228976
[overlays-358]: https://github.com/jimeh/huterm/actions/runs/41176975476/job/124466434006
[overlays-363]: https://github.com/jimeh/huterm/actions/runs/41214596936/job/124576473001
