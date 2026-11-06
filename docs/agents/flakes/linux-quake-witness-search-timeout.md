+++
id = "linux-quake-witness-search-timeout"
title = "Linux quake smoke external witness search timeout"
status = "open"
kind = "smoke"
platforms = ["linux-x86_64", "linux-aarch64"]
retries = 1
review_by = "2026-12-06"

[selector]
steps = ["linux-quake"]

[signature]
match = [
  "xdotool search --sync --name \\^Quake external focus witness\\$:",
]

[[occurrences]]
date = "2026-11-03"
url = "https://github.com/jimeh/huterm/actions/runs/41738420581/job/126274168984"
pr = 384
sha = "229a4974"
platform = "linux-x86_64"
+++

- **Label:** `Smoke Linux quake windows` in `Rust smoke (Linux x86_64)` fails
  with `error: xdotool search --sync --name ^Quake external focus witness$:`
  and no stderr after the colon, thrown by `run()` from the Linux branch of
  the witness setup in `scripts/check-quake.ts` (about line 257).
- **Occurrences:** once, on 2026-11-03 (PR #384 at `229a4974`, run
  41738420581, attempt 1, job 126274168984). The PR does not touch quake code.
  The same step passed on aarch64 in that run and on both architectures in the
  PR's earlier runs, and the rerun of the same job passed.
- **Evidence:** `run()` gives every command a 5-second `spawnSync` timeout.
  `xdotool search --sync` waits until a matching window exists, so a witness
  that is slow to map is killed at the deadline, which leaves stderr empty and
  matches the log. The failure came about 5.4 seconds after the step started,
  and the retained state shows Huterm's own window ready (`READY:` text,
  focused, `command_sequence=0`), so Huterm had started and the harness was
  still waiting for the `xmessage` witness. Whether `xmessage` was slow to map
  or exited early is unconfirmed: the harness pipes its stderr but does not
  print it or its exit status on failure.
- **Triage:** a failure at this label with empty stderr is this flake; rerun.
  If it recurs, make the failure print the witness's exit status and stderr
  before changing the timeout, so the next occurrence shows which cause it is.
