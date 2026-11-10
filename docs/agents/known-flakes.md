# Known flakes

Intermittent failures whose cause is unconfirmed or whose fix is deferred.
Check this list before treating a red check as a regression, and compare
against main's recent runs for the same job.

## The registry

Each flake is one file under [`flakes/`](flakes/): TOML front matter between
`+++` lines, then the evidence and triage in Markdown. The front matter holds
what tools read:

- `status`: `open`, `fixing` (name the PR or thread in `owner`), or `fixed`.
- `kind` and `selector`: a Rust test (`binary`, nextest's binary ID, and
  `tests`), a smoke step (`steps`, the `HUTERM_CI_SMOKE_STEP` names), a Bun
  test (`file` and `tests`), or an infrastructure step (`step_names`).
- `platforms`: where it fails: `linux-x86_64`, `linux-aarch64`,
  `macos-arm64`, or `macos-x86_64`.
- `signature`: regular expressions. Every `match` pattern must fit the
  failure, and no `reject` pattern may. The failure text is the test's
  output for Rust tests, and the step's error, standard output, and standard
  error for smokes. One `match` pattern needs a literal run of at least 12
  characters, so a signature cannot match everything.
- `retries`: reruns per failure: at most 2 for Rust tests and 1 for smokes,
  and 0 for Bun, infrastructure, and fixed entries.
- `review_by`: at most 90 days ahead. Past it, `flakes:check` warns, and
  every retried pass says "review overdue".
- `occurrences`: one `[[occurrences]]` table per failure, with `date`, `url`,
  `pr`, `sha`, `platform`, `retried`, `local`, and `note` as known.

`mise run flakes:check` validates the entries and their links, and fails when
the index below is stale; `mise run check` and the Policy job run it.
`mise run flakes:index` rewrites the index.

## Retries in CI

The Rust test runner (`scripts/test-rust.ts`) and the smoke step supervisor
(`scripts/run-smoke-step.ts`) classify every failure against the registry.
They rerun a failure only when:

- an `open` or `fixing` entry matches its selector, platform, and signature;
- the entry has retries left for this failure; and
- the job has retried at most one other entry. A third distinct flake in one
  job fails it, since that many at once points at a regression or a broken
  runner.

Any unmatched failure in the same run fails the step without reruns, and a
rerun that fails with a different signature fails it at once. A failure that
exhausts its retries fails CI: a repeated failure may be a real regression.
A failure that matches a `fixed` entry fails as a regression.

Every retried pass gets a warning annotation that names the entry, a row in
the step summary, and a line in the job's `flake-report-*` artifact. Local
runs never retry; they print the matching entry and fail.
`HUTERM_FLAKE_RETRY=1` opts in, and CI sets it.

## Recording a failure

Run `mise run flakes:record -- <run-or-job-url>` and commit the result. It
reads the run's flake reports, and the logs of its failed jobs, and appends an
occurrence to each matching entry; recording the same job twice adds nothing.
Job logs do not say which step printed a line, so it only names infrastructure
matches: check the failed step and add those occurrences by hand.
A failure that matches no entry is reported, so you can add one: copy an
entry's front matter, write a signature from the failure text, and run
`mise run flakes:index`. Edit an entry's prose only when the evidence itself
is new. Set `status = "fixed"` with a new `review_by` when a fix lands, and
delete the entry once the review date passes without a recurrence.

## Index

<!-- flakes:index:start -->

### Open

