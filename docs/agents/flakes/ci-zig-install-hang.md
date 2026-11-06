+++
id = "ci-zig-install-hang"
title = "Install Zig hang"
status = "open"
kind = "infra"
platforms = ["macos-arm64", "linux-x86_64", "linux-aarch64"]
retries = 0
review_by = "2027-01-15"

[selector]
step_names = ["Install Zig", "Retry Zig installation"]

[signature]
match = [
  "(?:has exceeded the maximum execution time|The operation was canceled)",
]

[[occurrences]]
date = "2026-10-28"
url = "https://github.com/jimeh/huterm/actions/runs/40636655413"
platform = "macos-arm64"
note = "Package macOS arm64, about 45 minutes"
+++

- **Label:** `Install Zig` runs until the step or job time limit.
- **Evidence:** in run 40636655413 (2026-10-28), `Package macOS arm64`'s
  `Install Zig` ran from 05:19 until the job stopped at 06:04, about 45
  minutes, and no later step started. Rerunning the failed jobs passed. A
  mirror that sends slowly is limited only by Mise's 30-minute download
  timeout, so the step now has a 4-minute bound and one retry; see
  [the CI guide](../ci.md#jobs-and-caches).
- **Triage:** a timeout here is an infrastructure failure; rerun the job.
