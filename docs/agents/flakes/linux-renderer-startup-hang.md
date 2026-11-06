+++
id = "linux-renderer-startup-hang"
title = "Linux renderer smoke startup hang"
status = "open"
kind = "smoke"
platforms = ["linux-x86_64", "linux-aarch64"]
retries = 1
review_by = "2026-12-06"

[selector]
steps = ["renderer"]

[signature]
match = ["renderer smoke timed out after \\d+ms"]
reject = ["RENDERER_SMOKE prepared"]

[[occurrences]]
date = "2026-09-27"
pr = 175
platform = "linux-x86_64"

[[occurrences]]
date = "2026-09-30"
pr = 196
platform = "linux-x86_64"

[[occurrences]]
date = "2026-10-25"
url = "https://github.com/jimeh/huterm/actions/runs/40211388780"
sha = "792f4f00"
platform = "linux-x86_64"

[[occurrences]]
date = "2026-10-26"
url = "https://github.com/jimeh/huterm/actions/runs/40346320545"
sha = "ea43dab1"
platform = "linux-aarch64"

[[occurrences]]
date = "2026-10-27"
url = "https://github.com/jimeh/huterm/actions/runs/40495981661/job/122161417130"
pr = 270
platform = "linux-x86_64"

[[occurrences]]
date = "2026-10-27"
url = "https://github.com/jimeh/huterm/actions/runs/40576556557/job/122444431856"
pr = 279
platform = "linux-x86_64"

[[occurrences]]
date = "2026-10-28"
url = "https://github.com/jimeh/huterm/actions/runs/40693317698/job/122833434701"
pr = 311
sha = "465f1dbc"
platform = "linux-x86_64"

[[occurrences]]
date = "2026-10-28"
url = "https://github.com/jimeh/huterm/actions/runs/40783118712/job/123149517765"
pr = 320
sha = "667367e6"
platform = "linux-x86_64"

[[occurrences]]
date = "2026-11-04"
url = "https://github.com/jimeh/huterm/actions/runs/41788505093/job/126436114743"
pr = 388
sha = "f52c8334"
platform = "linux-aarch64"

[[occurrences]]
date = "2026-11-05"
url = "https://github.com/jimeh/huterm/actions/runs/42099318070/job/127498303513"
pr = 412
sha = "766d4bd9"
platform = "linux-x86_64"
+++

- **Label:** `Smoke renderer` in `Rust smoke (Linux x86_64)` or
  `Rust smoke (Linux aarch64)` times out at 15 seconds with empty stderr and
  no `RENDERER_SMOKE prepared` line.
