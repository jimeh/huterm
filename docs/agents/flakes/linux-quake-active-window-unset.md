+++
id = "linux-quake-active-window-unset"
title = "Linux quake smoke reads no active window"
status = "open"
kind = "smoke"
platforms = ["linux-x86_64", "linux-aarch64"]
retries = 1
review_by = "2027-01-15"

[selector]
steps = ["linux-quake"]

[signature]
match = ["xdotool getactivewindow: XGetWindowProperty\\[_NET_ACTIVE_WINDOW\\] failed"]

[[occurrences]]
date = "2026-11-10"
url = "https://github.com/jimeh/huterm/actions/runs/42738570126/job/129573879114"
pr = 456
sha = "4da66842"
platform = "linux-x86_64"
retried = false

[[occurrences]]
date = "2026-11-10"
url = "https://github.com/jimeh/huterm/actions/runs/42824945353/job/129859943389"
pr = 461
sha = "4404e217"
platform = "linux-aarch64"
retried = false
note = "not retried: the entry listed only linux-x86_64"
+++

- **Label:** `Smoke Linux quake windows` in `Rust smoke (Linux x86_64)` or
  `Rust smoke (Linux aarch64)` fails
  with `error: xdotool getactivewindow: XGetWindowProperty[_NET_ACTIVE_WINDOW]
  failed (code=1)`, thrown by `run()` from `witnessActive` while
  `waitFor(witnessActive, "external focus before show attempt 1")` polls, in
  the focus-during-show phase of `scripts/check-quake.ts` (about line 641).
- **Occurrences:** twice, both on 2026-11-10. First on Linux x86_64 (PR #456
  at `4da66842`, run 42738570126, job 129573879114), then on Linux aarch64
  (PR #461 at `4404e217`, run 42824945353, job 129859943389), from the same
  `witnessActive` poll at `scripts/check-quake.ts:641`. PR #461 changes only
  macOS title-strip layout. The PR changes drag code only. The same
  step passed on the PR's previous head `8aee0873` (run 42737671075), whose
  Linux build is the same: the one change between them is a `cfg` gate on a
  method Linux does not compile.
- **Evidence:** every earlier quake phase in the step passed, including the
  focus, reporter, and departed-witness checks just before it. `xdotool
  getactivewindow` exits non-zero when the window manager has no active
  window set, as it can for a moment after focus leaves a window that
  unmapped. `run()` turns that exit into a throw, so `waitFor` stops at the
  first such sample instead of polling again. Whether the property was
  briefly unset or the call raced the window manager is unconfirmed.
- **Triage:** a likely fix is for `witnessActive` to treat a failed
  `getactivewindow` as "not active yet" and keep polling under `waitFor`'s
  deadline. Record further occurrences here until then.
