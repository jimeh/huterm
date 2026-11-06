+++
id = "linux-integration-held-link-hover"
title = "Linux integration smoke held-link hover timeout"
status = "open"
kind = "smoke"
platforms = ["linux-x86_64", "linux-aarch64"]
retries = 1
review_by = "2026-12-06"

[selector]
steps = ["linux-integration"]

[signature]
match = ["hover https://example\\.test/a_\\(b\\)\\?x=1&y=2"]

[[occurrences]]
date = "2026-09-23"
note = "main"

[[occurrences]]
date = "2026-09-29"
note = "Dependabot"
+++

- **Label:** a timeout at `hover https://example.test/a_(b)?x=1&y=2` inside the
  held-link loop.
- **Occurrences:** 2026-09-23 (main) and 2026-09-29 (Dependabot).
- **Evidence:** an owned link press without a target never triggers another
  lookup, so hover stays empty. PR #203 added `owned_link` and `window_active`
  to the smoke state, a bounded press retry that logs
  `DESKTOP_INTEGRATION ... retry=N`, and a state dump on hover timeouts. The
  retry may only reduce the rate.
- **Gate (2026-11-06):** on Linux, every link hover now waits for
  `window_active=true` before it presses the link modifier, so a modifier
  sent during an X11 focus change cannot silently start no lookup. A
  timeout there means the window lost focus, which the state dump shows.
- **Triage:** read the failure dump. A timeout at "window active before
  hovering" confirms the focus-flicker theory. A `retry=` line means the press
  had no target at press time. No retry line with an emptied `owned_link` means
  the target was lost during the hold, which the retry cannot cover. Check
  `window_active` for the X11 focus-flicker theory.
