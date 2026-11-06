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
]

[signature]
match = ["(?:HTTP status client error \\(429|429 Too Many Requests)"]
+++

- **Label:** `Install Zig`, `Retry Zig installation`, or an older `Install job
  tools with Mise` step fails on a Zig community-mirror rate limit (HTTP
  429).
- **Evidence:** the bootstrap and its limits are in
  [the CI guide](../ci.md#jobs-and-caches). Repeated failures usually follow
  Actions cache eviction: the repository has exceeded GitHub's 10 GB cache
  limit before, which forced cold installs.
- **Triage:** treat a single 429 as an infrastructure failure and rerun
  the job.
