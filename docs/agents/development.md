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
  xdotool x11-xkb-utils x11-utils openbox xcompmgr
```

These packages are tracked by apt and can be removed with `sudo apt-get remove`
if they are not needed by another application. Other Linux distributions need
the equivalent XKB development libraries, Vulkan driver, and Xvfb packages.
Run `mise run doctor` to check the link-time prerequisites.

Then install the pinned Rust and validation tools plus the local hook:

```sh
mise run setup
```

## Linux checks through Docker

Docker can run the Linux checks from macOS or Linux. The runner defaults to
Docker's native architecture and accepts `--arch amd64` or `--arch arm64`.
Docker must already be running with a Linux engine; non-native architectures
require working emulation in that engine.

```sh
mise run linux:test
mise run linux:smoke
mise run linux:test -- --arch amd64
mise run linux:smoke -- --arch arm64
mise run linux:exec -- mise run package:linux
mise run linux:exec -- mise run smoke:renderer
mise run linux:exec -- --arch amd64 mise run build:exec -- \
  cargo test -p huterm-gpui renderer::builtin --lib
```

`linux:test` runs the full Rust unit and PTY integration suite. `linux:smoke`
runs the existing renderer, application, input, and fullscreen smokes under
Xvfb with Mesa software Vulkan. `linux:exec` accepts a command and literal
arguments, including a focused Cargo test or `mise run lint`. Before direct
Cargo commands on a cold workspace, run `mise run linux:exec -- mise run
ghostty:prepare`, using the same `--arch` selection for both invocations.
These checks exercise Linux X11 rendering, not native Wayland or physical GPU
behavior. Keep timing benchmarks on native hardware.

The first invocation builds a local Ubuntu 24.04 image with pinned Mise, Rust,
Bun, and Zig. The Ubuntu index digest and Mise archive checksums are in
`scripts/linux/Dockerfile`; tool versions come from `mise.toml`, `mise.lock`,
and `rust-toolchain.toml`. Apt packages resolve to Ubuntu's updates when the
image is built. Image reuse depends on those files and the container
entrypoint, so ordinary source edits do not rebuild the image. Images are
local and are not published.

The checkout is mounted read-only, then copied into a Docker volume before
each run. This includes current uncommitted and untracked source files and
removes obsolete copies. Host `.git`, `target`, `.native`, `node_modules`, and
`.codegraph` directories are excluded. The copied workspace has no Git
metadata. Linux build artifacts, native source preparation, and dependency
caches stay in volumes scoped to the checkout's real path and architecture.
The runner allows only one active run for each such pair.

Containers are removed on completion or interruption; cache volumes remain. To
remove only this worktree's caches for one architecture:

```sh
mise run linux:clean
mise run linux:clean -- --arch amd64
```

Cleanup leaves cached images available for other worktrees. It fails if a
volume is still in use. Interrupted processes are stopped and removed by
container ID, so a name conflict cannot remove another run. A forced kill of
the host runner can leave a container behind; inspect `docker ps -a` before
removing it. The runner never prunes unrelated Docker resources.

Compilation defaults to four Cargo jobs to limit memory use in desktop VMs.
For a different limit, use `linux:exec` with `env CARGO_BUILD_JOBS=2` before
the command. AMD64 execution on an ARM64 engine uses emulation and can be
slower than native ARM64. Native x86_64 CI remains the architecture-specific
check.

The initial desktop client runs on macOS and Linux:

```sh
mise run dev
```

On macOS, the app launches `$SHELL -l` in the user's home directory, matching a
Finder launch, and supplies `LANG=en_US.UTF-8` only when no locale variable is
inherited. Linux launches `$SHELL` in the current working directory. The
fallback is `/bin/zsh` on macOS or `/bin/sh` on Linux. `Cmd-Enter` on macOS or
`F11` toggles the configured fullscreen mode. macOS defaults to `non_native` in
the current Space; `window.macos_fullscreen_mode = "native"` selects a separate
Space. The green window button still uses native fullscreen. Linux always uses
native fullscreen.
Closing a tab stops its terminal. Closing a shared
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

The tab bar is hidden with one tab by default. Set
`[window].always_show_tab_bar = true` to keep it visible. With two or more tabs,
it reserves space beside the terminal.

Set `[window].auto_hide_tab_bar_in_fullscreen = true` to reveal the bar only
when the pointer reaches its attached edge in fullscreen, even with one tab.
Fullscreen auto-hide overrides `always_show_tab_bar` and reserves no bar space,
regardless of tab count. It slides over the terminal without changing the grid,
and hides after the pointer leaves. Tab dragging and sidebar resizing keep it
open. A top bar can be revealed from the macOS notch-height region and appears
below the notch. Tab-switch commands, successful tab creation, and tab closure
also reveal the fullscreen overlay for one second before the normal dismissal
delay. Repeated activity restarts the hold.
Both settings default to `false` and take effect on config reload.

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
| Pull request | `mise run format:check`, `mise run ci:lint`, `mise run schema:check`, `mise run check:scripts`, and `mise run ci:test` on `macos-14`, Ubuntu 24.04 x86_64, and Ubuntu 24.04 aarch64 | Rust formatting, Clippy, protocol boundaries, generated schemas, scripts, and Rust tests in one job per platform | CI |
| Pull request | Named platform smoke steps on `macos-14`, Ubuntu 24.04 x86_64, and Ubuntu 24.04 aarch64, implemented as slices of `mise run ci:smoke:run`; equivalent to the local `mise run ci:smoke` aggregate and order | Cached native source preparation, one smoke binary compilation, then serial desktop smoke execution for each platform | CI |
| Pull request | `mise run verify:policy`, `mise run vendor:check`, `mise run license`, and `mise run audit:scripts` on Ubuntu 24.04 | Repository, vendor, Cargo dependency, and scripting dependency policy | CI |
| Linux smoke | `mise run smoke:linux` | GPUI window remains live under Xvfb | CI or implementer |
| Linux keyboard | `mise run smoke:linux-input` | XTest input through XKB, shortcut dispatch, and raw PTYs with both engines | CI or implementer |
| Linux fullscreen | `mise run smoke:linux-fullscreen` | Openbox EWMH property, geometry, PTY input/resize, ignored-request timeout, and Quit capture | CI or implementer |
| macOS fullscreen | `mise run smoke:macos-fullscreen` | AppKit modes, style/focus restoration, retained tabs, presentation leases, and PTY input/resize | CI or implementer |
| Linux quake | `mise run smoke:linux-quake` | Native XTest shortcuts, external focus, composited fade pixels, animations, OS grab rollback, and PTY lifecycle | CI or implementer |
| macOS quake | `mise run smoke:macos-quake` | Native session shortcuts, external AppKit focus, alpha/geometry, Space exit, and PTY lifecycle; requires event-posting permission | CI or implementer |
| macOS menus | `mise run smoke:macos-menus` | Real AppKit shortcut values at startup and reload | CI or implementer |
| macOS keyboard | `mise run smoke:macos-input` | Native input and composition through both engines | CI or implementer |
| macOS Quit | `mise run smoke:macos-quit` | Cancellable AppKit termination through both engines | CI or implementer |
| Scroll benchmark | `mise run ci:benchmarks` on Ubuntu 24.04 | Both engines' snapshot timing, offsets, and queue bounds; paint timing and row reuse when frames arrive | CI or implementer |
| macOS package | `mise run package:macos` | Universal app metadata, icon, executable, and both architectures | CI or implementer |
| Linux package | `mise run package:linux` | Native AppImage and relocatable tarball, dependency policy, provenance, payload equality, and both-engine input smoke | CI or implementer |

CI groups format, static analysis, scripts, and Rust tests into one job per
platform so their setup and debug artifacts are reused. Native desktop smokes,
Linux release benchmarks, and macOS packaging remain separate because combining
them would lengthen the workflow's slowest path. The final
`Verify Linux x86_64` and `Verify macOS arm64` jobs are the stable required
checks; both require every validation job to pass, including native aarch64
Linux checks and smoke coverage.

The smoke job restores Cargo dependencies, workspace build artifacts, and the
verified Ghostty source tree before separately timing source preparation,
compilation, and execution. `mise run ci:smoke` composes the same three phases
for local use; the workflow invokes its build and run subtasks directly so a
slow cache restore, native preparation, compile, or test is visible on its own.
The smoke cache key hashes the repository's pinned Rust, Cargo, and Ghostty
inputs explicitly; it does not vary with unrelated Rust versions preinstalled
on a hosted runner image. Because native smoke execution can fail transiently
after compilation succeeds, the job still saves its build cache on failure so
the requested rerun does not compile from scratch.

The hosted macOS runner may choose a different on-screen window origin after
leaving a native fullscreen Space. The smoke requires restored size, style,
focus, PTY geometry, and a settled on-screen frame; verify exact native position
on physical displays. AppKit can also deliver a late screen-change notification
as the next non-native entry begins. The adapter records it, then lets the
main-thread display identity and frame checks distinguish notification noise
from a real display change. Non-native macOS and Linux restoration remain exact.

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

The fullscreen smoke starts its own Openbox under an isolated Xvfb display.
It checks `_NET_WM_STATE_FULLSCREEN` with `xprop` from `x11-utils`; bare Xvfb
and the existing `twm` benchmarks do not prove EWMH fullscreen support. The
automated Linux coverage is X11-only, matching the enabled GPUI backend.
macOS CI has one virtual display. Physical multi-display entry, disconnect,
Space transitions, and Dock/menu-bar behavior still require the manual checks
in [the fullscreen plan](../plans/fullscreen-modes.md). Record Stage Manager
and Displays Have Separate Spaces settings with that evidence.

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

On macOS, `mise run package:macos` creates
`target/release/bundle/Huterm.app` and verifies its identifier, Cargo-derived
version, Developer Tools category, icon, executable, and arm64/x86_64 slices.
The task installs both Rust targets, builds each with both terminal engines,
and uses `lipo` to assemble the universal executable before packaging. It also
checks the packaged privacy descriptions and the entitlements used by release
signing. Local packages remain unsigned. The GitHub release path is documented
in the [release guide](releases.md).
The macOS CI job cross-compiles the Intel slice on Apple Silicon; this does not
replace native Intel UI and hardware validation, which remains pending.

On Linux, `mise run package:linux` creates native AppImage and binary tarball
artifacts in `dist/` and verifies both as a pair. Run it on the architecture the
artifact targets; cross-compilation is not release evidence. The build records
the exact commit time as `SOURCE_DATE_EPOCH`, enforces a maximum required glibc
symbol version of 2.35, writes only origin-relative ELF runpaths, records Debian
package provenance for privately bundled xkbcommon libraries, and verifies the
full dependency allowlist. It then extracts both formats into fresh temporary
directories and runs the exact-byte input smoke with Alacritty and Ghostty.

`mise run package:linux:verify` rechecks existing artifacts without downloading
tools or rebuilding. It expects the architecture-specific filenames produced by
the build and is suitable for offline release-artifact inspection. AppImage
tools and type-2 runtimes are selected by architecture from the versioned,
SHA-256-pinned manifest in `assets/linux/appimage-tools.json`.

The binary tarball is deliberately AppImage-neutral. For manual installation,
extract it wherever desired, keep its relative directory layout intact, and run
`bin/huterm`. Copy `share/applications/app.huterm.dev.desktop`,
`share/metainfo/app.huterm.dev.metainfo.xml`, and the icon below
`share/icons/hicolor/512x512/apps/` into matching `$XDG_DATA_HOME` directories
for desktop integration. If an AppImage cannot use FUSE, launch it with
`--appimage-extract-and-run`; the tarball is the simpler permanent fallback.

### Updating the app icon

Edit `assets/Huterm.icon` in Icon Composer from Xcode 27, then run this macOS step:

```sh
DEVELOPER_DIR=/Applications/Xcode-beta.app/Contents/Developer mise run icons:generate
mise run icons:check
```

Use the path to your compatible Xcode installation, or omit `DEVELOPER_DIR` if
it is already selected. Generation and manifest verification require Xcode 27
provenance; support for another major version needs a reviewed script change.
Include the source changes and all generated files:
`assets/Huterm.icns`, `assets/Huterm.png`, `assets/Huterm-512.png`,
`assets/macos/Assets.car`, `assets/icons.json`, and `assets/linux/icon.json`.
The 1024-pixel PNG is the default rendered appearance; the deterministic Linux
step downsamples it to the 512-pixel hicolor icon. The asset catalog retains the
layered macOS appearances; the ICNS supplies a static fallback.

Normal builds consume these committed files. `icons:check` verifies source,
generator, and output hashes on macOS and Linux without Apple tools. Changing
the source or generation script requires regeneration. The manifest records
the Xcode, Icon Composer, and macOS versions used, since Apple rendering can
change between releases. `package:macos` also verifies the packaged icon
metadata and exact resource bytes before release signing.

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

## Built-in terminal graphics

The GPUI renderer draws all Box Drawing and Block Elements characters,
U+2500–U+259F, using cell geometry. These characters join across rows and
columns independently of the selected font. It also draws 18 geometric
Powerline separators and eight filled/outlined corner triangles. Other
characters and combining
sequences use normal font shaping. Coverage and adaptation provenance are in
[the terminal graphics notices](../../third-party/terminal-graphics/README.md).

Run `mise run smoke:renderer` to exercise the production preparation and paint
paths with all 186 characters, at font sizes 12, 16, and 20. The smoke checks
font bypass, hidden text, combining-sequence fallback, geometry reuse, wide
glyphs and spacers, final-column width clamping, and display-scale
invalidation. It runs under Xvfb on Linux and natively on macOS. Geometry unit
tests check block coverage, fractional display scales, odd cell dimensions,
line junctions, dashes, shape orientation, hollow interiors, and tiny-cell
fallback. Path tests use the production builder, including a synthetic stroke
that exceeds GPUI's vertex capacity to verify error propagation.

For visual inspection after building the smoke executable, run:

```sh
HUTERM_RENDERER_HOLD=1 target/debug/examples/renderer_smoke
```

The fixture shows stacked scrollbar blocks, connected borders, shades,
selection colors, Powerline joins on colored backgrounds, geometric triangles,
and ordinary text. Close it with the window close button or
interrupt the process. A passing smoke proves that native preparation and
painting ran; it does not replace checking the resulting pixels.
