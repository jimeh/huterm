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

The app launches `$SHELL` in the current working directory, falling back to
`/bin/zsh` on macOS or `/bin/sh` on Linux. `Ctrl-Cmd-F` or `F11` toggles native
fullscreen. Closing the window shuts down the terminal runtime and its child
process.

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

The opt-in renderer benchmark additionally needs `twm`, which ensures GPUI's
window is exposed and painted under Xvfb:

```sh
sudo apt-get install --no-install-recommends twm
mise run bench:renderer
```
