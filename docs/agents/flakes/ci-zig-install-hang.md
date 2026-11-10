+++
id = "ci-zig-install-hang"
title = "Install Zig hang"
status = "open"
kind = "infra"
platforms = ["macos-arm64", "linux-x86_64", "linux-aarch64"]
retries = 0
review_by = "2027-01-15"

[selector]
step_names = [
  "Install Zig",
  "Retry Zig installation",
  "Install release tools in a clean Mise state",
]

[signature]
match = [
  "(?:has exceeded the maximum execution time|The operation was canceled|has timed out after)",
]

[[occurrences]]
date = "2026-10-28"
url = "https://github.com/jimeh/huterm/actions/runs/40636655413"
platform = "macos-arm64"
note = "Package macOS arm64, about 45 minutes"

[[occurrences]]
date = "2026-11-10"
url = "https://github.com/jimeh/huterm/actions/runs/42829130219/job/129874439249"
sha = "ca88a42e"
platform = "macos-arm64"
note = "Package macOS arm64, release-tools step hit its 8-minute limit on a 26 kB/s Zig mirror"
+++

- **Label:** `Install Zig` runs until the step or job time limit. The
  package jobs' `Install release tools in a clean Mise state` step also
  installs Bun, Syft, uv, and SPDX tools, so a timeout there can come from any
  of them: check which install was still running.
- **Evidence:** in run 40636655413 (2026-10-28), `Package macOS arm64`'s
  `Install Zig` ran from 05:19 until the job stopped at 06:04, about 45
  minutes, and no later step started. Rerunning the failed jobs passed. A
  mirror that sends slowly is limited only by Mise's 30-minute download
  timeout, so the step now has a 4-minute bound and one retry; see
  [the CI guide](../ci.md#jobs-and-caches). In run 42829130219
  (2026-11-10), `Package macOS arm64`'s `Install release tools in a clean
  Mise state` reached its 8-minute limit while Mise downloaded Zig from a
  community mirror at about 26 kB/s, although the Zig download cache had
  hit. Mise reuses a cached archive only for a URL it has recorded, and it
  shuffles the mirrors per install, so CI now installs Zig from ziglang.org
  only and runs each release-tools install in three attempts of three
  minutes. GitHub reports a step limit as "has timed out after"; so does the
  retry helper when its last attempt times out.
- **Triage:** a timeout here is an infrastructure failure; rerun the job.
