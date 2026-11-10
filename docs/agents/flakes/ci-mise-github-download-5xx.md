+++
id = "ci-mise-github-download-5xx"
title = "GitHub release download 5xx during tool installation"
status = "open"
kind = "infra"
platforms = ["linux-x86_64", "linux-aarch64", "macos-arm64"]
retries = 0
review_by = "2027-01-15"

[selector]
step_names = [
  "Install job tools with Mise",
  "Install macOS package tools with Mise",
  "Install Linux package tools with Mise",
  "Install preflight tools with Mise",
  "Install macOS release tools with Mise",
  "Install Linux release tools with Mise",
  "Install assembly tools with Mise",
  "Install release tools in a clean Mise state",
  "Prepare and verify exact release candidate",
]

[signature]
match = ["HTTP status server error \\(5\\d\\d [^)]*\\) for url \\(https://github\\.com/"]

[[occurrences]]
date = "2026-11-05"
url = "https://github.com/jimeh/huterm/actions/runs/41999118741"
platform = "macos-arm64"
note = "Checks (macOS arm64), bun@1.4.4 504"

[[occurrences]]
date = "2026-11-05"
url = "https://github.com/jimeh/huterm/actions/runs/41999189761"
platform = "macos-arm64"
note = "Package macOS arm64, bun@1.4.4 504, same minute"
+++

- **Label:** a Mise install step fails with `Failed to install <tool>:
  HTTP status server error (5xx …) for url (https://github.com/…)` while
  downloading a release asset.
- **Evidence:** on 2026-11-05 at 09:03 UTC, two macOS jobs of the same
  release-please branch push failed within a minute on a 504 from
  `github.com/oven-sh/bun/releases/download/…`. Mise retried each fetch three
  times within about 15 seconds before giving up. Rerunning the failed jobs
  passed. No Release run has failed this way.
- **Triage:** treat a 5xx from GitHub's download host as an infrastructure
  failure and rerun the job, after checking GitHub's status page. A
  `requires configured install dependency` error in the same step is a
  configuration error, not this flake; see
  [the CI guide](../ci.md#toolchain-bootstrap).
