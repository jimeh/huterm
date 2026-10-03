# Huterm agent guide

Huterm is a Rust terminal emulator whose runtime owns PTYs, emulator state,
sessions, workspaces, tabs, and panes. GPUI is one client. The runtime boundary
must remain usable by a future local server and text client.

Read [README.md](README.md) for product scope and behavior. The boundaries
below and the topic guides in `docs/agents/` describe the current architecture.
Plans in `docs/plans/` record design decisions as they were made; implemented
and superseded plans are history, not current rules. Each plan opens with a
`Status:` line (proposed, approved, in progress, implemented, or superseded);
update it in the change that ships, revises, or replaces the plan.

Read [CONTEXT.md](CONTEXT.md) for canonical terminology and
[the workspace plan](docs/plans/workspaces-windows-tabs.md) for the agreed
direction and follow-up milestones. Core owns a logical socket scope with
ordered sessions, workspaces, tabs, and terminal runtimes. Each desktop window
creates a private session and workspace; shared attachments remain deferred.
Keep view destruction and detachment separate from explicit close.

## Where guidance lives

This file holds the rules that shape most changes. Platform and subsystem
hazards live in topic guides under `docs/agents/`. Before editing an area, read
its guide; most of its rules record a failure that already happened once.

| Change area | Guide |
| --- | --- |
| PTY I/O, process info, runtime workers, shutdown, Mux, IDs, terminal lifecycle, or a new binary that hosts terminals | [Core runtime](docs/agents/core-runtime.md) |
| GPUI threading, the window model, frame scheduling, close and Quit, rendering, keymap and menus, overlays, configuration, schemas | [Desktop client](docs/agents/desktop-client.md) |
| Keyboard and text input, mouse reporting, scrolling, selection, tab dragging, links, file drops | [Desktop input](docs/agents/desktop-input.md) |
| Fullscreen, Quake windows, the title row, window decorations, native window handles | [Window presentation](docs/agents/window-presentation.md) |
| Smoke fixtures, desktop harnesses, benchmarks | [Desktop smokes](docs/agents/desktop-smokes.md) |
| The Ghostty pin, `huterm-ghostty`, native builds, OSC color and mouse state | [Ghostty engine](docs/agents/terminal-engines.md) |
| Workflows, local actions, CI caches, toolchain bootstrap | [CI](docs/agents/ci.md) |
| Rust toolchain, GPUI and other pinned crates, vendored patches | [Dependencies](docs/agents/dependencies.md) |
| Packaging, signing, notarization, the release workflow | [Releases](docs/agents/releases.md) |
| Setup, Docker and Tart runs, repository scripts, icons, test layout, the validation ladder | [Development](docs/agents/development.md) |
| An intermittent red check | [Known flakes](docs/agents/known-flakes.md) |

When a change teaches something non-obvious, record it in the guide that owns
the topic, next to related rules, and merge it with any rule it overlaps. Add
to this file only what applies to most changes. `mise run lint:agents` fails
when this file exceeds its line budget or a guide link breaks.

## Boundaries that must hold

- `huterm-protocol` owns dependency-neutral IDs, input, lifecycle events, and
  immutable snapshots. Its public types must not expose GPUI, Ghostty, or
  `portable-pty` types. `mise run architecture` enforces its empty dependency
  set.
- `huterm-core` alone owns PTYs and canonical emulator state. One runtime
  thread mutates each terminal. Blocking PTY reads, ordered writes, and child
  waits stay off the GPUI thread.
- `huterm-gpui` owns macOS/Linux window state, rendering, key translation,
  focus, selection gestures, and scrollbar animation. The terminal runtime owns
  the shared viewport; clients send ordered scroll commands.
- Every terminal uses `libghostty-vt` through Huterm's own `huterm-ghostty`
  binding crate, behind core's private concrete engine. Only `huterm-core`
  depends on it. Do not expose native handles outside core or use Ghostty's
  application or PTY event loop. Keep engine selection out of runtime and
  protocol APIs. Do not copy or depend on GPL-covered Zed application or
  terminal-view code.
- `huterm-procinfo` reports operating-system process facts and depends on no
  Huterm crate (`mise run architecture` checks this).
- Every binary that hosts terminals calls `huterm_core::raise_open_file_limit()`
  first at startup; the GPUI bootstrap already does.
- Treat child exit, client detachment, and explicit terminal close as distinct
  lifecycle events. Any shutdown change must prove that live children and
  blocked I/O workers terminate.

## Commands

Run `mise tasks` to discover the full task set.

- `mise run doctor` checks host prerequisites; `mise run dev` starts the native
  macOS or Linux client.
