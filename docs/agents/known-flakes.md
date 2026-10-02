# Known flakes

Intermittent failures whose cause is unconfirmed or whose fix is deferred. Check
this list before treating a red check as a regression, and compare against
main's recent runs for the same job. Add an entry when a failure recurs without
a code cause. Update it when evidence changes, and delete it once a fix lands
and the failure stops.

Each entry names the failing label, the evidence so far, and how to triage it.

## `smoke:macos-quake` departed-witness activation

- **Label:** the departed-witness block in `scripts/check-quake.ts` (about lines
  470 to 480), usually "default visible endpoint" or "external target
  termination moved focus before hide".
- **Rate:** about 1 in 5 hosted macOS smoke jobs since 2026-09-26. Failures and
  passes interleave on the same code, and two consecutive failures on one head
  have occurred without a regression (PR #194, 2026-09-30). It has also failed
  once in the local Tart VM.
- **Evidence:** the final state shows `activation_seen=true`, `active=false`,
  and `current_focus_id` set to the departed witness. SettleVisible reaches
  Idle only after activation is seen, so the quake was active and the newly
  launched witness took activation back. `trace.jsonl` records animation
  samples but not focus, so the reactivation mechanism is unconfirmed.
- **Hypothesis and mitigation (2026-10-02):** the departed witness requested
  activation at launch and again on the `focus` command. Under cooperative
  activation a queued second request may apply after Huterm has seen its own
  activation, and Huterm deliberately does not re-arm activation retries after
  that. The departed witness now launches with `--no-launch-activation`, so the
  awaited `focus` command is its only request. Every witness also logs
  activation requests and active/resign notifications with uptime to
  `witness-events`, and departed-block failures print that log.
- **Triage:** if the label recurs, read the "departed witness activation
  events" in the failure. A `became_active` after Huterm summoned the quake
  confirms a late activation; its absence refutes the hypothesis. Remove this
  entry after about ten clean macOS smoke runs, the rate at which it used to
  fail at least once. Otherwise a failure at these labels is this flake; rerun.
  Investigate failures at other quake labels. The hosted runner's work area is 1024x686,
  so the quarter-height quake grid has only 8 rows: a new step that prints
  rows before the geometry section scrolls `READY:` out of view and fails
  "resize replaced retained window or shell" only in CI.

## Linux renderer smoke startup hang

- **Label:** `Smoke renderer` in `Rust smoke (Linux x86_64)` times out at 15
  seconds with empty stdout and stderr and no `RENDERER_SMOKE` marker.
- **Occurrences:** 2026-09-27 (PR #175) and 2026-09-30 (PR #196), unrelated
  PRs, every other Linux smoke in the job passing. Reruns passed.
- **Triage:** download the `smoke-evidence-Linux-X64-*` artifact and read
  `renderer-process/events.jsonl` and stdout. No markers means the process hung
  before its first paint under Xvfb; rerun once. A failure with markers, or a
  repeat on the same head, needs investigation of Linux event-loop startup.

## `smoke:linux-integration` held-link hover timeout

- **Label:** a timeout at `hover https://example.test/a_(b)?x=1&y=2` inside the
  held-link loop.
- **Occurrences:** 2026-09-23 (main) and 2026-09-29 (Dependabot).
- **Evidence:** an owned link press without a target never triggers another
  lookup, so hover stays empty. PR #203 added `owned_link` and `window_active`
  to the smoke state, a bounded press retry that logs
  `DESKTOP_INTEGRATION ... retry=N`, and a state dump on hover timeouts. The
  retry may only reduce the rate.
- **Triage:** read the failure dump. A `retry=` line means the press had no
  target at press time. No retry line with an emptied `owned_link` means the
  target was lost during the hold, which the retry cannot cover. Check
  `window_active` for the X11 focus-flicker theory.

## Linux client-frame smoke: first key lost after a title-bar drag

- **Label:** `Smoke Linux client-side frame` (part of `Smoke Linux command
  palette` before 2026-10-02) timed out waiting for `ACK:ackdragx`, and the
  terminal showed `ckdragx`.
- **Occurrences:** once, 2026-10-01 (Release Please PR #202).
- **Evidence:** the smoke already waits until Openbox releases its move grab.
  The first key after the grab still reached the window before GPUI drew the
  refocused frame and was dropped, as documented for other input smokes.
- **Mitigation:** the post-drag acknowledgment now resends its token within a
  bound and logs `CLIENT_FRAME_SMOKE ... ackdragx retry=N` when it had to.
- **Triage:** a `retry=` line shows the drop recurred and the mitigation
  absorbed it. A failure after ten attempts means input stopped reaching the
  window, which is a real regression.

## `smoke:macos-refresh` resize indicator

- **Label:** "one-pixel resize activates the indicator without resizing the
  grid", with opacity 0 in the last observed state.
- **Occurrences:** once, 2026-09-26 (PR #174); the rerun passed.
- **Candidates:** after `check_animations`, `resize_visibility` keeps a 100 ms
  hold, which leaves `check_render_resize` about 500 ms to see nonzero
  opacity; a runner stall would miss it. Alternatively, no frame rendered after
  `window.resize`. Restoring the default hold at the start of
  `check_render_resize` would only help the stall case.
- **Triage:** read the step's panic output first; this smoke has caught real
  bugs. On a second occurrence, look for frame evidence before changing the
  check.
- **Not this flake:** "missing pending_work_cancellation success" failed twice
  on feature branches (2026-10-01 alongside a `projection.rs` panic, and
  2026-10-02 on in-progress shared-viewer work). Treat that signature as a
  regression on the branch, not as this flake.

## huterm-core foreground-job tests under full local runs

- **Label:**
  `terminal::tests::close_confirmation_distinguishes_idle_shell_foreground_job_and_exit`
  and, once,
  `mux::lifecycle::tests::orphans_holding_the_terminal_need_consent_and_are_cleaned_up`.
- **Rate:** about 1 in 3 full `cargo test -p huterm-core --lib` runs on a
  developer Mac, at the 3-second "foreground job not detected" deadline. Both
  pass alone and in CI.
- **Triage:** rerun once before suspecting the change. A fix would replace the
  fixed deadline with a condition-driven wait.

## huterm-gpui terminfo temp-directory abort on hosted macOS

- **Label:**
  `terminfo::tests::linux_bundle_discovery_is_relative_to_the_executable` in
  `Checks (macOS arm64)`, where `create_dir_all` under `temp_dir()` returns
  EINVAL. The `TestDirectory` drop then panics on NotFound, and the double
  panic aborts every huterm-gpui lib test.
- **Occurrences:** once, 2026-10-01 (PR #204); the next push passed.
- **Triage:** check whether the change touches `terminfo.rs` or temp-directory
  handling, then rerun. A non-panicking cleanup in the drop would keep one
  failure from aborting the binary.

## CI tool installation

`Install job tools with Mise` and Zig installation steps can fail on Zig
community-mirror rate limits (HTTP 429) or a GitHub API 403. The current
bootstrap and its limits are in [the CI guide](ci.md#toolchain-bootstrap).
Treat a single 429 or 403 there as an infrastructure failure and rerun it.
Repeated failures usually follow Actions cache eviction: the repository has
exceeded GitHub's 10 GB cache limit before, which forced cold installs.

## Local environment hazards

These are not flakes, but they produce failures that look like regressions:

- macOS frame-driven benchmarks and smokes, such as `bench:output-latency` and
  the renderer smoke, get no frames while the screen is locked or the window
  is occluded, because GPUI stops the display link. Check
  `ioreg -n Root -d1 | rg CGSSessionScreenIsLocked` first. The first run after
  unlocking can still miss intervals; retry once.
- The `smoke:macos-quake` external grab probe needs one free global chord from
  `GRAB_CANDIDATES`. If every candidate is owned by another application, the
  probe writes `failed` with each candidate's error; read that file instead of
  assuming a regression.
