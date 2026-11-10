+++
id = "ci-zig-mirror-rate-limit"
title = "Zig mirror rate limit during CI tool installation"
status = "open"
kind = "infra"
platforms = ["linux-x86_64", "linux-aarch64", "macos-arm64"]
retries = 0
review_by = "2027-01-15"

[selector]
step_names = [
  "Install Zig",
  "Retry Zig installation",
  "Install job tools with Mise",
  "Install macOS release tools with Mise",
  "Install Linux release tools with Mise",
  "Install release tools in a clean Mise state",
]

[signature]
match = ["(?:HTTP status client error \\(429|429 Too Many Requests)"]

[[occurrences]]
date = "2026-10-01"
url = "https://github.com/jimeh/huterm/actions/runs/36940033982"
sha = "fad1780e"
platform = "linux-x86_64"
note = "Release v0.16.0, Build Linux x86_64 package, Mise 2026.9.14"
+++

- **Label:** `Install Zig`, `Retry Zig installation`, or an older `Install job
  tools with Mise` step fails on a Zig community-mirror rate limit (HTTP
  429). The package jobs' `Install release tools in a clean Mise state` step
  installs other tools too, so check that the 429 came from a Zig mirror.
- **Evidence:** the bootstrap and its limits are in
  [the CI guide](../ci.md#jobs-and-caches). Repeated failures usually follow
  Actions cache eviction: the repository has exceeded GitHub's 10 GB cache
  limit before, which forced cold installs.
- **Release:** release jobs install Zig in their Mise action step, with no
  time bound or retry of their own. The v0.16.0 Release failed this way on
  Mise 2026.9.14, which asked only the serving mirror for the signature, and
  passed on rerun. No Release has failed on Zig since Mise 2026.10.7 added
  mirror fallback.
- **Triage:** treat a single 429 as an infrastructure failure and rerun
  the job.
