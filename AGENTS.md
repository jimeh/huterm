# Huterm agent guide

Huterm is a Rust terminal emulator whose runtime owns PTYs, emulator state,
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
- After a nonblocking PTY read returns `WouldBlock`, wait for readability
  instead of sleeping before every retry. Use `filedescriptor` for this wait:
  its macOS implementation avoids the platform's unreliable PTY `poll(2)` by
  using `select(2)`.

## Commands

Run `mise tasks` to discover the full task set.

- `mise run doctor` checks host prerequisites; `mise run dev` starts the native
  macOS or Linux client.
- `mise run check` runs the fast format, Clippy compilation, docs, and
  architecture gate; `mise run typecheck` remains available independently.
- `mise run test` runs unit and PTY integration tests.
- `mise run verify` matches CI and adds dependency-license and workflow checks.
- `mise run bench:renderer` drives the release renderer under Xvfb and reports
  opt-in CPU preparation and paint-encoding timings.
- `mise run bench:scroll` drives production scroll inputs against 10,000 rows
  and enforces snapshot CPU, wakeup, offset, and bounded-queue budgets under
  Xvfb. It also enforces paint CPU, reuse, and input latency when the host
  delivers enough frames.
- `mise run package:macos` builds and verifies the Apple Silicon `Huterm.app`.
- `mise run format` writes Rust formatting and refreshes action pins.

Repository Rust formatting is defined by `rustfmt.toml`; it must not depend on
or require changes to `~/.rustfmt.toml`.
Keep the Rust version, minimal profile, and `clippy`/`rustfmt` components in
`mise.toml` aligned with `rust-toolchain.toml`; CI installs only the named Mise
tools for each job.

Keep `verify:toolchain` as a serial preflight before `verify:parallel`. Mise's
CI cache can restore its Rust install symlink without the corresponding rustup
toolchain, and parallel Cargo invocations then race while materializing it.

Use focused Cargo tests while iterating. Run `mise run verify` before a broad
handoff. GitHub Actions is the source of truth for macOS arm64 compilation and
runs the same checks plus an Xvfb smoke on Linux x86_64. Linux development
requires the XKB packages documented in
[the development guide](docs/agents/development.md).

The pre-commit hook runs staged-path formatting and Markdown checks, then
triggers whole-workspace Clippy or workflow checks only for relevant staged
inputs. Install it with `mise run setup`. Keep its representative warm path
below 10 seconds; CI remains authoritative because hooks can be bypassed.

## Dependency changes

The repository uses a three-day release-age policy for Mise tools, Cargo
updates, and GitHub Actions, and commits `Cargo.lock` and `mise.lock`. Pin
direct runtime dependencies deliberately. Use `mise run actions:update` to
refresh action pins and `mise run tools:update` to refresh project tools
without accepting releases inside the cooldown window.
Keep the cooldown values in `mise.toml`, `.pinact.yaml`,
`.github/dependabot.yml`, `.github/workflows/ci.yml`, and `zizmor.yml` aligned
when changing the policy.
The published GPUI dependency graph needs Rust 1.88 or newer; project tooling
pins Rust 1.98.
GPUI 0.2.2 depends on `stacksafe` 0.1.x, whose `proc-macro-error2` dependency
emits a Rust future-incompatibility warning. The published `stacksafe` 1.x fix
is outside GPUI's semver requirement, so resolve this through a future GPUI
release rather than a Git or local patch.
Keep direct `nix` on the newest 0.29.x release while `portable-pty`'s
`MasterPty` exposes only `as_raw_fd()`. Nix 0.30 and newer require `AsFd` for
`fcntl`; upgrading earlier would require an unsafe borrowed-descriptor
conversion in `huterm-core`, which denies unsafe code. Remove the matching
Dependabot ignore when a `portable-pty` release exposes a safe borrowed
descriptor.
On macOS, GPUI shader compilation needs Xcode's optional Metal Toolchain;
`mise run doctor` checks it and reports the installation command.
On Ubuntu, GPUI's X11 backend needs both XKB development packages at link time
and a Vulkan device at runtime; CI uses Mesa's software Vulkan driver under
Xvfb.
GPUI's `ShapedLine` contains a large inline decoration buffer. Retained terminal
rendering should cache `Arc<LineLayout>` from `layout_line`, not `ShapedLine`,
and apply colors and decorations during paint. Use the generic `monospace` font
family on Linux; requesting macOS-only Menlo repeatedly exercises GPUI's
missing-font fallback path.
Headless Xvfb does not deliver continuous GPUI frames after the initial
presentation. Keep Linux scroll gates based on completion-time snapshot and
queue evidence; require frame-bound paint and input-latency evidence only on a
host that actually delivers enough frames.
Run `mise run license` after any dependency change. GPL and AGPL dependencies,
unknown registries, Git dependencies, and Cargo wildcard requirements are not
allowed.
