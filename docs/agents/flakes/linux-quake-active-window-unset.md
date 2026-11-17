+++
id = "linux-quake-active-window-unset"
title = "Linux quake smoke reads no active window"
status = "fixed"
kind = "smoke"
platforms = ["linux-x86_64", "linux-aarch64"]
retries = 0
review_by = "2026-12-17"

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

[[occurrences]]
date = "2026-11-07"
url = "https://github.com/jimeh/huterm/actions/runs/42368662592/job/128392024324"
pr = 433
sha = "64f082c6"
platform = "linux-aarch64"
retried = false
note = "not retried: this entry did not exist yet"

[[occurrences]]
date = "2026-11-07"
url = "https://github.com/jimeh/huterm/actions/runs/42326484515/job/128262855197"
pr = 432
sha = "c33a8541"
platform = "linux-aarch64"
retried = false
note = "not retried: this entry did not exist yet"

[[occurrences]]
date = "2026-11-09"
url = "https://github.com/jimeh/huterm/actions/runs/42701070619/job/129445619487"
pr = 451
sha = "165b64b7"
platform = "linux-aarch64"
retried = false
note = "not retried: this entry did not exist yet"

[[occurrences]]
date = "2026-11-10"
url = "https://github.com/jimeh/huterm/actions/runs/42828749907/job/129872874604"
pr = 460
sha = "3624c477"
platform = "linux-x86_64"
retried = true

[[occurrences]]
date = "2026-11-14"
url = "https://github.com/jimeh/huterm/actions/runs/43595080438/job/132447754862"
pr = 477
sha = "a72b3c5b"
platform = "linux-x86_64"
retried = true

[[occurrences]]
date = "2026-11-16"
url = "https://github.com/jimeh/huterm/actions/runs/43843512672/job/133217826505"
pr = 476
sha = "321900ff"
platform = "linux-aarch64"
retried = true

[[occurrences]]
date = "2026-11-16"
url = "https://github.com/jimeh/huterm/actions/runs/43855935665/job/133260072797"
pr = 473
sha = "2089962b"
platform = "linux-aarch64"
retried = true

[[occurrences]]
date = "2026-11-16"
url = "https://github.com/jimeh/huterm/actions/runs/43981933055/job/133700320239"
pr = 484
sha = "d8bfaeed"
platform = "linux-x86_64"
retried = true
+++

- **Label:** `Smoke Linux quake windows` in `Rust smoke (Linux x86_64)` or
  `Rust smoke (Linux aarch64)` fails with `error: xdotool getactivewindow:
  XGetWindowProperty[_NET_ACTIVE_WINDOW] failed (code=1)`, thrown by `run()`
  from `witnessActive` while `waitFor(witnessActive, ...)` polls right after
  `focusWitness()` activates the external witness.
- **Occurrences:** ten in CI from 2026-11-07 to 2026-11-16, five hard
  failures and five retried passes, on both architectures and on main as
  well as pull requests. They came from four waits in
  `scripts/check-quake.ts`: "external witness focus after sibling hide",
  "external focus before unchanged fullscreen toggle", "external focus below
  quake", and "external focus before show attempt 1", which follows the
  reporter checks. The three hard failures from 2026-11-07 to 2026-11-09
  predate this entry and were recorded later.
- **Cause:** `xdotool getactivewindow` prints exactly this error, and exits
  1, when the root's `_NET_ACTIVE_WINDOW` is None or absent. Under Openbox,
  when `xdotool windowactivate` moves focus from a Huterm window to the
  witness, the property becomes None for a moment before it names the
  witness. An `xprop -spy` on it during Docker quake smokes (arm64) saw
  None 49 to 67 times per run, including 2 to 6 ms after nearly every
  `focusWitness()`. The
  first `waitFor` sample comes right after `windowactivate` returns, and
  `run()` turned that sample's non-zero exit into a throw instead of another
  poll.
- **Fix (2026-11-17):** `witnessActive` reads the active window through
  `activeWindowFrom`, which returns no window for that one failure and
  throws on any other, so `waitFor` polls on until its deadline. The
  one-shot "show stole focus" check polls up to 2 seconds for a definite
  window. In Docker on arm64, 12 fixed runs passed, one of them after
  reading an unset property inside a wait. The original script did not hit
  the error in 13 runs, even polling without a pause: the spy measured a
  median None period of 0.2 ms, shorter than one `xdotool` call. CI hit it
  in 10 of roughly 350 Linux smoke jobs from 2026-11-07 to 2026-11-16.
- **Triage:** a match now means a script reads `getactivewindow` through
  `run()` again, so the error is the old throw, not a focus problem. A
  witness that never becomes active fails at its wait's timeout, and a
  property that stays unset fails with `_NET_ACTIVE_WINDOW stayed unset`;
  this signature matches neither.
