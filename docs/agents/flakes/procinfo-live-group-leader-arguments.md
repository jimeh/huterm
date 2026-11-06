+++
id = "procinfo-live-group-leader-arguments"
title = "huterm-procinfo live group leader arguments on Linux"
status = "fixed"
kind = "rust"
platforms = ["linux-x86_64", "linux-aarch64"]
retries = 0
review_by = "2026-12-06"

[selector]
binary = "huterm-procinfo"
tests = ["tests::reads_a_live_process_group_leader_and_its_arguments"]

[signature]
match = [
  "(?:condition never held; last observed|called `Option::unwrap\\(\\)` on a `None` value)",
]

[[occurrences]]
date = "2026-10-21"
url = "https://github.com/jimeh/huterm/actions/runs/39695920472"
pr = 226
platform = "linux-x86_64"

[[occurrences]]
date = "2026-10-25"
url = "https://github.com/jimeh/huterm/actions/runs/40219352235"
platform = "linux-aarch64"
note = "Release Please"

[[occurrences]]
date = "2026-10-26"
url = "https://github.com/jimeh/huterm/actions/runs/40266500383"
platform = "linux-aarch64"
note = "Dependabot"

[[occurrences]]
date = "2026-10-26"
url = "https://github.com/jimeh/huterm/actions/runs/40320456365"
platform = "linux-aarch64"
note = "main"
+++

- **Label:**
  `tests::reads_a_live_process_group_leader_and_its_arguments` in
  `Checks (Linux x86_64)` or `Checks (Linux aarch64)`, panicking at
  `arguments(pid).unwrap()` with "called `Option::unwrap()` on a `None`
  value". After the fix, a failure there times out with "condition never
  held; last observed" and the process facts and arguments it last read.
- **Occurrences:** four, each at `lib.rs:262:35`: 2026-10-21 on x86_64
  (PR #226, run 39695920472), then on aarch64 on 2026-10-25 (Release Please,
  run 40219352235) and 2026-10-26 (Dependabot run 40266500383, and main run
  40320456365). Reruns passed.
- **Cause:** Linux renames the process `sleep` before the exec records where
  its arguments are, and `cmdline` reads as empty until then. The public
  `arguments` turns an empty list into `None`. Reading `cmdline` the moment
  `stat` first named `sleep` returned nothing in 494 of 500 tries in Docker
  on arm64. The test now waits for the name and the arguments in one poll.
  With that poll running without a pause, the old test failed 174 of 200
  runs and the new one passed 200 of 200.
- **Triage:** a failure at this test after the fix landed is new; read the
  last observed facts and arguments in its panic before rerunning. Remove
  this entry once the fixed test has passed in about 20 Linux check jobs.