- `mise run check` runs the fast format, Clippy compilation, test-module
  order, docs, and architecture gate; `mise run typecheck` remains available
  independently.
- `mise run test` runs unit and PTY integration tests.
- `mise run test:ghostty-releasesafe` reruns the `huterm-ghostty` and
  `huterm-core` tests against a ReleaseSafe Ghostty build in
  `target/ghostty-releasesafe`, where Zig's safety checks turn silent
  undefined behavior into failures. CI runs it on every pull request, and both
  `Verify` gates require it.
- `mise run verify` matches CI and adds dependency-license and workflow checks.
- `mise run bench:*` tasks measure rendering, scrolling, and output latency;
  [the smokes guide](docs/agents/desktop-smokes.md#benchmarks) lists their
  budgets.
- `mise run package:macos` builds and verifies a Sparkle-free universal
  `Huterm.app`; `package:macos-release` is the updater-enabled release input.
- `mise run vm:macos:smoke` runs the macOS desktop smokes in a disposable
  headless Tart VM so they do not take over the host display; `vm:macos:dev`
  runs a dev build in this worktree's persistent VM window. See the development
  guide.
- `mise run vm:linux:dev` runs a container-built Linux binary in a GNOME desktop
  Tart VM for manual QA; `vm:linux:dev:x11` selects Xorg over Wayland. Docker
  remains the path for Linux tests and smokes. See the development guide.
- `mise run package:linux:container` builds verified Linux packages in the
  pinned Ubuntu 22.04 container and exports them to host `dist/`.
- `mise run format` writes Rust formatting and refreshes action pins.

## Validation

Use focused Cargo tests while iterating. Run `mise run verify` before a broad
handoff. GitHub Actions is the source of truth for macOS arm64 compilation and
runs the same checks plus an Xvfb smoke on Linux x86_64. Linux development
requires the XKB packages documented in
[the development guide](docs/agents/development.md). Before treating an
intermittent failure as a regression, check
[known flakes](docs/agents/known-flakes.md).

Keep small Rust unit-test suites inline, after every production item in their
module; `mise run lint:test-order` enforces that order. Move a suite into an
adjacent child-module file when it dominates its source file or gets in the way
of reading the implementation. This is a judgment call, not a line count.
Extraction keeps module names, test paths, private access, and cfgs; a separate
file does not make a unit test an integration test. Read the relevant tests when
investigating or changing behavior, wherever they live. Leave unrelated tests in
place during routine fixes and move them only in an explicitly scoped refactor.
Each test covers one coherent behavior with as many assertions as that behavior
needs. See [Rust test layout](docs/agents/development.md#rust-test-layout).
Modules that land before their consumer carry `#[expect(dead_code)]` so Clippy
fails when the consumer arrives and the attribute must go.

For desktop behavior, run the relevant automated smoke tests before using
computer use. Discover them with `mise tasks`; native input coverage lives in
`smoke:macos-input` and `smoke:linux-input`, with separate macOS menu and Quit
smokes. Extend these tests when new behavior needs coverage. Use computer use
for uncovered behavior, visual checks, and investigation, then turn useful
manual reproductions into automated regression checks where practical. Physical
device behavior and unsupported IMEs may still require manual testing; state
those coverage limits explicitly.

The pre-commit hook checks staged files where a check can work per file, runs
only fast script tests that a staged path affects, and runs whole-project checks
such as Clippy only when a matching input is staged. It does not replace running
the relevant tests yourself. Install it with
`mise run setup`. Keep its representative warm path
below 10 seconds; CI remains authoritative because hooks can be bypassed.

## Dependency changes

The repository uses a three-day release-age policy for Mise tools, Cargo
updates, Bun packages, and GitHub Actions, and commits `Cargo.lock`,
`mise.lock`, and `bun.lock`. Pin direct runtime dependencies deliberately. Use
`mise run actions:update` to refresh action pins and `mise run tools:update` to
refresh project tools without accepting releases inside the cooldown window.
Keep the cooldown values in `mise.toml`, `bunfig.toml`, `.pinact.yaml`,
`.github/dependabot.yml`, and `zizmor.yml` aligned when changing the policy;
`scripts/policy-alignment.test.ts` checks them. Dependabot uses one extra day
because it counts calendar days while the other tools require full hours.
Toolchain and crate-specific pins, and the bumps that need a regeneration
commit, are in [the dependencies guide](docs/agents/dependencies.md).

Run `mise run license` after any dependency change. GPL and AGPL dependencies,
unknown registries, Git dependencies, and Cargo wildcard requirements are not
allowed.