| Flake | Kind | Platforms | Seen | Last | Retries | Review by |
| --- | --- | --- | --- | --- | --- | --- |
| [GitHub API 403 during CI tool installation](flakes/ci-mise-github-api-403.md) | infra | linux-x86_64, linux-aarch64, macos-arm64 | none recorded | never | 0 | 2027-01-15 |
| [Linux quake smoke reads no active window](flakes/linux-quake-active-window-unset.md) | smoke | linux-x86_64 | 1 CI | 2026-11-10 | 1 | 2027-01-15 |
| [macOS clipboard smoke witness loses the app's pid](flakes/macos-clipboard-witness-missing-pid.md) | smoke | macos-arm64 | 1 CI | 2026-11-10 | 0 | 2026-12-10 |
| [huterm-core command markers queued behind the exit](flakes/core-record-markers-after-exit.md) | rust | macos-arm64 | 1 local | 2026-11-06 | 0 | 2026-12-06 |
| [huterm-gpui inactive tab close button](flakes/gpui-inactive-tab-close-button.md) | rust | macos-arm64, linux-x86_64, linux-aarch64 | 4 CI | 2026-11-06 | 2 | 2026-12-06 |
| [GitHub release download 5xx during tool installation](flakes/ci-mise-github-download-5xx.md) | infra | linux-x86_64, linux-aarch64, macos-arm64 | 2 CI | 2026-11-05 | 0 | 2027-01-15 |
| [Linux renderer smoke startup hang](flakes/linux-renderer-startup-hang.md) | smoke | linux-x86_64, linux-aarch64 | 10 CI | 2026-11-05 | 1 | 2026-12-06 |
| [macOS quake smoke departed-witness activation](flakes/macos-quake-departed-witness-activation.md) | smoke | macos-arm64 | 1 CI, 1 local | 2026-10-30 | 1 | 2026-12-06 |
| [Install Zig hang](flakes/ci-zig-install-hang.md) | infra | macos-arm64, linux-x86_64, linux-aarch64 | 1 CI | 2026-10-28 | 0 | 2027-01-15 |
| [Release stapler CloudKit ticket lookup](flakes/release-stapler-cloudkit-lookup.md) | infra | macos-arm64 | 3 CI | 2026-10-27 | 0 | 2027-01-15 |
| [Zig mirror rate limit during CI tool installation](flakes/ci-zig-mirror-rate-limit.md) | infra | linux-x86_64, linux-aarch64, macos-arm64 | 1 CI | 2026-10-01 | 0 | 2027-01-15 |
| [Linux client-frame smoke first key lost after a title-bar drag](flakes/linux-client-frame-first-key-after-drag.md) | smoke | linux-x86_64, linux-aarch64 | 1 CI | 2026-10-01 | 0 | 2026-12-06 |
| [Linux integration smoke held-link hover timeout](flakes/linux-integration-held-link-hover.md) | smoke | linux-x86_64, linux-aarch64 | 2 CI | 2026-09-29 | 1 | 2026-12-06 |

### Fixed, under observation

A failure that matches one of these fails CI as a regression. Remove an
entry once its review date passes without a recurrence.

| Flake | Kind | Platforms | Seen | Last | Retries | Review by |
| --- | --- | --- | --- | --- | --- | --- |
| [huterm-core foreground-job tests under full local runs](flakes/core-foreground-job-tests.md) | rust | macos-arm64 | 1 CI, 11 local | 2026-11-06 | 0 | 2026-12-06 |
| [idle-sample.test.ts native sampler compile timeout](flakes/idle-sample-compile-timeout.md) | bun | macos-arm64 | 1 CI, 1 local | 2026-11-06 | 0 | 2026-12-06 |
| [Linux palette smoke busy step](flakes/linux-palette-busy-step.md) | smoke | linux-x86_64, linux-aarch64 | 15 CI | 2026-11-05 | 0 | 2026-12-06 |
| [huterm-gpui pane rename title on hosted macOS](flakes/gpui-pane-rename-title.md) | rust | macos-arm64, linux-x86_64, linux-aarch64 | 2 CI | 2026-11-04 | 0 | 2026-12-06 |
| [Linux input smoke tear-off cue timeout](flakes/linux-input-tear-off-cue.md) | smoke | linux-x86_64, linux-aarch64 | 2 CI | 2026-11-04 | 0 | 2026-12-06 |
| [Linux palette smoke wheel does not scroll the results](flakes/linux-palette-wheel-scroll.md) | smoke | linux-x86_64, linux-aarch64 | 3 CI | 2026-11-04 | 0 | 2026-12-06 |
| [Linux quake smoke external witness search timeout](flakes/linux-quake-witness-search-timeout.md) | smoke | linux-x86_64, linux-aarch64 | 1 CI | 2026-11-03 | 0 | 2026-12-06 |
| [huterm-gpui Edit menu Select All](flakes/gpui-edit-menu-select-all.md) | rust | macos-arm64, linux-x86_64, linux-aarch64 | 6 CI, 1 local | 2026-10-31 | 0 | 2026-12-06 |
| [Linux overlays smoke Enter on "Close 2 tabs?" closes no tabs](flakes/linux-overlays-close-tabs-enter.md) | smoke | linux-x86_64, linux-aarch64 | 7 CI | 2026-10-31 | 0 | 2026-12-06 |
| [macOS input smoke empty state read](flakes/macos-input-empty-state-read.md) | smoke | macos-arm64 | 2 CI, 1 local | 2026-10-30 | 0 | 2026-12-06 |
| [smoke-process.test.ts deadlines under parallel builds](flakes/smoke-process-test-deadlines.md) | bun | macos-arm64 | 4 local | 2026-10-30 | 0 | 2026-12-06 |
| [huterm-gpui replaced view's clipboard write on Linux](flakes/gpui-replaced-view-clipboard-write.md) | rust | linux-x86_64, linux-aarch64 | 5 CI | 2026-10-28 | 0 | 2026-12-06 |
| [macOS palette smoke stray DEL at an early step](flakes/macos-palette-stray-del.md) | smoke | macos-arm64 | 2 local | 2026-10-26 | 0 | 2026-12-06 |
| [huterm-procinfo live group leader arguments on Linux](flakes/procinfo-live-group-leader-arguments.md) | rust | linux-x86_64, linux-aarch64 | 4 CI | 2026-10-26 | 0 | 2026-12-06 |
| [macOS refresh smoke resize indicator](flakes/macos-refresh-resize-indicator.md) | smoke | macos-arm64 | 1 CI | 2026-09-26 | 0 | 2026-12-06 |

<!-- flakes:index:end -->

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
