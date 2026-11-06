+++
id = "release-stapler-cloudkit-lookup"
title = "Release stapler CloudKit ticket lookup"
status = "open"
kind = "infra"
platforms = ["macos-arm64"]
retries = 0
review_by = "2027-01-15"

[selector]
step_names = ["Build, sign, notarize, and inventory universal app"]

[signature]
match = ["xcrun exited with status 68"]

[[occurrences]]
date = "2026-10-23"
url = "https://github.com/jimeh/huterm/actions/runs/40034372242"
sha = "be18c69e"
platform = "macos-arm64"
note = "v0.18.0"

[[occurrences]]
date = "2026-10-25"
url = "https://github.com/jimeh/huterm/actions/runs/40218061074"
sha = "99a5ea15"
platform = "macos-arm64"
note = "v0.19.0"

[[occurrences]]
date = "2026-10-27"
url = "https://github.com/jimeh/huterm/actions/runs/40487164468"
sha = "9782d235"
platform = "macos-arm64"
note = "v0.21.0"
+++

- **Label:** `Build signed macOS package`, step "Build, sign, notarize, and
  inventory universal app", ending in `xcrun exited with status 68` after
  `Error Domain=NSURLErrorDomain Code=-1005 "The network connection was
  lost."` against the
  `com.apple.gk.ticket-delivery/production/public/records/lookup` CloudKit URL.
- **Occurrences:** the first attempt of three of five releases, each passing on
  rerun: v0.18.0 (run 40034372242) and v0.19.0 (run 40218061074), where
  `stapler staple` reported "The staple and validate action failed! Error 68.",
  and v0.21.0 (run 40487164468), where `stapler staple` worked and the
  following `stapler validate` reported "The validate action failed! Error 68."
  Notarization had been accepted each time.
- **Mitigation (2026-10-27):** `scripts/release-macos.ts` retries
  `stapler staple`, `stapler validate`, and `spctl --assess` on any failure, up
  to five attempts with 15, 30, 45, and 60 second waits. Each failed attempt
  logs `<step> failed on attempt N of 5: <exit status>: <reason>`, where the
  reason is the NSError domain and code or the tool's last line.
- **Triage:** a `retrying in` line before a successful step shows the retry
  absorbed the drop. A `giving up` line means five attempts over about two and
  a half minutes failed. Read the reasons: the same network error each time
  points at an Apple outage, so check Apple's developer system status and
  rerun the job. Stapler's Error 65 means it found no ticket; right after an
  accepted notarization that is usually propagation lag, so rerun once before
  investigating the notarization or signature. Remove this entry once about
  ten releases pass without a `giving up` line.
