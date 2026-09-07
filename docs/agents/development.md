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
  libxkbcommon-dev libxkbcommon-x11-dev mesa-vulkan-drivers xvfb \
  xdotool x11-xkb-utils
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
toggles native fullscreen. Closing a tab stops its terminal. Closing a shared
session view detaches it; closing the final view terminates that session and
its terminals. Explicit detachment preserves sessions without viewers. Huterm
exits after the last window closes only when no sessions or pending spawns
remain. Foreground and background jobs require confirmation while the shell is
alive. Root-shell exit closes its tab quietly unless
`[terminal] close_on_exit = false` retains the history.

Configuration is loaded at startup from `$HUTERM_CONFIG_FILE`,
`$XDG_CONFIG_HOME/huterm/config.toml`, or `~/.config/huterm/config.toml`, in
that order. Settings creates the default document without overwriting an
existing file and opens it with the system editor. Malformed TOML and invalid
`terminal.engine` values are fatal at startup, before UI creation. With valid
TOML and a known engine, unrelated settings errors fall back to defaults while
preserving that engine and showing a diagnostic in the terminal status overlay.

Clipboard shortcuts are `Cmd-C` and `Cmd-V` on macOS and `Ctrl-Shift-C` and
`Ctrl-Shift-V` on Linux. Plain `Ctrl-C` remains terminal input. Shift-modified
Page Up, Page Down, and End scroll the viewport. The macOS Window menu exposes
native Minimize and Zoom commands.

Window and tab shortcuts:

| Action | macOS | Linux |
| --- | --- | --- |
| New window | `Cmd-N` | `Ctrl-Shift-N` |
| New tab | `Cmd-T` | `Ctrl-Shift-T` |
| Close tab | `Cmd-W` | `Ctrl-Shift-W` |
| Close window | `Cmd-Shift-W` | `Ctrl-Shift-Q` |
| Next / previous tab | `Ctrl-Tab` / `Ctrl-Shift-Tab` | `Ctrl-Tab` / `Ctrl-Shift-Tab` |
| Select tab 1 through 8 | `Cmd-1` through `Cmd-8` | `Alt-1` through `Alt-8` |
| Select last tab | `Cmd-9` | `Alt-9` |

Set `[window].tab_position` to `top`, `bottom`, `left`, or `right`. Top is the
default. Horizontal tabs divide the available width equally until their
120-pixel minimum, then scroll horizontally. Vertical tabs stay 32 pixels tall
and fill the sidebar width. Drag the sidebar's inner edge to resize it between
140 and 400 logical pixels, capped at half the window width. Each window keeps
its preferred width for its lifetime, including through temporary window
shrinking. The full-width vertical new-tab button follows the last tab and
stays visible at the bottom when tabs overflow.

Trackpad and wheel scrolling move the strip without selecting a tab. Floating
arrows indicate hidden content and animate scrolling when clicked. Explicit
selection or creating a tab reveals it; ordinary redraws preserve manual
scroll. Drag a tab to reorder it within its window, horizontally or
vertically. The preview and insertion position remain constrained to the bar
when the pointer leaves it; release commits and Escape cancels. During a drag,
hovering an overflow edge scrolls continuously without changing the active
terminal. Reordering preserves terminal processes, focus, selection, and
scroll position. Cross-window moves and tear-out remain deferred.
Reload applies placement, font, padding, and theme changes across all windows.
Shell titles label tabs, with the launched program as fallback. Directory and
process labels and directory inheritance are not implemented yet.

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
| Linux keyboard | `mise run smoke:linux-input` | XTest input through XKB, shortcut dispatch, and raw PTYs with both engines | CI or implementer |
| macOS menus | `mise run smoke:macos-menus` | Real AppKit shortcut values at startup and reload | CI or implementer |
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
Under Xvfb, it enforces snapshot elapsed time, input-to-snapshot latency, wakeup
delay, returned offsets, and queue bounds. If the host produces at least 25 paint
samples, it also enforces combined paint elapsed time, input-to-paint latency,
and row reuse. The same task runs in a native window on macOS to collect those
frame-bound measurements. Benchmark metadata records the hardware model and
GPUI window scale. CPU preparation and paint encoding do not prove GPU
presentation. These durations use a monotonic wall clock and include scheduler
preemption, not just per-thread CPU execution.

On Apple Silicon macOS, `mise run package:macos` creates
`target/release/bundle/Huterm.app` and verifies its identifier, Cargo-derived
version, Developer Tools category, icon, executable, and arm64 architecture.

## Terminal engines

Every build includes both engines; `mise run dev` starts the app. Set
`[terminal] engine = "ghostty"`
and reload to use Ghostty for new tabs and windows. Shared scrolling and immutable
rows apply to both engines. See [the engine guide](terminal-engines.md) for
pinned native inputs, license coverage, and matched benchmark commands.

`check`, `test`, and `verify` exercise both engines and prepare the pinned native
source through Mise. Normal build and packaging tasks do the same.
Standard setup installs the pinned Bun and Zig tools alongside
Rust. Repository scripts run on Bun and are type-checked with TypeScript 7;
Python is not required. Run `mise run scripts:install` to install the locked
TypeScript dependencies, `mise run check:scripts` for script tests and type
checking, and `mise run audit:scripts` for dependency advisories. These checks
also run through the appropriate verification and CI tasks.

### Native macOS input smoke

Run `mise run smoke:macos-input` in a macOS GUI session with the US, ABC, or
British keyboard layout selected. The smoke reads the current input source and
fails with its identifier when unsupported. It does not change the input source
or require Accessibility permission.

The dedicated `native_input_smoke` executable opens production WorkspaceView and
TerminalView instances. Its isolated native helper posts synthetic NSEvent key,
modifier, and mouse events to NSApplication's queue. AppKit dispatches them through
GPUI's native window, shortcut matcher, and text input handler. Queue dispatch
also supplies the real NSApplication.currentEvent used by Option composition.
The ordinary application entrypoint does not enable the smoke command protocol.

Both Alacritty and Ghostty receive input through a raw, no-echo PTY recorder.
Assertions compare cumulative bytes after a printable barrier, including negative
assertions for consumed shortcuts and replayed prefixes. Read-only probes wait
for GPUI pending-input completion, configuration changes, tab readiness, and
terminal exit. Cases cover Option symbols/dead keys, Meta policy reload,
printable chord timeout/mismatch, collapsed fallback, modifier-only mismatch,
selection changes during pending input, fallback reload removing its binding,
paste, and mouse-driven tab focus cancellation. The recorder has a bounded
lifetime, and normal completion exits it before invoking native Quit. Failure
cleanup requests application shutdown before escalating to process signals.

The runner snapshots every clipboard item's declared data formats and restores
those bytes in its cleanup path, including an empty clipboard. It runs in a
separate process so app failure cannot discard the snapshot. Avoid editing the
clipboard while the smoke runs. A restoration error retains the temporary
snapshot and fails the task.

This proves synthetic AppKit event dispatch into real GPUI input and PTY bytes.
It does not prove physical keyboard device behavior, arbitrary input-method
preedit, alternate layouts, or hardware key-up ordering.