- **Occurrences:** 2026-09-27 (PR #175) and 2026-09-30 (PR #196) on x86_64,
  then on main: 2026-10-25 on x86_64 (run 40211388780) and 2026-10-26 on
  aarch64 (run 40346320545), then 2026-10-27 on x86_64 (PR #270, run
  40495981661, job 122161417130), and again on x86_64 the same day (PR
  #279, run 40576556557, job 122444431856), and 2026-10-28 on x86_64 (PR
  #311 at `465f1dbc`, run 40693317698, job 122833434701), and again
  that day (PR #320 at `667367e6`, run 40783118712, job 123149517765),
  and 2026-11-04 on aarch64 (PR #388 at `f52c8334`, run 41788505093, job
  126436114743). Every other Linux smoke in those jobs passed, and reruns
  passed.
- **Evidence:** the smoke prints its first `RENDERER_SMOKE prepared` line
  during the first prepare, and Rust line-buffers stdout even into a pipe,
  so an empty stdout means the process hung before its first prepare. It
  did not reproduce in 460 local runs under Xvfb in Docker on arm64,
  including 400 pinned to four cores with eight running at once. Since
  2026-10-26 the smoke prints `RENDERER_SMOKE launching` before creating
  the application and `application started` inside it; `prepared` lines
  come from the first draw inside `open_window`, and `window opened` after
  it returns. At its deadline the smoke supervisor also records a `threads`
  event in `events.jsonl` with each thread's name, state, kernel wait
  channel, and current syscall.
- **First thread dump:** the 2026-10-27 run stopped after
  `application started` at 2.7 seconds, so it hung between application
  startup and the first draw. At the deadline the main thread was in state
  `D` in `folio_wait_bit_common`, the kernel's wait on a page-cache folio's
  lock or writeback bit, with syscall `-1`, meaning it was not in a system
  call. That combination suggests a page fault waiting on page I/O, but the
  dump does not show the caller. The other 21 threads, including GPUI's
  workers, four `llvmpipe` threads, and Mesa's `rendere:disk$0` shader-cache
  thread, were idle in futex, epoll, or poll waits.
- **Second thread dump:** the PR #279 run got further. By the job log's
  timestamps, measured from `RENDERER_SMOKE launching`, which the smoke
  prints as the supervised process starts: `application started` came at
  0.2 seconds, the three `prepared` lines at 14.3, and `window opened` at
  14.3. The supervisor's SIGTERM came at 15.03 seconds (`elapsedMs` 15027 in
  `outcome.json`), before any paint was reported.
  This time the main thread was idle in a futex wait. The thread in state `D`
  in `folio_wait_bit_common`, with syscall `-1`, was an unnamed
  `renderer_smoke` thread started after the GPUI workers and before the
  `llvmpipe` threads. Both dumps point at a page-fault stall on page I/O
  while Mesa starts, not at a deadlock in Huterm code.
- **Third thread dump:** the PR #311 run matched the first: stdout stopped
  after `application started`, stderr was empty, and the supervisor's
  SIGTERM came at 15.02 seconds. At the 15.01-second `threads` event the
  main thread (its thread ID equal to the process ID) was in state `D` in
  `folio_wait_bit_common` with syscall `-1`, and the other 20 threads were
  sleeping (state `S`).
- **Fourth thread dump:** the PR #320 run also stopped after
  `application started`, with empty stderr and SIGTERM at 15.02 seconds.
  At the 15.01-second `threads` event the main thread was running (state
  `R`, syscall `running`), not waiting on page I/O. Mesa's
  `WSI swapchain` threads existed, so the window's surface was up, and the
  other 23 threads were sleeping. Mesa's llvmpipe compiles shaders on the
  calling thread, so this fits a first draw still compiling at the
  deadline, not a deadlock.
- **Fifth thread dump:** the PR #388 run matched the first and third:
  stdout stopped after `application started`, stderr was empty, and the
  supervisor's SIGTERM came at 15.02 seconds. At the 15.01-second `threads`
  event the main `renderer_smoke` thread (its thread ID equal to the process
  ID) was in state `D` in `folio_wait_bit_common` with syscall `-1`. The
  rerun passed. This is the first occurrence on aarch64 since 2026-10-26.
- **Sixth thread dump:** on 2026-11-05 on x86_64 (PR #412 at `766d4bd9`,
  run 42099318070, job 127498303513), which changes terminal selection and
  not the renderer, stdout again stopped after `application started`, with
  empty stderr and SIGTERM at 15.02 seconds. At the 15.01-second `threads`
  event the main thread (the lowest thread ID) was in state `D` in
  `folio_wait_bit_common` with syscall `-1`, and the other 11 threads were
  sleeping.
- **Mitigation (2026-11-06):** five of the six dumps show a thread waiting
  on a page-cache folio, which fits a page fault on freshly linked binary
  pages still under writeback. Before the timed smoke, `check-renderer.ts`
  now runs `sync` and reads the smoke binary and Mesa's llvmpipe and LLVM
  libraries into the page cache, printing a `RENDERER_WARMUP` line with the
  time each part took. The fixed 15-second deadline became a progress
  watchdog: the smoke fails after 15 seconds without a new `RENDERER_SMOKE`
  line, or after 60 seconds in total. At either deadline the supervisor also
  records an `io` event with each process's `/proc/<pid>/io` and the
  system's dirty, writeback, and major-fault counters. The fourth dump's
  running main thread is not covered by the warm-up; the watchdog gives it
  time as long as markers keep arriving.
- **Triage:** CI reruns this signature once. Download the
  `smoke-evidence-Linux-*` artifact and read `renderer-process/stdout.log`
  for the last marker, and `renderer-process/events.jsonl` for the `stall`
  or `deadline` event and the `threads` and `io` events after it. Compare
  the `RENDERER_WARMUP` line in the step log with a passing run. Record the
  occurrence with `mise run flakes:record`.
