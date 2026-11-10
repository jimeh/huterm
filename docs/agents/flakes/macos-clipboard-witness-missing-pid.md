+++
id = "macos-clipboard-witness-missing-pid"
title = "macOS clipboard smoke witness loses the app's pid"
status = "open"
kind = "smoke"
platforms = ["macos-arm64"]
retries = 0
review_by = "2026-12-10"

[selector]
steps = ["macos-clipboard"]

[signature]
match = [
  "clipboard-witness visible \\d+ failed: clipboard-witness: no running application has pid",
]

[[occurrences]]
date = "2026-11-10"
url = "https://github.com/jimeh/huterm/actions/runs/42730597925/job/129547489932"
pr = 453
sha = "cc1a880064001aad2936b481cb81b541e411f8d9"
platform = "macos-arm64"
note = "first sighting; PR changed only toml crates, Lucide SVG headers, and action pins"
+++

- **Label:** `checkDirect` in `scripts/check-clipboard.ts` (about line 429),
  the `visible` witness call after the Ghostty engine's `copy-then-hide` check
  and the `show` command.
- **Evidence:** the `hidden` witness call on the same pid passed less than a
  second earlier, and `copy-then-hide` reported its exact bytes. The app
  answered the `show` control command, and the job log has no panic, crash, or
  signal. `NSRunningApplication(processIdentifier:)` then returned nil in
  `runningApplication` in `scripts/clipboard-witness.swift`.
- **Rate:** one failure in the 40 most recent CI runs across branches, on
  2026-11-10.
- **Triage:** retries stay at 0 until it recurs. On a second sighting, compare
  the witness's lookup with a `kill(pid, 0)` liveness check: a live process
  without an `NSRunningApplication` points at a LaunchServices lookup race,
  which a short retry in `runningApplication` would cover; a dead process is a
  real regression in hide and show.
