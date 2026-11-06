+++
id = "ci-mise-github-api-403"
title = "GitHub API 403 during CI tool installation"
status = "open"
kind = "infra"
platforms = ["linux-x86_64", "linux-aarch64", "macos-arm64"]
retries = 0
review_by = "2027-01-15"

[selector]
step_names = ["Install job tools with Mise"]

[signature]
match = ["(?:403 Forbidden|API rate limit exceeded)"]
+++

- **Label:** `Install job tools with Mise` fails on a GitHub API 403.
- **Evidence:** an unpinned Mise ran `mise self-update`, whose GitHub API
  lookup was rate limited. Every `jdx/mise-action` step now pins one Mise
  version; see [the CI guide](../ci.md#toolchain-bootstrap).
- **Triage:** treat a single 403 as an infrastructure failure and rerun
  the job. Check that every Mise action step still pins `version`.
