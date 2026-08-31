# Development and validation

On Ubuntu 24.04, first install GPUI's X11 link libraries and the software Vulkan
driver used by the headless smoke test:

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
| Pre-commit | `mise run check` | Format, Clippy, types, crate boundary | Local hook |
| Handoff | `mise run verify` | Check, tests, licenses, workflows | Implementer |
| Pull request | `mise run verify` on `macos-14` and Ubuntu 24.04 | Apple Silicon and Linux builds/tests | CI |
| Linux smoke | `mise run smoke:linux` | GPUI window remains live under Xvfb | CI or implementer |

The pre-commit decision is `add`: the canonical warm `check` task took 1.45
seconds on the Linux development host, within the project's 10-second hook
budget, and does not modify files. Reconsider the hook if the measured warm
time crosses that budget. Do not move license or workflow audits into the hook;
they belong to handoff and CI.

Linux compiles the actual GPUI client and can smoke its window/event loop under
Xvfb with Mesa's software Vulkan device. That smoke does not prove visual
correctness or native input behavior. Use Apple Silicon CI and the manual
checklist in the initial plan for macOS evidence.
