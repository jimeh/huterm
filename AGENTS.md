# HUTerm agent guide

HUTerm is a Rust terminal emulator whose runtime owns PTYs, emulator state,
sessions, tabs, and panes. GPUI is one client. The runtime boundary must remain
usable by a future local server and text client.

Read [README.md](README.md) for product scope and
[the initial desktop plan](docs/plans/initial-desktop-poc.md) for the current
architecture and acceptance criteria.

## Boundaries that must hold

- `huterm-protocol` owns dependency-neutral IDs, input, lifecycle events, and
  immutable snapshots. Its public types must not expose GPUI, Alacritty, or
  `portable-pty` types. `mise run architecture` enforces its empty dependency
  set.
- `huterm-core` alone owns PTYs and canonical Alacritty state. One runtime
  thread mutates each terminal. Blocking PTY reads, ordered writes, and child
  waits stay off the GPUI thread.
- `huterm-gpui` owns macOS/Linux window state, rendering, key translation,
  focus, and each client's viewport offset. Scrolling must not mutate shared
  emulator state.
- Use upstream `alacritty_terminal`; do not use its PTY event loop. Do not copy
  or depend on GPL-covered Zed application or terminal-view code.
- Treat child exit, client detachment, and explicit terminal close as distinct
  lifecycle events. Any shutdown change must prove that live children and
  blocked I/O workers terminate.
- On Unix, configure the PTY master as nonblocking before cloning reader and
  writer handles; the clones share its open-file-description flags.

## Commands

Run `mise tasks` to discover the full task set.

- `mise run doctor` checks host prerequisites; `mise run dev` starts the native
  macOS or Linux client.
- `mise run check` runs the fast format, lint, type, and architecture gate.
- `mise run test` runs unit and PTY integration tests.
- `mise run verify` matches CI and adds dependency-license and workflow checks.
- `mise run format` writes Rust formatting and refreshes action pins.

Repository Rust formatting is defined by `rustfmt.toml`; it must not depend on
or require changes to `~/.rustfmt.toml`.

Keep `verify:toolchain` as a serial preflight before `verify:parallel`. Mise's
CI cache can restore its Rust install symlink without the corresponding rustup
toolchain, and parallel Cargo invocations then race while materializing it.

Use focused Cargo tests while iterating. Run `mise run verify` before a broad
handoff. GitHub Actions is the source of truth for macOS arm64 compilation and
runs the same checks plus an Xvfb smoke on Linux x86_64. Linux development
requires the XKB packages documented in
[the development guide](docs/agents/development.md).

The pre-commit hook runs `mise run check`. Install it with `mise run setup`.
Keep it only while its warm runtime stays below 10 seconds; CI remains
authoritative because hooks can be bypassed.

## Dependency changes

The repository uses a seven-day Mise release-age policy and commits
`Cargo.lock` and `mise.lock`. Pin direct runtime dependencies deliberately.
The published GPUI dependency graph needs Rust 1.88 or newer; project tooling
pins Rust 1.98.
On macOS, GPUI shader compilation needs Xcode's optional Metal Toolchain;
`mise run doctor` checks it and reports the installation command.
On Ubuntu, GPUI's X11 backend needs both XKB development packages at link time
and a Vulkan device at runtime; CI uses Mesa's software Vulkan driver under
Xvfb.
Run `mise run license` after any dependency change. GPL and AGPL dependencies,
unknown registries, Git dependencies, and Cargo wildcard requirements are not
allowed.
