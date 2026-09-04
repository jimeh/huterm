# Development and validation

## macOS prerequisites

GPUI compiles Metal shaders as part of the macOS build. Install Xcode and make
sure the selected Xcode has Apple's optional Metal Toolchain. Recent Xcode
installations may not include it initially:

```sh
xcodebuild -downloadComponent MetalToolchain
```

If multiple Xcode versions are installed, select the intended version with
`xcode-select` before downloading the component. `mise run doctor` checks that
Xcode is selected and that `xcrun` can locate the Metal compiler. Apple
documents both the Xcode settings and command-line installation paths in
[Downloading and installing additional Xcode components][apple-components].

[apple-components]: https://developer.apple.com/documentation/xcode/downloading-and-installing-additional-xcode-components

## Ubuntu 24.04 prerequisites

Install GPUI's X11 link libraries and the software Vulkan driver used by the
headless smoke test:

```sh
sudo apt-get install --no-install-recommends \
  libxkbcommon-dev libxkbcommon-x11-dev mesa-vulkan-drivers xvfb
```

These packages are tracked by apt and can be removed with `sudo apt-get remove`
if they are not needed by another application. Other Linux distributions need
the equivalent XKB development libraries, Vulkan driver, and Xvfb packages.
Run `mise run doctor` to check the link-time prerequisites.

Then install the pinned Rust and validation tools plus the local hook:

```sh
mise run setup
```

The initial desktop client runs on macOS and Linux:

```sh
mise run dev
```

On macOS, the app launches `$SHELL -l` in the user's home directory, matching a
Finder launch, and supplies `LANG=en_US.UTF-8` only when no locale variable is
inherited. Linux launches `$SHELL` in the current working directory. The
fallback is `/bin/zsh` on macOS or `/bin/sh` on Linux. `Ctrl-Cmd-F` or `F11`
toggles native fullscreen. Closing the window shuts down the terminal runtime
and its child process.

Configuration is loaded at startup from `$HUTERM_CONFIG_FILE`,
`$XDG_CONFIG_HOME/huterm/config.toml`, or `~/.config/huterm/config.toml`, in
that order. Settings creates the default document without overwriting an
existing file and opens it with the system editor. Invalid settings fall back
to defaults and remain visible in the terminal status overlay.

Clipboard shortcuts are `Cmd-C` and `Cmd-V` on macOS and `Ctrl-Shift-C` and
`Ctrl-Shift-V` on Linux. Plain `Ctrl-C` remains terminal input. Shift-modified
Page Up, Page Down, and End scroll the viewport. The macOS Window menu exposes
native Minimize and Zoom commands.

## Validation ladder

| Trigger | Command | Scope | Evidence owner |
| --- | --- | --- | --- |
| Iteration | focused `cargo test -p <crate> <test>` | Changed behavior | Implementer |
| Pre-commit | Lefthook change-aware jobs | Staged Markdown/Rust plus affected whole-workspace analysis | Local hook |
| Handoff | `mise run verify` | Check, tests, licenses, workflows | Implementer |
| Pull request | `mise run verify:platform` on `macos-14` and Ubuntu 24.04 | Apple Silicon and Linux builds/tests | CI |
| Pull request | `mise run verify:policy` on Ubuntu 24.04 | Docs and workflow policy | CI |
| Pull request | `mise run license` on Ubuntu 24.04 | Dependency policy and advisories | CI |
| Linux smoke | `mise run smoke:linux` | GPUI window remains live under Xvfb | CI or implementer |
| Scroll benchmark | `mise run bench:scroll` | Snapshot timing, offsets, and queue bounds; paint timing and row reuse when frames arrive | Implementer |
| macOS package | `mise run package:macos` | Apple Silicon app metadata, icon, executable, and architecture | CI or implementer |

The pre-commit hook runs independent jobs in parallel. Markdown and Rust
formatting receive only matching staged paths. Clippy compilation and the
protocol boundary remain whole-workspace checks, but run only when staged Rust
or Cargo inputs can affect them. Workflow policy runs only for staged Actions
or policy configuration, and harness configuration validates its own task and
hook definitions. Keep the representative warm path below the project's
10-second hook budget. Dependency audits remain in handoff and CI because they
are broader and may refresh advisory data.

Linux compiles the actual GPUI client and can smoke its window/event loop under
Xvfb with Mesa's software Vulkan device. That smoke does not prove visual
correctness or native input behavior. Use Apple Silicon CI and the manual
checklist in the initial plan for macOS evidence.

On Linux, the opt-in renderer and scroll benchmarks need `twm`, which ensures
GPUI's window is exposed and painted under Xvfb:

```sh
sudo apt-get install --no-install-recommends twm
mise run bench:renderer
mise run bench:scroll
```

`bench:scroll` drives wheel-equivalent fractional movement, rows, pages, and
thumb jumps through the production scroll controller over 10,000 unique rows.
Under Xvfb, it enforces snapshot CPU, input-to-snapshot latency, wakeup delay,
returned offsets, and queue bounds. If the host produces at least 25 paint
samples, it also enforces combined paint CPU, input-to-paint latency, and row
reuse. The same task runs in a native window on macOS to collect those
frame-bound measurements. Benchmark metadata records the hardware model and
GPUI window scale. CPU preparation and paint encoding do not prove GPU
presentation.

On Apple Silicon macOS, `mise run package:macos` creates
`target/release/bundle/Huterm.app` and verifies its identifier, Cargo-derived
version, Developer Tools category, icon, executable, and arm64 architecture.
